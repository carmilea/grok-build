use crate::agent::config::{ConfigSource, Resolved};

/// Default auto-compact threshold (% of context window) when no source sets it.
pub const DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT: u8 = 85;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompactionToolChoice {
    #[default]
    Auto,
    None,
}

impl std::str::FromStr for CompactionToolChoice {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "none" => Ok(Self::None),
            _ => Err(()),
        }
    }
}

pub(crate) const ENV_COMPACTION_TOOL_CHOICE: &str = "GROK_COMPACTION_TOOL_CHOICE";

pub(crate) fn resolve_compaction_tool_choice_from(
    env: Option<&str>,
    config: Option<&str>,
    remote: Option<&str>,
) -> CompactionToolChoice {
    env.and_then(|s| s.parse().ok())
        .or_else(|| config.and_then(|s| s.parse().ok()))
        .or_else(|| remote.and_then(|s| s.parse().ok()))
        .unwrap_or_default()
}

// FORK PATCH 13 (compaction model+effort pinning).
//
// Upstream always summarizes with whatever the session is sampling with, so a cheap/fast chat model
// also writes the compaction summary — the one call in a session where a stronger model pays for
// itself, and where a third-party chat model may not even be the one you want holding the whole
// transcript. These two resolvers add `[compaction] model` / `[compaction] effort` (env
// `GROK_COMPACTION_MODEL` / `GROK_COMPACTION_EFFORT`), naming a `[models]` catalog entry that
// compaction is pinned to plus the reasoning effort to ask it for.
//
// Reverting them silently returns compaction to the session model and that model's own effort:
// no error, no warning — the pin just stops having any effect.
//
// The TOML tier is read overlay-free (`effective_config_base_without_overlay`), like `[auto_mode]`
// and `[prompt_suggestions]`, so a `GROK_CONFIG` overlay cannot re-route compaction — and with it
// the entire conversation — to an endpoint the user's own config never named.
pub(crate) const ENV_COMPACTION_MODEL: &str = "GROK_COMPACTION_MODEL";
pub(crate) const ENV_COMPACTION_EFFORT: &str = "GROK_COMPACTION_EFFORT";

/// Trimmed value, or `None` for an unset/blank tier (an empty `GROK_COMPACTION_MODEL=` means "no pin").
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

/// Precedence: env `GROK_COMPACTION_MODEL` > `[compaction] model` > unset (the session model).
fn resolve_compaction_model_from(
    env: Option<&str>,
    config: Option<&str>,
) -> Resolved<Option<String>> {
    if let Some(model) = non_blank(env) {
        return Resolved::new(Some(model.to_owned()), ConfigSource::Env);
    }
    if let Some(model) = non_blank(config) {
        return Resolved::new(Some(model.to_owned()), ConfigSource::UserConfig);
    }
    Resolved::new(None, ConfigSource::Default)
}

/// Precedence: env `GROK_COMPACTION_EFFORT` > `[compaction] effort` > unset (the pinned model's own effort).
/// Tokens are canonical reasoning-effort names, parsed case-insensitively (`"xHigh"` == `"xhigh"`).
/// An unparseable tier is warned and falls through to the next one, like [`resolve_compaction_tool_choice_from`].
fn resolve_compaction_effort_from(
    env: Option<&str>,
    config: Option<&str>,
) -> Resolved<Option<xai_grok_sampling_types::ReasoningEffort>> {
    for (raw, source) in [
        (non_blank(env), ConfigSource::Env),
        (non_blank(config), ConfigSource::UserConfig),
    ] {
        let Some(token) = raw else { continue };
        match xai_grok_sampling_types::parse_canonical_effort_token(token) {
            Some(effort) => return Resolved::new(Some(effort), source),
            None => tracing::warn!(
                value = %token,
                source = %source,
                "compaction effort is not a reasoning-effort token (none, minimal, low, medium, high, xhigh, max); ignoring"
            ),
        }
    }
    Resolved::new(None, ConfigSource::Default)
}

/// The `[compaction]` table as the trusted disk layers merged it (no `GROK_CONFIG` overlay).
/// A malformed table resolves to the defaults, which is the unpinned path.
fn compaction_section_from_disk() -> crate::agent::config::CompactionConfig {
    let Ok(layers) = crate::config::ConfigLayers::load() else {
        return Default::default();
    };
    let Some(table) = layers
        .effective_config_base_without_overlay()
        .get("compaction")
        .cloned()
    else {
        return Default::default();
    };
    table
        .try_into()
        .map_err(|e| tracing::warn!(error = %e, "[compaction]: dropped malformed local table"))
        .unwrap_or_default()
}

/// FORK PATCH 13: the catalog id compaction is pinned to, or `None` for the session model.
pub(crate) fn resolve_compaction_model() -> Resolved<Option<String>> {
    resolve_compaction_model_from(
        std::env::var(ENV_COMPACTION_MODEL).ok().as_deref(),
        compaction_section_from_disk().model.as_deref(),
    )
}

/// FORK PATCH 13: the reasoning effort requested of the pinned compaction model, or `None` for its own.
pub(crate) fn resolve_compaction_effort()
-> Resolved<Option<xai_grok_sampling_types::ReasoningEffort>> {
    resolve_compaction_effort_from(
        std::env::var(ENV_COMPACTION_EFFORT).ok().as_deref(),
        compaction_section_from_disk().effort.as_deref(),
    )
}

pub(crate) const ENV_AUTO_COMPACT_THRESHOLD_PERCENT: &str = "GROK_AUTO_COMPACT_THRESHOLD_PERCENT";

/// Precedence (highest first): env `GROK_AUTO_COMPACT_THRESHOLD_PERCENT` user TOML `[model.<id>].auto_compact_threshold_percent` (`cfg.config_models`, the merge of user and managed `[model.<id>]` sections) user TOML `[session].auto_compact_threshold_percent` remote settings per-model `ModelInfo.auto_compact_threshold_percent` (kept out of `ConfigModelOverride::apply` so the user and remote per-model tiers stay distinct) remote settings global `RemoteSettings.auto_compact_threshold_percent` default `DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT`
pub(crate) fn resolve_auto_compact_threshold_percent(
    cfg: &crate::agent::config::Config,
    model_id: &str,
    model: Option<&crate::agent::config::ModelInfo>,
) -> u8 {
    resolve_auto_compact_threshold_percent_from_tiers(
        cfg.config_models
            .get(model_id)
            .and_then(|m| m.auto_compact_threshold_percent),
        cfg.session.auto_compact_threshold_percent,
        model.and_then(|m| m.auto_compact_threshold_percent),
        cfg.remote_settings
            .as_ref()
            .and_then(|r| r.auto_compact_threshold_percent),
    )
}

/// [`resolve_auto_compact_threshold_percent`] for callers without a `Config`, e.g. subagent spawn paths that pass the parent's tiers explicitly.
/// There the per-model tier uses the subagent's resolved model id, not the parent's.
pub(crate) fn resolve_auto_compact_threshold_percent_from_tiers(
    user_per_model: Option<u8>,
    user_global: Option<u8>,
    gb_per_model: Option<u8>,
    gb_global: Option<u8>,
) -> u8 {
    fn clamp_env(raw: i64) -> Option<u8> {
        if (0..=100).contains(&raw) {
            Some(raw as u8)
        } else {
            tracing::debug!(
                source = "env",
                value = raw,
                "auto_compact_threshold_percent out of range 0..=100; ignoring"
            );
            None
        }
    }
    let from_env = || -> Option<u8> {
        std::env::var(ENV_AUTO_COMPACT_THRESHOLD_PERCENT)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(clamp_env)
    };

    from_env()
        .or(user_per_model)
        .or(user_global)
        .or(gb_per_model)
        .or(gb_global)
        .unwrap_or(DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT)
}

/// Fleet p99 of successful compactions is ~181s (≈225s at 400K+ input).
/// So 300s clears the legit tail with margin while cutting a runaway from the ~600s deadline.
pub const DEFAULT_COMPACTION_WALL_CLOCK_BUDGET_SECS: u64 = 300;

/// Below this, a configured budget is almost certainly a misconfig (fleet success p99 ~181s); logged at `warn`, not clamped.
const COMPACTION_WALL_CLOCK_BUDGET_WARN_SECS: u64 = 120;

const ENV_COMPACTION_WALL_CLOCK_BUDGET_SECS: &str = "GROK_COMPACTION_WALL_CLOCK_SECS";

/// Precedence: env `GROK_COMPACTION_WALL_CLOCK_SECS`, then remote `RemoteSettings.compaction_wall_clock_budget_secs`, then the client default. `0` **disables** it.
/// Low values are warned, not clamped: any "safe" clamp (e.g. 30s) would itself cut legit compactions, trading one silent failure for another. Ops own the value.
pub(crate) fn resolve_compaction_wall_clock_budget_secs(gb_global: Option<u64>) -> u64 {
    let from_env = std::env::var(ENV_COMPACTION_WALL_CLOCK_BUDGET_SECS)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    let resolved = from_env
        .or(gb_global)
        .unwrap_or(DEFAULT_COMPACTION_WALL_CLOCK_BUDGET_SECS);
    if resolved > 0 && resolved < COMPACTION_WALL_CLOCK_BUDGET_WARN_SECS {
        tracing::warn!(
            budget_secs = resolved,
            "compaction wall-clock budget {resolved}s is below {COMPACTION_WALL_CLOCK_BUDGET_WARN_SECS}s \
             and may cut legitimate compactions (fleet success p99 ~181s); set 0 to disable"
        );
    }
    resolved
}

#[cfg(test)]
mod compaction_wall_clock_budget_tests {
    use super::resolve_compaction_wall_clock_budget_secs as resolve;

    // Assumes GROK_COMPACTION_WALL_CLOCK_SECS is unset in the test env.
    #[test]
    fn default_global_disable_and_no_clamp() {
        assert_eq!(resolve(None), 300); // client default
        assert_eq!(resolve(Some(450)), 450); // server global wins
        assert_eq!(resolve(Some(0)), 0); // 0 explicitly disables (no clamp)
        assert_eq!(resolve(Some(5)), 5); // low values pass through (warned, not clamped)
    }
}

#[cfg(test)]
mod compaction_tool_choice_tests {
    use super::{CompactionToolChoice, resolve_compaction_tool_choice_from as resolve};

    #[test]
    fn default_is_auto() {
        assert_eq!(resolve(None, None, None), CompactionToolChoice::Auto);
    }

    #[test]
    fn precedence_env_over_config_over_remote() {
        assert_eq!(
            resolve(Some("none"), Some("auto"), Some("auto")),
            CompactionToolChoice::None
        );
        assert_eq!(
            resolve(None, Some("none"), Some("auto")),
            CompactionToolChoice::None
        );
        assert_eq!(
            resolve(None, None, Some("none")),
            CompactionToolChoice::None
        );
    }

    #[test]
    fn garbage_falls_through() {
        assert_eq!(
            resolve(Some("garbage"), None, Some("none")),
            CompactionToolChoice::None
        );
        assert_eq!(
            resolve(Some("garbage"), Some("also-bad"), None),
            CompactionToolChoice::Auto
        );
    }

    #[test]
    fn from_str_case_insensitive() {
        assert_eq!("AUTO".parse(), Ok(CompactionToolChoice::Auto));
        assert_eq!(" None ".parse(), Ok(CompactionToolChoice::None));
        assert!("required".parse::<CompactionToolChoice>().is_err());
    }
}

/// FORK PATCH 13 (compaction model+effort pinning): the precedence cores behind
/// [`resolve_compaction_model`] / [`resolve_compaction_effort`]. The public resolvers are those cores
/// plus a process-env read and an overlay-free `[compaction]` disk read, so the tiers are tested here
/// and only the env tier is exercised end to end (a disk tier would depend on the developer's own config).
#[cfg(test)]
mod compaction_model_pin_tests {
    use super::{
        ConfigSource, ENV_COMPACTION_EFFORT, ENV_COMPACTION_MODEL,
        resolve_compaction_effort as resolve_effort_from_process,
        resolve_compaction_effort_from as resolve_effort,
        resolve_compaction_model as resolve_model_from_process,
        resolve_compaction_model_from as resolve_model,
    };
    use xai_grok_sampling_types::ReasoningEffort;
    use xai_grok_test_support::EnvGuard;

    #[test]
    fn model_unset_is_the_session_model() {
        let resolved = resolve_model(None, None);
        assert_eq!(resolved.value, None);
        assert_eq!(resolved.source, ConfigSource::Default);
        // A blank tier is "unset", not a pin to the empty model id.
        assert_eq!(resolve_model(Some("  "), Some("")).value, None);
    }

    #[test]
    fn model_env_beats_toml_beats_default() {
        let from_env = resolve_model(Some("sonnet-compactor"), Some("from-toml"));
        assert_eq!(from_env.value.as_deref(), Some("sonnet-compactor"));
        assert_eq!(from_env.source, ConfigSource::Env);

        let from_toml = resolve_model(None, Some(" from-toml "));
        assert_eq!(from_toml.value.as_deref(), Some("from-toml"));
        assert_eq!(from_toml.source, ConfigSource::UserConfig);
    }

    #[test]
    fn effort_env_beats_toml_and_parses_mixed_case() {
        let from_env = resolve_effort(Some("xHigh"), Some("low"));
        assert_eq!(from_env.value, Some(ReasoningEffort::Xhigh));
        assert_eq!(from_env.source, ConfigSource::Env);

        let from_toml = resolve_effort(None, Some(" MAX "));
        assert_eq!(from_toml.value, Some(ReasoningEffort::Max));
        assert_eq!(from_toml.source, ConfigSource::UserConfig);
    }

    #[test]
    fn unparseable_effort_falls_through_to_the_next_tier() {
        assert_eq!(
            resolve_effort(Some("turbo"), Some("high")).value,
            Some(ReasoningEffort::High)
        );
        let none_usable = resolve_effort(Some("turbo"), Some("also-bad"));
        assert_eq!(none_usable.value, None);
        assert_eq!(none_usable.source, ConfigSource::Default);
    }

    /// The env tier reaches the process resolvers; it wins over any `[compaction]` table on disk.
    #[test]
    #[serial_test::serial]
    fn process_resolvers_read_the_env_tier() {
        let _model = EnvGuard::set(ENV_COMPACTION_MODEL, "env-pinned-compactor");
        let _effort = EnvGuard::set(ENV_COMPACTION_EFFORT, "XHIGH");
        assert_eq!(
            resolve_model_from_process().value.as_deref(),
            Some("env-pinned-compactor")
        );
        assert_eq!(
            resolve_effort_from_process().value,
            Some(ReasoningEffort::Xhigh)
        );
    }

    /// The TOML tier reaches the process resolvers, and env still wins over it.
    /// `GROK_HOME` isolates the read from the developer's own `config.toml`.
    #[test]
    #[serial_test::serial]
    fn process_resolvers_read_the_toml_tier() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("config.toml"),
            "[compaction]\nmodel = \"toml-compactor\"\neffort = \"High\"\n",
        )
        .expect("write config.toml");
        let _home = EnvGuard::set("GROK_HOME", home.path());
        let _model = EnvGuard::unset(ENV_COMPACTION_MODEL);
        let _effort = EnvGuard::unset(ENV_COMPACTION_EFFORT);

        let model = resolve_model_from_process();
        assert_eq!(model.value.as_deref(), Some("toml-compactor"));
        assert_eq!(model.source, ConfigSource::UserConfig);
        assert_eq!(
            resolve_effort_from_process().value,
            Some(ReasoningEffort::High)
        );

        let _model_env = EnvGuard::set(ENV_COMPACTION_MODEL, "env-compactor");
        assert_eq!(
            resolve_model_from_process().value.as_deref(),
            Some("env-compactor")
        );
    }

    /// No `[compaction]` table and no env: nothing is pinned, so compaction keeps the session model.
    #[test]
    #[serial_test::serial]
    fn process_resolvers_default_to_the_session_model() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home = EnvGuard::set("GROK_HOME", home.path());
        let _model = EnvGuard::unset(ENV_COMPACTION_MODEL);
        let _effort = EnvGuard::unset(ENV_COMPACTION_EFFORT);
        assert_eq!(resolve_model_from_process().value, None);
        assert_eq!(resolve_effort_from_process().value, None);
    }
}
