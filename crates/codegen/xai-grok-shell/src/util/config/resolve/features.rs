use crate::util::config::RemoteSettings;
use toml::Value as TomlValue;

/// Resolve whether ZDR users are allowed to use the product.
///
/// Precedence: requirements > env > config.toml > managed > remote settings > default (false).
pub fn resolve_zdr_access_enabled(
    requirements: Option<&TomlValue>,
    user: Option<&TomlValue>,
    managed: Option<&TomlValue>,
    remote: Option<&RemoteSettings>,
) -> bool {
    use crate::agent::config::BoolFlag;
    fn from_toml(v: Option<&TomlValue>) -> Option<bool> {
        v?.get("features")?.get("zdr_access_enabled")?.as_bool()
    }
    BoolFlag::env("GROK_ZDR_ACCESS_ENABLED")
        .requirement(from_toml(requirements))
        .config(from_toml(user))
        .managed(from_toml(managed))
        .feature_flag(remote.and_then(|r| r.zdr_access_enabled))
        .resolve()
        .value
}

pub(crate) fn turn_transient_retry_from_toml(v: Option<&TomlValue>) -> Option<bool> {
    v?.get("features")?.get("turn_transient_retry")?.as_bool()
}

/// Spawn-time kill switch: `[features] turn_transient_retry`, `GROK_TURN_TRANSIENT_RETRY`, remote key (below local), default on.
/// Loads local layers itself; call once per session.
pub(crate) fn resolve_turn_transient_retry(remote: Option<bool>) -> bool {
    let user_cfg = crate::config::load_effective_config().ok();
    // Merged (user, system, MDM last-wins) like the sibling resolvers, so an enterprise pin is not silently dropped
    let requirements = crate::config::load_merged_requirements();
    compose_turn_transient_retry(requirements.as_ref(), user_cfg.as_ref(), remote)
}

fn compose_turn_transient_retry(
    requirements: Option<&TomlValue>,
    user: Option<&TomlValue>,
    remote: Option<bool>,
) -> bool {
    use turn_transient_retry_from_toml as from_toml;
    crate::agent::config::resolve_turn_transient_retry(
        from_toml(requirements),
        /* cli          */ None,
        from_toml(user),
        /* managed: merged into the config layer by load_effective_config */ None,
        remote,
    )
    .value
}

/// Whether model-catalog (`/v1/models`) and remote-settings (`/v1/settings`) fetches from xAI backends are allowed. That includes the deployment-config sync bundled into the startup prefetch.
/// The background managed-config sync has its own `[features] managed_config` gate. Precedence: requirements (MDM > system > user) > managed (`managed_config.toml` > system managed) > user `config.toml` > default (true).
/// This is an egress gate, so an overlay cannot re-enable a user's or a deployment's "never fetch" decision. Callable before an `AgentConfig` exists (startup prefetch runs pre-agent).
pub fn resolve_remote_fetch_enabled() -> bool {
    // REMOTE FETCH HARD-DISABLED (local fork patch).
    //
    // The startup prefetch (GET /v1/settings to cli-chat-proxy) is an
    // authenticated phone-home: even though it only *downloads* config, the
    // request carries `Authorization`, `x-userid`, and `x-email`, so it tells
    // xAI "this user launched grok" every startup. Returning `false` here
    // suppresses that fetch (and all remote feature-flag delivery); the client
    // falls back to built-in defaults and cannot be re-armed by any config,
    // managed, or requirement layer.
    //
    // Upstream moved the layer walk into `xai_grok_config` and re-exported it
    // from here; the fork keeps a wrapper so every `crate::util::config`
    // caller stays gated. We still evaluate the original layered decision so
    // the resolver stays live and the MDM/managed semantics are preserved for
    // anyone who reverts this patch — but the effective result is forced to
    // `false`.
    let _original = xai_grok_config::resolve_remote_fetch_enabled();
    false
}

pub const REMOTE_FETCH_CONFIG_PATH: &str = "features.remote_fetch";

/// Keys whose dedicated resolver walks managed before user `config.toml`.
/// The effective merge lets the user file win for every other key.
pub const MANAGED_WINS_OVER_USER: &[&str] = &[REMOTE_FETCH_CONFIG_PATH];

#[cfg(test)]
mod turn_transient_retry_toml_tests {
    use super::turn_transient_retry_from_toml;

    #[test]
    fn composition_orders_requirements_over_config_over_remote() {
        let t = |b: bool| -> toml::Value {
            let mut f = toml::map::Map::new();
            f.insert("turn_transient_retry".into(), toml::Value::Boolean(b));
            let mut root = toml::map::Map::new();
            root.insert("features".into(), toml::Value::Table(f));
            toml::Value::Table(root)
        };
        // requirements beat user config; config beats remote; remote beats default.
        assert!(!super::compose_turn_transient_retry(
            Some(&t(false)),
            Some(&t(true)),
            None
        ));
        assert!(!super::compose_turn_transient_retry(
            None,
            Some(&t(false)),
            Some(true)
        ));
        assert!(!super::compose_turn_transient_retry(
            None,
            None,
            Some(false)
        ));
        assert!(super::compose_turn_transient_retry(None, None, None));
    }

    #[test]
    fn reads_the_features_key_and_defaults_absent_to_none() {
        let v: toml::Value = toml::toml! { [features] turn_transient_retry = false }.into();
        assert_eq!(turn_transient_retry_from_toml(Some(&v)), Some(false));
        let empty: toml::Value = toml::toml! { [features] }.into();
        assert_eq!(turn_transient_retry_from_toml(Some(&empty)), None);
        assert_eq!(turn_transient_retry_from_toml(None), None);
    }
}
