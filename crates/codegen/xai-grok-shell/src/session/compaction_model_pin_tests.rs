//! FORK PATCH 13 (compaction model+effort pinning): [`SessionActor::compaction_sampling_config`],
//! the helper both compaction call sites ask for their sampler config.
//!
//! The pin is driven here through `GROK_COMPACTION_MODEL` / `GROK_COMPACTION_EFFORT` against a stubbed
//! catalog entry, so nothing touches the network or the developer's own `config.toml` (`GROK_HOME` is
//! redirected at a tempdir for the unpinned case, where the disk tier would otherwise be consulted).

use super::super::support::*;
use super::*;
use xai_grok_sampling_types::{ApiBackend, ReasoningEffort, ReasoningEffortOption};
use xai_grok_test_support::EnvGuard;

/// A Sonnet-shaped Messages entry with its own credential and an effort menu that offers `xhigh`.
fn sonnet_entry() -> crate::agent::config::ModelEntry {
    let mut info = crate::agent::config::ModelInfo::fallback("claude-sonnet-5");
    info.base_url = "https://api.anthropic.com/v1".to_string();
    info.api_backend = ApiBackend::Messages;
    info.reasoning_efforts = vec![
        ReasoningEffortOption {
            id: "high".into(),
            value: ReasoningEffort::High,
            label: "High".into(),
            description: None,
            default: true,
        },
        ReasoningEffortOption {
            id: "xhigh".into(),
            value: ReasoningEffort::Xhigh,
            label: "Xhigh".into(),
            description: None,
            default: false,
        },
    ];
    crate::agent::config::ModelEntry {
        info,
        mtls_cert_dir: None,
        api_key: Some("anthropic-key".to_string()),
        env_key: None,
        auth_provider: None,
        api_base_url: None,
    }
}

async fn actor_with_sonnet_in_catalog() -> SessionActor {
    let (gateway_tx, _gateway_rx) = tokio::sync::mpsc::unbounded_channel();
    let (persistence_tx, _persistence_rx) = tokio::sync::mpsc::unbounded_channel();
    let (actor, _ev) = create_test_actor_ex(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor
        .models_manager
        .insert_test_entry("sonnet-compactor", sonnet_entry());
    actor
}

/// With the pin set, the compaction sampler gets the pinned entry's model, endpoint, backend and effort.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn pinned_model_supplies_its_own_endpoint_backend_and_effort() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _model = EnvGuard::set("GROK_COMPACTION_MODEL", "sonnet-compactor");
            let _effort = EnvGuard::set("GROK_COMPACTION_EFFORT", "xHigh");
            let actor = actor_with_sonnet_in_catalog().await;

            let sampling = actor.compaction_sampling_config().await;

            assert!(sampling.is_pinned());
            assert_eq!(sampling.config.model, "claude-sonnet-5");
            assert_eq!(sampling.config.base_url, "https://api.anthropic.com/v1");
            assert_eq!(sampling.config.api_backend, ApiBackend::Messages);
            assert_eq!(
                sampling.config.reasoning_effort,
                Some(ReasoningEffort::Xhigh)
            );
            assert_eq!(
                sampling.config.api_key.as_deref(),
                Some("anthropic-key"),
                "the pinned entry authenticates with its own credential"
            );
            let session = sampling
                .session_fallback
                .expect("a pinned config keeps the session config for the retry");
            assert_eq!(
                session.model, "test",
                "the session model is what the fallback retries with"
            );
        })
        .await;
}

/// An effort the pinned entry does not offer degrades to the entry's default option, not to a 400.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn effort_not_offered_by_the_pinned_model_uses_its_default() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _model = EnvGuard::set("GROK_COMPACTION_MODEL", "sonnet-compactor");
            let _effort = EnvGuard::set("GROK_COMPACTION_EFFORT", "minimal");
            let actor = actor_with_sonnet_in_catalog().await;

            let sampling = actor.compaction_sampling_config().await;

            assert_eq!(
                sampling.config.reasoning_effort,
                Some(ReasoningEffort::High)
            );
        })
        .await;
}

/// Unpinned is upstream behavior: the session's own config, and no fallback to arrange.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn unpinned_compaction_uses_the_session_config() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let home = tempfile::tempdir().expect("tempdir");
            let _home = EnvGuard::set("GROK_HOME", home.path());
            let _model = EnvGuard::unset("GROK_COMPACTION_MODEL");
            let _effort = EnvGuard::unset("GROK_COMPACTION_EFFORT");
            let actor = actor_with_sonnet_in_catalog().await;

            let sampling = actor.compaction_sampling_config().await;
            let session = actor.reconstruct_full_config().await;

            assert!(!sampling.is_pinned());
            assert_eq!(sampling.config.model, session.model);
            assert_eq!(sampling.config.base_url, session.base_url);
            assert_eq!(sampling.config.api_backend, session.api_backend);
            assert_eq!(sampling.config.reasoning_effort, session.reasoning_effort);
        })
        .await;
}

/// A typo in `[compaction] model` must not route compaction anywhere: it falls back to the session.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn unknown_pinned_model_falls_back_to_the_session_config() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _model = EnvGuard::set("GROK_COMPACTION_MODEL", "sonnet-compacter-typo");
            let _effort = EnvGuard::set("GROK_COMPACTION_EFFORT", "xhigh");
            let actor = actor_with_sonnet_in_catalog().await;

            let sampling = actor.compaction_sampling_config().await;

            assert!(!sampling.is_pinned());
            assert_eq!(sampling.config.model, "test");
            assert_eq!(sampling.config.base_url, "http://localhost");
        })
        .await;
}
