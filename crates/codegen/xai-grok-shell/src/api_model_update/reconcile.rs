//! Pure plan computation: config entries + a provider's live list → adds, trims, keeps.
//!
//! Nothing here touches the network or the filesystem, so every safety rule
//! (never trim on failure, never trim a referenced entry) is unit-testable.

use std::collections::BTreeSet;

use indexmap::IndexMap;
use xai_grok_sampling_types::ApiBackend;

use super::discovery::DiscoveredModel;

/// Substrings that mark a listed id as something the chat catalog cannot use.
/// Conservative on purpose: it only filters *new* ids, never an entry the user
/// already configured.
const NON_CHAT_MARKERS: [&str; 11] = [
    "embed",
    "embedding",
    "moderation",
    "rerank",
    "whisper",
    "tts",
    "audio",
    "speech",
    "image",
    "dall-e",
    "video",
];

/// One `[model.<key>]` table from the user's `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogEntry {
    /// The `[model.<key>]` table name (also the catalog key).
    pub(crate) key: String,
    /// The routing slug sent to the provider (`model`, defaulting to the key).
    pub(crate) model: String,
}

/// Every `[model.*]` entry sharing one `(base_url, api_backend)` pair, in config order.
#[derive(Debug, Clone)]
pub(crate) struct ProviderGroup {
    pub(crate) base_url: String,
    pub(crate) backend: ApiBackend,
    /// Config order; the last one is the newest sibling a new entry inherits from.
    pub(crate) entries: Vec<CatalogEntry>,
}

impl ProviderGroup {
    /// Newest sibling = last declared in `config.toml`, preferring one the
    /// provider still lists so a new entry never inherits from an entry the
    /// same run is about to trim.
    fn newest_sibling(&self, live: &BTreeSet<&str>) -> Option<&CatalogEntry> {
        self.entries
            .iter()
            .rev()
            .find(|entry| live.contains(entry.model.as_str()) || live.contains(entry.key.as_str()))
            .or_else(|| self.entries.last())
    }
}

/// Model names that must survive a trim: role pins from `[models]` plus the
/// model this session is running on.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProtectedModels {
    /// `[models]` key → the model name it pins.
    pub(crate) pins: IndexMap<String, String>,
    pub(crate) session_model: Option<String>,
}

impl ProtectedModels {
    /// Why `entry` may not be trimmed, if it may not.
    pub(crate) fn reason_for(&self, entry: &CatalogEntry) -> Option<String> {
        if let Some(session) = self.session_model.as_deref()
            && (session == entry.key || session == entry.model)
        {
            return Some("in use by this session".to_string());
        }
        self.pins
            .iter()
            .find(|(_, pinned)| *pinned == &entry.key || *pinned == &entry.model)
            .map(|(pin, _)| format!("referenced by [models] {pin}"))
    }
}

/// A new `[model.<id>]` table to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddPlan {
    pub(crate) id: String,
    /// `[model.<key>]` the new entry copies backend, endpoint, and credentials from.
    pub(crate) sibling_key: String,
    /// From provider metadata; `None` copies the sibling's value.
    pub(crate) context_window: Option<u64>,
    /// From provider metadata; empty copies the sibling's value.
    pub(crate) reasoning_efforts: Vec<String>,
}

/// An entry the provider stopped listing but that is pinned elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectedEntry {
    pub(crate) key: String,
    pub(crate) reason: String,
}

/// What this run decided for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderOutcome {
    /// The provider answered; the config can be reconciled against its list.
    Reconciled {
        adds: Vec<AddPlan>,
        trims: Vec<String>,
        keeps: Vec<String>,
        protected: Vec<ProtectedEntry>,
        /// Listed ids skipped by the non-chat denylist.
        skipped_ids: Vec<String>,
        /// Listed ids that already name a `[model.*]` table owned by another
        /// provider. Adding them would overwrite that table, so they are reported.
        collisions: Vec<String>,
    },
    /// The provider was not asked (responses backend, filter, mTLS, no credential).
    Unchecked { reason: String },
    /// The provider was asked and failed. Nothing changes for it.
    Failed { error: String },
}

/// The whole run: one entry per provider, plus config entries no provider covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderPlan {
    pub(crate) base_url: String,
    pub(crate) backend: ApiBackend,
    /// Config keys in this group, for the "left alone" reporting on non-reconciled outcomes.
    pub(crate) entry_keys: Vec<String>,
    pub(crate) outcome: ProviderOutcome,
}

impl ProviderPlan {
    pub(crate) fn writes_anything(&self) -> bool {
        match &self.outcome {
            ProviderOutcome::Reconciled { adds, trims, .. } => {
                !adds.is_empty() || !trims.is_empty()
            }
            ProviderOutcome::Unchecked { .. } | ProviderOutcome::Failed { .. } => false,
        }
    }
}

/// Reconcile one provider's live list against its config entries.
///
/// `listed` carries the discovery result verbatim: an `Err` becomes
/// [`ProviderOutcome::Failed`], which writes nothing at all for this provider.
/// `all_config_keys` is every `[model.*]` table name in the config, so an add
/// can never overwrite an entry that belongs to a different provider.
pub(crate) fn reconcile(
    group: &ProviderGroup,
    listed: Result<Vec<DiscoveredModel>, String>,
    protected: &ProtectedModels,
    all_config_keys: &BTreeSet<String>,
) -> ProviderOutcome {
    let listed = match listed {
        Ok(models) => models,
        Err(error) => return ProviderOutcome::Failed { error },
    };
    let live: BTreeSet<&str> = listed.iter().map(|m| m.id.as_str()).collect();
    let Some(sibling) = group.newest_sibling(&live) else {
        return ProviderOutcome::Unchecked {
            reason: "no configured entries".to_string(),
        };
    };

    let configured: BTreeSet<&str> = group
        .entries
        .iter()
        .flat_map(|entry| [entry.key.as_str(), entry.model.as_str()])
        .collect();

    let mut adds = Vec::new();
    let mut skipped_ids = Vec::new();
    let mut collisions = Vec::new();
    for model in &listed {
        if configured.contains(model.id.as_str()) {
            continue;
        }
        if is_non_chat_id(&model.id) {
            skipped_ids.push(model.id.clone());
            continue;
        }
        if all_config_keys.contains(&model.id) {
            collisions.push(model.id.clone());
            continue;
        }
        adds.push(AddPlan {
            id: model.id.clone(),
            sibling_key: sibling.key.clone(),
            context_window: model.context_window,
            reasoning_efforts: model.reasoning_efforts.clone(),
        });
    }

    let mut trims = Vec::new();
    let mut keeps = Vec::new();
    let mut protected_entries = Vec::new();
    for entry in &group.entries {
        if live.contains(entry.model.as_str()) || live.contains(entry.key.as_str()) {
            keeps.push(entry.key.clone());
        } else if let Some(reason) = protected.reason_for(entry) {
            protected_entries.push(ProtectedEntry {
                key: entry.key.clone(),
                reason,
            });
        } else {
            trims.push(entry.key.clone());
        }
    }

    ProviderOutcome::Reconciled {
        adds,
        trims,
        keeps,
        protected: protected_entries,
        skipped_ids,
        collisions,
    }
}

/// Whether a listed id is obviously not a chat model.
pub(crate) fn is_non_chat_id(id: &str) -> bool {
    let lowered = id.to_ascii_lowercase();
    NON_CHAT_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}
