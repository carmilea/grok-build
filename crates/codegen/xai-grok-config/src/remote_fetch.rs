//! Reads the `[features] remote_fetch` setting from the config layers.

use crate::{Capability, Distribution};

/// Returns whether fetches of `/v1/models` and `/v1/settings` from xAI backends are allowed.
/// The startup prefetch calls this for its deployment-config sync, before any `AgentConfig` exists.
/// `[features] managed_config` controls the background managed-config sync.
pub fn resolve_remote_fetch_enabled() -> bool {
    // REMOTE FETCH HARD-DISABLED (local fork patch 2, second site).
    //
    // Upstream 1.0.41 moved this resolver out of xai-grok-shell into this
    // crate, which created an independent entry point: `xai-grok-cloud-config`
    // calls `xai_grok_config::resolve_remote_fetch_enabled()` directly
    // (commit.rs, settings_get.rs x2) and never passes through the shell-side
    // wrapper that carries the first site of this patch. Patching only the
    // shell wrapper would leave the settings/models fetch live for every
    // caller in that crate — the same two-entry-point shape as patches 1/1b
    // (telemetry) and 7/7b (Sentry), so it gets the same treatment.
    //
    // Reverting this re-arms GET /v1/settings and the deployment-config
    // prefetch, which is the channel that can server-push `[telemetry] otel_*`
    // back on. We still evaluate the original decision so the layer-walk
    // helpers below stay live for anyone who reverts the patch — but the
    // effective result is `false`. See PATCH.md patch 2.
    let _original = match crate::ConfigLayers::load() {
        Ok(layers) => remote_fetch_enabled_from_layers(&layers),
        // A corrupt user `config.toml` must not discard the requirements and managed settings
        Err(_) => remote_fetch_enabled_from_policy_layers(
            crate::load_merged_requirements().as_ref(),
            crate::load_managed_config().ok().as_ref(),
            crate::load_system_managed_config().ok().as_ref(),
        ),
    };
    false
}

/// `effective_config_base` puts the user layer over managed config.
/// Here a deployment's `remote_fetch = false` wins over a user's `remote_fetch = true`.
pub fn remote_fetch_enabled_from_layers(layers: &crate::ConfigLayers) -> bool {
    // Each new `ConfigLayers` field must go in the list below or be skipped with `_`
    // `env_overlay` and `campaigns` may not override a `remote_fetch = false` from the user or a deployment
    let crate::ConfigLayers {
        system_managed,
        managed,
        user,
        env_overlay: _,
        user_requirements,
        system_requirements,
        mdm_requirements,
        campaigns: _,
    } = layers;
    remote_fetch_enabled_first_match(
        Distribution::current(),
        [
            mdm_requirements.as_ref(),
            system_requirements.as_ref(),
            user_requirements.as_ref(),
            Some(managed),
            Some(system_managed),
            Some(user),
        ],
    )
}

/// The fallback for [`resolve_remote_fetch_enabled`] when `ConfigLayers::load` fails.
/// `load_merged_requirements` lets MDM override system and system override user, like `remote_fetch_enabled_from_layers`.
/// The user `config.toml` is skipped because it holds no deployment policy.
fn remote_fetch_enabled_from_policy_layers(
    merged_requirements: Option<&toml::Value>,
    managed: Option<&toml::Value>,
    system_managed: Option<&toml::Value>,
) -> bool {
    remote_fetch_enabled_first_match(
        Distribution::current(),
        [merged_requirements, managed, system_managed],
    )
}

/// Both public entry points end here, so neither can skip the distribution.
fn remote_fetch_enabled_first_match<'a>(
    distribution: Distribution,
    layers: impl IntoIterator<Item = Option<&'a toml::Value>>,
) -> bool {
    if !distribution.allows(Capability::RemoteFetch) {
        return false;
    }
    layers
        .into_iter()
        .flatten()
        .find_map(remote_fetch_value)
        .unwrap_or(true)
}

fn remote_fetch_value(v: &toml::Value) -> Option<bool> {
    v.get("features")?.get("remote_fetch")?.as_bool()
}

#[cfg(test)]
#[path = "remote_fetch_tests.rs"]
mod tests;
