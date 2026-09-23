// FORK PATCH 14 (api model-list refresh): BYOK `[model.*]` entries are hand-written,
// so a new Kimi or Opus release is unusable until the user edits config.toml and a
// retired model stays in the picker until they notice the 404. This module asks each
// configured provider for its own live model list and reconciles the user's
// `config.toml` against it (add new, trim retired, keep the rest). Reverting it
// removes `/api-model-update` and puts BYOK catalog maintenance back on the user's
// text editor; nothing else in the fork depends on it.
//! `/api-model-update`: reconcile the user's BYOK `[model.*]` entries against
//! each provider's live model list.
//!
//! Scope is deliberately narrow:
//!
//! * only `$GROK_HOME/config.toml` is read and written — never the managed,
//!   requirements, or remote layers, which this command has no business editing;
//! * one request per unique `(base_url, api_backend)` pair, carrying only the
//!   credential that pair's own entries already use (the patch 12 trust rule);
//! * a provider that errors changes nothing, and an entry that is pinned by
//!   `[models]` or in use by the session is never trimmed automatically.

mod discovery;
mod reconcile;
#[cfg(test)]
mod tests;
mod write;

use indexmap::IndexMap;
use xai_grok_sampling_types::ApiBackend;

use crate::agent::config::{ConfigModelOverride, first_own_credential};
use discovery::ProviderCredential;
use reconcile::{CatalogEntry, ProtectedModels, ProviderGroup, ProviderOutcome, ProviderPlan};

/// What the user asked for on the command line.
#[derive(Debug, Clone, Default)]
pub struct UpdateRequest {
    /// `false` is the dry run: discover, print the plan, write nothing.
    pub apply: bool,
    /// Case-insensitive substring matched against each provider's `base_url` host.
    pub provider_filter: Option<String>,
    /// The model this session is running on; never trimmed automatically.
    pub session_model: Option<String>,
}

impl UpdateRequest {
    /// Parse the slash-command argument string: `[apply] [provider-substring]`.
    pub fn parse(args: &str) -> Self {
        let mut apply = false;
        let mut provider_filter = None;
        for token in args.split_whitespace() {
            if token.eq_ignore_ascii_case("apply") {
                apply = true;
            } else if provider_filter.is_none() {
                provider_filter = Some(token.to_string());
            }
        }
        Self {
            apply,
            provider_filter,
            session_model: None,
        }
    }
}

/// A `[model.<key>]` table name paired with its parsed override.
type Entry = (String, ConfigModelOverride);

/// A `[model.<key>]` entry no provider request covers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrphanEntry {
    key: String,
    reason: String,
}

/// One provider's entries plus what it takes to ask it for a list.
struct ProviderSource {
    group: ProviderGroup,
    credential: Option<ProviderCredential>,
    /// Set when the provider must not be asked at all; carries the reason for the report.
    blocked: Option<String>,
}

/// Run the command and render the report the user sees.
///
/// Every failure path returns text rather than an error: the command reports
/// and leaves the config alone, it never half-writes.
pub async fn run(request: UpdateRequest) -> String {
    match run_inner(request).await {
        Ok(report) => report,
        Err(error) => format!("/api-model-update: {error}"),
    }
}

async fn run_inner(request: UpdateRequest) -> Result<String, String> {
    let path = crate::util::config::user_config_path();
    let original = crate::util::config::read_to_string_or_empty(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if original.trim().is_empty() {
        return Ok(format!(
            "No models to refresh: {} has no [model.*] entries.",
            path.display()
        ));
    }
    let raw = crate::util::config::parse_existing_config_toml(&original).map_err(|e| {
        format!(
            "could not parse {}: {}",
            path.display(),
            xai_grok_config::toml_error_detail(&original, &e)
        )
    })?;

    let (sources, orphans) = collect_sources(&raw, request.provider_filter.as_deref());
    if sources.is_empty() && orphans.is_empty() {
        return Ok(format!(
            "No BYOK models to refresh: {} defines no [model.*] entries.",
            path.display()
        ));
    }
    let protected = protected_models(&raw, request.session_model.as_deref());
    let all_config_keys: std::collections::BTreeSet<String> = sources
        .iter()
        .flat_map(|source| source.group.entries.iter().map(|entry| entry.key.clone()))
        .chain(orphans.iter().map(|orphan| orphan.key.clone()))
        .collect();

    let client = xai_grok_http::shared_client();
    let mut plans = Vec::with_capacity(sources.len());
    for source in sources {
        let outcome = match (&source.blocked, &source.credential) {
            (Some(reason), _) => ProviderOutcome::Unchecked {
                reason: reason.clone(),
            },
            (None, None) => ProviderOutcome::Unchecked {
                reason: "no API key resolved from api_key, env_key, or extra_headers".to_string(),
            },
            (None, Some(credential)) => {
                let listed =
                    discovery::list_models(&client, &source.group.base_url, credential).await;
                reconcile::reconcile(&source.group, listed, &protected, &all_config_keys)
            }
        };
        plans.push(ProviderPlan {
            base_url: source.group.base_url.clone(),
            backend: source.group.backend.clone(),
            entry_keys: source
                .group
                .entries
                .iter()
                .map(|entry| entry.key.clone())
                .collect(),
            outcome,
        });
    }

    if !request.apply {
        return Ok(render(&plans, &orphans, None));
    }
    let backup = apply_to_disk(&path, &original, &plans).await?;
    Ok(render(&plans, &orphans, backup.as_deref()))
}

/// Back up and publish the edited `config.toml`; returns the backup path when a write happened.
async fn apply_to_disk(
    path: &std::path::Path,
    planned_from: &str,
    plans: &[ProviderPlan],
) -> Result<Option<String>, String> {
    if !plans.iter().any(ProviderPlan::writes_anything) {
        return Ok(None);
    }
    let guard = crate::util::config::lock_config_writes()
        .await
        .map_err(|e| format!("could not lock config.toml for writing: {e}"))?;
    let path = path.to_path_buf();
    let planned_from = planned_from.to_string();
    let plans: Vec<ProviderPlan> = plans.to_vec();
    let stamp = backup_stamp(chrono::Utc::now());
    let added_on = chrono::Utc::now().format("%Y-%m-%d").to_string();
    guard
        .run_blocking(move || write_locked(&path, &planned_from, &plans, &stamp, &added_on))
        .await
        .map_err(|e| format!("config write task failed: {e}"))?
}

/// The read-modify-write body, under the config write lock.
fn write_locked(
    path: &std::path::Path,
    planned_from: &str,
    plans: &[ProviderPlan],
    stamp: &str,
    added_on: &str,
) -> Result<Option<String>, String> {
    let (dest, current) = crate::util::config::read_follow_bound(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if current != planned_from {
        return Err(format!(
            "{} changed while providers were being queried; nothing written — re-run the command",
            path.display()
        ));
    }
    let updated = write::apply_plan(&current, plans, added_on)?;
    let backup = backup_path(path, stamp);
    xai_grok_config::fs_atomic::write_atomically(&backup, &current, Some(0o600))
        .map_err(|e| format!("could not write backup {}: {e}", backup.display()))?;
    crate::util::config::atomic_write_follow_bound(path, &dest, &updated)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(Some(backup.display().to_string()))
}

/// `config.toml.bak-<rfc3339>` next to the config file.
fn backup_path(path: &std::path::Path, stamp: &str) -> std::path::PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    path.with_file_name(format!("{name}.bak-{stamp}"))
}

/// RFC 3339 to the second. Windows rejects `:` in filenames, so it is folded there.
fn backup_stamp(now: chrono::DateTime<chrono::Utc>) -> String {
    let stamp = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    if cfg!(windows) {
        stamp.replace(':', "-")
    } else {
        stamp
    }
}

/// Group the user's `[model.*]` entries by `(base_url, api_backend)` and work out
/// whether each group can be asked for a list.
fn collect_sources(
    raw: &toml::Value,
    provider_filter: Option<&str>,
) -> (Vec<ProviderSource>, Vec<OrphanEntry>) {
    let parsed = crate::agent::config_model_override_parse::parse_model_overrides(raw);
    // `ApiBackend` is not `Hash`, so the group key carries its label and the
    // backend itself rides along in the value.
    let mut groups: IndexMap<(String, &'static str), (ApiBackend, Vec<Entry>)> = IndexMap::new();
    let mut orphans = Vec::new();
    for (key, entry) in parsed.models {
        let Some(base_url) = entry.base_url.as_deref().map(normalize_base_url) else {
            orphans.push(OrphanEntry {
                key,
                reason: "no base_url in config.toml".to_string(),
            });
            continue;
        };
        let backend = entry.api_backend.clone().unwrap_or_default();
        groups
            .entry((base_url, backend_label(&backend)))
            .or_insert_with(|| (backend, Vec::new()))
            .1
            .push((key, entry));
    }

    let sources = groups
        .into_iter()
        .map(|((base_url, _), (backend, entries))| {
            let group = ProviderGroup {
                base_url: base_url.clone(),
                backend: backend.clone(),
                entries: entries
                    .iter()
                    .map(|(key, entry)| CatalogEntry {
                        key: key.clone(),
                        model: entry.model.clone().unwrap_or_else(|| key.clone()),
                    })
                    .collect(),
            };
            let blocked = blocked_reason(&base_url, &backend, &entries, provider_filter);
            let credential = provider_credential(&backend, &entries);
            ProviderSource {
                group,
                credential,
                blocked,
            }
        })
        .collect();
    (sources, orphans)
}

/// Why this provider must not be asked, if it must not.
fn blocked_reason(
    base_url: &str,
    backend: &ApiBackend,
    entries: &[Entry],
    provider_filter: Option<&str>,
) -> Option<String> {
    if let Some(filter) = provider_filter
        && !host_matches(base_url, filter)
    {
        return Some(format!("not selected by filter `{filter}`"));
    }
    if matches!(backend, ApiBackend::Responses) {
        return Some("responses backend: the xAI catalog is managed upstream".to_string());
    }
    if entries
        .iter()
        .any(|(_, entry)| entry.mtls_cert_dir.is_some())
    {
        return Some("mTLS entry: /api-model-update does not send client certificates".to_string());
    }
    None
}

/// The credential this provider's own entries already use, in the dialect its backend speaks.
fn provider_credential(backend: &ApiBackend, entries: &[Entry]) -> Option<ProviderCredential> {
    match backend {
        ApiBackend::Messages => entries.iter().find_map(|(_, entry)| {
            let key = header(entry, "x-api-key").map(str::to_owned).or_else(|| {
                first_own_credential(entry.api_key.as_deref(), entry.env_key.as_ref())
            })?;
            let version = header(entry, "anthropic-version")
                .unwrap_or(discovery::ANTHROPIC_VERSION)
                .to_string();
            Some(ProviderCredential::AnthropicKey { key, version })
        }),
        ApiBackend::ChatCompletions | ApiBackend::Responses => entries.iter().find_map(|(_, e)| {
            first_own_credential(e.api_key.as_deref(), e.env_key.as_ref())
                .map(ProviderCredential::Bearer)
        }),
    }
}

/// HTTP header names are case-insensitive; `extra_headers` keys are whatever the user typed.
fn header<'a>(entry: &'a ConfigModelOverride, name: &str) -> Option<&'a str> {
    entry
        .extra_headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Role pins from `[models]` plus this session's model: the entries a trim must never touch.
fn protected_models(raw: &toml::Value, session_model: Option<&str>) -> ProtectedModels {
    let mut pins = IndexMap::new();
    if let Some(models) = raw.get("models").and_then(toml::Value::as_table) {
        for (key, value) in models {
            match value {
                toml::Value::String(name) => {
                    pins.insert(key.clone(), name.clone());
                }
                toml::Value::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        if let Some(name) = item.as_str() {
                            pins.insert(format!("{key}[{index}]"), name.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    ProtectedModels {
        pins,
        session_model: session_model.map(str::to_owned),
    }
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

/// Filters match the host, so `moonshot` cannot accidentally select a provider
/// whose *path* happens to contain the word.
fn host_matches(base_url: &str, filter: &str) -> bool {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| base_url.to_string());
    host.to_ascii_lowercase()
        .contains(&filter.to_ascii_lowercase())
}

fn backend_label(backend: &ApiBackend) -> &'static str {
    match backend {
        ApiBackend::ChatCompletions => "chat_completions",
        ApiBackend::Responses => "responses",
        ApiBackend::Messages => "messages",
    }
}

/// The plan, or the result of having written it — the same body either way.
fn render(plans: &[ProviderPlan], orphans: &[OrphanEntry], backup: Option<&str>) -> String {
    let mut out = String::new();
    let (adds, trims) = totals(plans);
    match backup {
        Some(_) => out.push_str("/api-model-update — applied\n"),
        None => out.push_str("/api-model-update — dry run (nothing written)\n"),
    }

    for plan in plans {
        out.push_str(&format!(
            "\n{} ({})\n",
            plan.base_url,
            backend_label(&plan.backend)
        ));
        match &plan.outcome {
            ProviderOutcome::Reconciled {
                adds,
                trims,
                keeps,
                protected,
                skipped_ids,
                collisions,
            } => {
                for add in adds {
                    out.push_str(&format!(
                        "  + {}  (new; inherits from [model.{}])\n",
                        add.id, add.sibling_key
                    ));
                }
                for trim in trims {
                    out.push_str(&format!("  - {trim}  (no longer listed)\n"));
                }
                for keep in keeps {
                    out.push_str(&format!("  = {keep}\n"));
                }
                for entry in protected {
                    out.push_str(&format!(
                        "  ~ {}  (would trim but {} — left in place)\n",
                        entry.key, entry.reason
                    ));
                }
                if !skipped_ids.is_empty() {
                    out.push_str(&format!(
                        "    skipped {} non-chat id(s): {}\n",
                        skipped_ids.len(),
                        skipped_ids.join(", ")
                    ));
                }
                for id in collisions {
                    out.push_str(&format!(
                        "    ? {id} is already a [model.{id}] entry of another provider — not added\n"
                    ));
                }
            }
            ProviderOutcome::Failed { error } => {
                out.push_str(&format!("  ! {error}\n"));
                out.push_str(&format!(
                    "    unchanged: {}\n",
                    join_or_none(&plan.entry_keys)
                ));
            }
            ProviderOutcome::Unchecked { reason } => {
                out.push_str(&format!("  ? unchecked — {reason}\n"));
                out.push_str(&format!(
                    "    unchanged: {}\n",
                    join_or_none(&plan.entry_keys)
                ));
            }
        }
    }

    if !orphans.is_empty() {
        out.push_str("\nunchecked entries\n");
        for orphan in orphans {
            out.push_str(&format!("  ? {} — {}\n", orphan.key, orphan.reason));
        }
    }

    out.push('\n');
    match backup {
        Some(backup) => {
            out.push_str(&format!(
                "Wrote {} add(s) and {} trim(s). Backup: {backup}\n",
                adds, trims
            ));
            out.push_str("New entries are picked up by the next session.\n");
        }
        None if adds + trims == 0 => out.push_str("Nothing to change.\n"),
        None => out.push_str(&format!(
            "Run `/api-model-update apply` to write {adds} add(s) and {trims} trim(s).\n"
        )),
    }
    out
}

fn totals(plans: &[ProviderPlan]) -> (usize, usize) {
    plans
        .iter()
        .fold((0, 0), |(adds, trims), plan| match &plan.outcome {
            ProviderOutcome::Reconciled {
                adds: a, trims: t, ..
            } => (adds + a.len(), trims + t.len()),
            ProviderOutcome::Unchecked { .. } | ProviderOutcome::Failed { .. } => (adds, trims),
        })
}

fn join_or_none(keys: &[String]) -> String {
    if keys.is_empty() {
        "(none)".to_string()
    } else {
        keys.join(", ")
    }
}
