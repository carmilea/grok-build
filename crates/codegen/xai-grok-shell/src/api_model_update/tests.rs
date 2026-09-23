//! No test here touches a real provider: every request goes to a local wiremock
//! server, and every reconcile/write test is pure.

use std::time::Duration;

use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::discovery::{
    DiscoveredModel, ProviderCredential, list_models, list_models_with_timeout,
};
use super::reconcile::{
    CatalogEntry, ProtectedModels, ProviderGroup, ProviderOutcome, ProviderPlan, reconcile,
};
use super::*;

// ── fixtures ──────────────────────────────────────────────────────────────

fn entry(key: &str) -> CatalogEntry {
    CatalogEntry {
        key: key.to_string(),
        model: key.to_string(),
    }
}

fn group(base_url: &str, backend: ApiBackend, keys: &[&str]) -> ProviderGroup {
    ProviderGroup {
        base_url: base_url.to_string(),
        backend,
        entries: keys.iter().map(|key| entry(key)).collect(),
    }
}

fn all_keys(group: &ProviderGroup) -> std::collections::BTreeSet<String> {
    group
        .entries
        .iter()
        .map(|entry| entry.key.clone())
        .collect()
}

fn listed(ids: &[&str]) -> Result<Vec<DiscoveredModel>, String> {
    Ok(ids
        .iter()
        .map(|id| DiscoveredModel {
            id: (*id).to_string(),
            context_window: None,
            reasoning_efforts: Vec::new(),
        })
        .collect())
}

fn pins(entries: &[(&str, &str)]) -> ProtectedModels {
    ProtectedModels {
        pins: entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        session_model: None,
    }
}

const CONFIG: &str = r#"# grok config — hand written, keep the comments
[ui]
theme = "dark"   # trailing comment

[models]
default = "kimi-k2-thinking"
web_search = "claude-opus-4-8"

# Moonshot: the daily driver
[model.kimi-k2-thinking]
model = "kimi-k2-thinking"
base_url = "https://api.moonshot.ai/v1"
api_backend = "chat_completions"
env_key = "MOONSHOT_API_KEY"
context_window = 262144
reasoning_efforts = ["low", "high"]

[model."kimi-k1.5"]
base_url = "https://api.moonshot.ai/v1"
api_backend = "chat_completions"
env_key = "MOONSHOT_API_KEY"
context_window = 131072

[model.claude-opus-4-8]
base_url = "https://api.anthropic.com/v1"
api_backend = "messages"
extra_headers = { "x-api-key" = "sk-ant-test", "anthropic-version" = "2023-06-01" }
context_window = 200000

[model.grok-4-6]
base_url = "https://api.x.ai/v1"
api_backend = "responses"
env_key = "XAI_API_KEY"
context_window = 256000
"#;

fn raw_config() -> toml::Value {
    crate::util::config::parse_existing_config_toml(CONFIG).expect("fixture parses")
}

// ── discovery (wiremock) ──────────────────────────────────────────────────

#[tokio::test]
async fn chat_completions_list_parses_ids_and_sends_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer moonshot-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "kimi-k2-thinking", "object": "model", "context_length": 262_144},
                {"id": "kimi-k2.7-code", "object": "model"}
            ]
        })))
        .mount(&server)
        .await;

    let models = list_models(
        &xai_grok_http::shared_client(),
        &format!("{}/v1", server.uri()),
        &ProviderCredential::Bearer("moonshot-key".to_string()),
    )
    .await
    .expect("list succeeds");

    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["kimi-k2-thinking", "kimi-k2.7-code"]
    );
    assert_eq!(models.first().and_then(|m| m.context_window), Some(262_144));
}

#[tokio::test]
async fn messages_list_sends_x_api_key_and_version_never_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-api-key", "sk-ant-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(query_param("limit", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "claude-opus-4-8", "display_name": "Claude Opus 4.8"},
                {"id": "claude-fable-1", "display_name": "Claude Fable 1"}
            ],
            "has_more": false
        })))
        .mount(&server)
        .await;

    let models = list_models(
        &xai_grok_http::shared_client(),
        &format!("{}/v1", server.uri()),
        &ProviderCredential::AnthropicKey {
            key: "sk-ant-test".to_string(),
            version: super::discovery::ANTHROPIC_VERSION.to_string(),
        },
    )
    .await
    .expect("list succeeds");

    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["claude-opus-4-8", "claude-fable-1"]
    );
    let requests = server.received_requests().await.unwrap_or_default();
    let first = requests.first().expect("one request");
    assert!(
        !first.headers.contains_key("authorization"),
        "the Anthropic dialect must not send a bearer token"
    );
}

#[tokio::test]
async fn auth_failure_is_reported_not_trimmed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"type":"error","error":{"type":"authentication_error"}}"#),
        )
        .mount(&server)
        .await;

    let error = list_models(
        &xai_grok_http::shared_client(),
        &server.uri(),
        &ProviderCredential::Bearer("bad".to_string()),
    )
    .await
    .expect_err("401 is an error");
    assert!(error.contains("401"), "{error}");
    assert!(error.contains("authentication_error"), "{error}");
}

#[tokio::test]
async fn server_error_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
        .mount(&server)
        .await;

    let error = list_models(
        &xai_grok_http::shared_client(),
        &server.uri(),
        &ProviderCredential::Bearer("key".to_string()),
    )
    .await
    .expect_err("500 is an error");
    assert!(error.contains("500"), "{error}");
}

#[tokio::test]
async fn malformed_json_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{ not json"))
        .mount(&server)
        .await;

    let error = list_models(
        &xai_grok_http::shared_client(),
        &server.uri(),
        &ProviderCredential::Bearer("key".to_string()),
    )
    .await
    .expect_err("garbage is an error");
    assert!(error.contains("malformed JSON"), "{error}");
}

#[tokio::test]
async fn timeout_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(5))
                .set_body_json(serde_json::json!({"data": [{"id": "late"}]})),
        )
        .mount(&server)
        .await;

    let error = list_models_with_timeout(
        &xai_grok_http::shared_client(),
        &server.uri(),
        &ProviderCredential::Bearer("key".to_string()),
        Duration::from_millis(150),
    )
    .await
    .expect_err("a slow provider is an error");
    assert!(error.contains("request failed"), "{error}");
}

#[tokio::test]
async fn paginated_answer_is_an_error_so_a_partial_page_cannot_trim() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "claude-opus-4-8"}],
            "has_more": true
        })))
        .mount(&server)
        .await;

    let error = list_models(
        &xai_grok_http::shared_client(),
        &server.uri(),
        &ProviderCredential::AnthropicKey {
            key: "sk-ant-test".to_string(),
            version: super::discovery::ANTHROPIC_VERSION.to_string(),
        },
    )
    .await
    .expect_err("a partial list is an error");
    assert!(error.contains("partial list"), "{error}");
}

#[test]
fn provider_metadata_keeps_only_effort_ids_the_catalog_accepts() {
    let models = super::discovery::parse_model_list(
        r#"{"data":[{"id":"kimi-next","context_length":128000,
                     "reasoning_efforts":["High","turbo","low"]}]}"#,
    )
    .expect("parses");
    let model = models.first().expect("one model");
    assert_eq!(model.context_window, Some(128_000));
    assert_eq!(
        model.reasoning_efforts,
        vec!["high".to_string(), "low".to_string()],
        "unknown effort ids are dropped rather than written into config.toml"
    );
}

#[test]
fn response_without_data_array_is_an_error() {
    let error = super::discovery::parse_model_list(r#"{"models": ["a"]}"#)
        .expect_err("no data array is an error");
    assert!(error.contains("no `data` array"), "{error}");
}

// ── reconcile ─────────────────────────────────────────────────────────────

#[test]
fn new_id_is_added_and_inherits_from_the_newest_sibling() {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k1.5", "kimi-k2-thinking"],
    );
    let live = Ok(vec![
        DiscoveredModel {
            id: "kimi-k1.5".to_string(),
            context_window: None,
            reasoning_efforts: Vec::new(),
        },
        DiscoveredModel {
            id: "kimi-k2-thinking".to_string(),
            context_window: None,
            reasoning_efforts: Vec::new(),
        },
        DiscoveredModel {
            id: "kimi-k2.7-code".to_string(),
            context_window: Some(300_000),
            reasoning_efforts: vec!["high".to_string()],
        },
    ]);
    let ProviderOutcome::Reconciled { adds, trims, .. } =
        reconcile(&group, live, &ProtectedModels::default(), &all_keys(&group))
    else {
        panic!("expected a reconciled outcome");
    };
    assert!(trims.is_empty());
    let [add] = adds.as_slice() else {
        panic!("expected exactly one add: {adds:?}");
    };
    assert_eq!(add.id, "kimi-k2.7-code");
    assert_eq!(add.sibling_key, "kimi-k2-thinking", "newest sibling wins");
    assert_eq!(add.context_window, Some(300_000));
    assert_eq!(add.reasoning_efforts, vec!["high".to_string()]);
}

#[test]
fn an_add_never_inherits_from_an_entry_the_same_run_trims() {
    // Newest by declaration order is `kimi-k1.5`, but the provider dropped it.
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k2-thinking", "kimi-k1.5"],
    );
    let ProviderOutcome::Reconciled { adds, trims, .. } = reconcile(
        &group,
        listed(&["kimi-k2-thinking", "kimi-new"]),
        &ProtectedModels::default(),
        &all_keys(&group),
    ) else {
        panic!("expected a reconciled outcome");
    };
    assert_eq!(trims, vec!["kimi-k1.5".to_string()]);
    assert_eq!(
        adds.first().map(|a| a.sibling_key.as_str()),
        Some("kimi-k2-thinking"),
        "the donor must be an entry that survives the run"
    );
}

#[test]
fn entry_the_provider_dropped_is_trimmed() {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k1.5", "kimi-k2-thinking"],
    );
    let ProviderOutcome::Reconciled { trims, keeps, .. } = reconcile(
        &group,
        listed(&["kimi-k2-thinking"]),
        &ProtectedModels::default(),
        &all_keys(&group),
    ) else {
        panic!("expected a reconciled outcome");
    };
    assert_eq!(trims, vec!["kimi-k1.5".to_string()]);
    assert_eq!(keeps, vec!["kimi-k2-thinking".to_string()]);
}

#[test]
fn role_pinned_and_session_entries_are_never_trimmed() {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-pinned", "kimi-session", "kimi-orphan"],
    );
    let mut protected = pins(&[("default", "kimi-pinned")]);
    protected.session_model = Some("kimi-session".to_string());

    let ProviderOutcome::Reconciled {
        trims,
        protected: left_in_place,
        ..
    } = reconcile(
        &group,
        listed(&["kimi-new-only"]),
        &protected,
        &all_keys(&group),
    )
    else {
        panic!("expected a reconciled outcome");
    };
    assert_eq!(trims, vec!["kimi-orphan".to_string()]);
    assert_eq!(
        left_in_place
            .iter()
            .map(|e| e.key.as_str())
            .collect::<Vec<_>>(),
        ["kimi-pinned", "kimi-session"]
    );
    assert!(
        left_in_place
            .iter()
            .any(|e| e.reason.contains("[models] default")),
        "{left_in_place:?}"
    );
    assert!(
        left_in_place
            .iter()
            .any(|e| e.reason.contains("in use by this session")),
        "{left_in_place:?}"
    );
}

#[test]
fn a_failed_provider_changes_nothing() {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k2-thinking"],
    );
    let outcome = reconcile(
        &group,
        Err("401 Unauthorized".to_string()),
        &ProtectedModels::default(),
        &all_keys(&group),
    );
    assert_eq!(
        outcome,
        ProviderOutcome::Failed {
            error: "401 Unauthorized".to_string()
        }
    );
    let plan = ProviderPlan {
        base_url: group.base_url.clone(),
        backend: group.backend.clone(),
        entry_keys: vec!["kimi-k2-thinking".to_string()],
        outcome,
    };
    assert!(!plan.writes_anything());
    assert_eq!(
        write::apply_plan(CONFIG, std::slice::from_ref(&plan), "2026-09-23").unwrap(),
        CONFIG,
        "a failed provider must leave config.toml byte-identical"
    );
}

#[test]
fn non_chat_ids_are_skipped_not_added() {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k2-thinking"],
    );
    let ProviderOutcome::Reconciled {
        adds, skipped_ids, ..
    } = reconcile(
        &group,
        listed(&[
            "kimi-k2-thinking",
            "moonshot-embedding-v1",
            "text-moderation-latest",
            "whisper-1",
            "kimi-k2.7-code",
        ]),
        &ProtectedModels::default(),
        &all_keys(&group),
    )
    else {
        panic!("expected a reconciled outcome");
    };
    assert_eq!(
        adds.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["kimi-k2.7-code"]
    );
    assert_eq!(skipped_ids.len(), 3, "{skipped_ids:?}");
}

#[test]
fn an_id_owned_by_another_provider_is_reported_not_overwritten() {
    // The gateway now lists `claude-opus-4-8`, which is already a hand-written
    // `[model.claude-opus-4-8]` entry pointing at Anthropic.
    let gateway = group(
        "https://gateway.internal/v1",
        ApiBackend::ChatCompletions,
        &["gateway-default"],
    );
    let mut all = all_keys(&gateway);
    all.insert("claude-opus-4-8".to_string());
    let ProviderOutcome::Reconciled {
        adds, collisions, ..
    } = reconcile(
        &gateway,
        listed(&["gateway-default", "claude-opus-4-8"]),
        &ProtectedModels::default(),
        &all,
    )
    else {
        panic!("expected a reconciled outcome");
    };
    assert!(adds.is_empty(), "{adds:?}");
    assert_eq!(collisions, vec!["claude-opus-4-8".to_string()]);
}

#[test]
fn a_denylisted_id_already_in_config_is_kept() {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["moonshot-embedding-v1"],
    );
    let ProviderOutcome::Reconciled { keeps, trims, .. } = reconcile(
        &group,
        listed(&["moonshot-embedding-v1"]),
        &ProtectedModels::default(),
        &all_keys(&group),
    ) else {
        panic!("expected a reconciled outcome");
    };
    assert_eq!(keeps, vec!["moonshot-embedding-v1".to_string()]);
    assert!(trims.is_empty());
}

// ── grouping, credentials, filters ────────────────────────────────────────

#[test]
fn entries_group_by_base_url_and_backend() {
    let (sources, orphans) = collect_sources(&raw_config(), None);
    assert!(orphans.is_empty(), "{orphans:?}");
    let described: Vec<(String, &'static str, Vec<String>)> = sources
        .iter()
        .map(|s| {
            (
                s.group.base_url.clone(),
                backend_label(&s.group.backend),
                s.group
                    .entries
                    .iter()
                    .map(|e| e.key.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        described,
        vec![
            (
                "https://api.moonshot.ai/v1".to_string(),
                "chat_completions",
                vec!["kimi-k2-thinking".to_string(), "kimi-k1.5".to_string()],
            ),
            (
                "https://api.anthropic.com/v1".to_string(),
                "messages",
                vec!["claude-opus-4-8".to_string()],
            ),
            (
                "https://api.x.ai/v1".to_string(),
                "responses",
                vec!["grok-4-6".to_string()],
            ),
        ]
    );
}

#[test]
fn responses_backend_is_left_to_upstream() {
    let (sources, _) = collect_sources(&raw_config(), None);
    let xai = sources
        .iter()
        .find(|s| s.group.base_url.contains("x.ai"))
        .expect("xai group");
    assert!(
        xai.blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("responses backend")),
        "{:?}",
        xai.blocked
    );
}

#[test]
fn provider_filter_matches_the_host_only() {
    let (sources, _) = collect_sources(&raw_config(), Some("moonshot"));
    let selected: Vec<&str> = sources
        .iter()
        .filter(|s| s.blocked.is_none())
        .map(|s| s.group.base_url.as_str())
        .collect();
    assert_eq!(selected, ["https://api.moonshot.ai/v1"]);
    assert!(
        sources
            .iter()
            .filter(|s| s.group.base_url.contains("anthropic"))
            .all(|s| s
                .blocked
                .as_deref()
                .is_some_and(|r| r.contains("not selected by filter"))),
    );
    // `v1` appears in every path but in no host, so it selects nothing.
    let (path_filtered, _) = collect_sources(&raw_config(), Some("v1"));
    assert!(path_filtered.iter().all(|s| s.blocked.is_some()));
}

#[test]
fn anthropic_credential_comes_from_extra_headers() {
    let (sources, _) = collect_sources(&raw_config(), None);
    let anthropic = sources
        .iter()
        .find(|s| s.group.base_url.contains("anthropic"))
        .expect("anthropic group");
    assert_eq!(
        anthropic.credential,
        Some(ProviderCredential::AnthropicKey {
            key: "sk-ant-test".to_string(),
            version: "2023-06-01".to_string(),
        })
    );
}

#[test]
fn chat_completions_credential_comes_from_env_key() {
    // SAFETY: single-threaded test-local env write, mirroring the env_key resolution the chat path uses.
    unsafe { std::env::set_var("MOONSHOT_API_KEY", "moonshot-secret") };
    let (sources, _) = collect_sources(&raw_config(), None);
    let moonshot = sources
        .iter()
        .find(|s| s.group.base_url.contains("moonshot"))
        .expect("moonshot group");
    assert_eq!(
        moonshot.credential,
        Some(ProviderCredential::Bearer("moonshot-secret".to_string()))
    );
    unsafe { std::env::remove_var("MOONSHOT_API_KEY") };
}

#[test]
fn entries_without_a_base_url_are_reported_unchecked() {
    let raw = crate::util::config::parse_existing_config_toml(
        "[model.local-llama]\ncontext_window = 8192\n",
    )
    .expect("parses");
    let (sources, orphans) = collect_sources(&raw, None);
    assert!(sources.is_empty());
    assert_eq!(
        orphans,
        vec![OrphanEntry {
            key: "local-llama".to_string(),
            reason: "no base_url in config.toml".to_string(),
        }]
    );
}

#[test]
fn mtls_entries_are_left_unchecked() {
    let raw = crate::util::config::parse_existing_config_toml(
        "[model.private]\nbase_url = \"https://models.internal/v1\"\nmtls_cert_dir = \"/etc/certs\"\n",
    )
    .expect("parses");
    let (sources, _) = collect_sources(&raw, None);
    let source = sources.first().expect("one group");
    assert!(
        source
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("mTLS")),
        "{:?}",
        source.blocked
    );
}

#[test]
fn role_pins_are_read_from_the_models_section() {
    let protected = protected_models(&raw_config(), Some("kimi-k1.5"));
    assert_eq!(
        protected.reason_for(&entry("kimi-k2-thinking")).as_deref(),
        Some("referenced by [models] default")
    );
    assert_eq!(
        protected.reason_for(&entry("claude-opus-4-8")).as_deref(),
        Some("referenced by [models] web_search")
    );
    assert_eq!(
        protected.reason_for(&entry("kimi-k1.5")).as_deref(),
        Some("in use by this session")
    );
    assert_eq!(protected.reason_for(&entry("kimi-other")), None);
}

#[test]
fn argument_parsing_covers_apply_and_filter() {
    let bare = UpdateRequest::parse("");
    assert!(!bare.apply && bare.provider_filter.is_none());
    let apply = UpdateRequest::parse("  apply ");
    assert!(apply.apply && apply.provider_filter.is_none());
    let filtered = UpdateRequest::parse("moonshot");
    assert!(!filtered.apply);
    assert_eq!(filtered.provider_filter.as_deref(), Some("moonshot"));
    let both = UpdateRequest::parse("APPLY anthropic");
    assert!(both.apply);
    assert_eq!(both.provider_filter.as_deref(), Some("anthropic"));
}

// ── write-back ────────────────────────────────────────────────────────────

fn moonshot_plan() -> ProviderPlan {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k2-thinking", "kimi-k1.5"],
    );
    let live = Ok(vec![
        DiscoveredModel {
            id: "kimi-k2-thinking".to_string(),
            context_window: None,
            reasoning_efforts: Vec::new(),
        },
        DiscoveredModel {
            id: "kimi-k2.7-code".to_string(),
            context_window: None,
            reasoning_efforts: Vec::new(),
        },
    ]);
    ProviderPlan {
        base_url: group.base_url.clone(),
        backend: group.backend.clone(),
        entry_keys: vec!["kimi-k2-thinking".to_string(), "kimi-k1.5".to_string()],
        outcome: reconcile(&group, live, &ProtectedModels::default(), &all_keys(&group)),
    }
}

#[test]
fn apply_adds_trims_and_leaves_every_other_byte_alone() {
    let updated =
        write::apply_plan(CONFIG, &[moonshot_plan()], "2026-09-23").expect("plan applies");

    // Unrelated sections and their comments survive verbatim.
    for untouched in [
        "# grok config — hand written, keep the comments\n[ui]\ntheme = \"dark\"   # trailing comment\n",
        "[models]\ndefault = \"kimi-k2-thinking\"\nweb_search = \"claude-opus-4-8\"\n",
        "# Moonshot: the daily driver\n[model.kimi-k2-thinking]\nmodel = \"kimi-k2-thinking\"\n",
        "[model.claude-opus-4-8]\nbase_url = \"https://api.anthropic.com/v1\"\n",
        "[model.grok-4-6]\nbase_url = \"https://api.x.ai/v1\"\n",
    ] {
        assert!(
            updated.contains(untouched),
            "unrelated text changed; missing:\n{untouched}\n--- got ---\n{updated}"
        );
    }
    assert!(
        !updated.contains(r#"[model."kimi-k1.5"]"#),
        "the trimmed entry is gone:\n{updated}"
    );
    assert!(updated.contains("# added by /api-model-update on 2026-09-23"));

    // The new entry inherits endpoint, backend, credentials, and the sibling's window.
    let parsed: toml::Value = toml::from_str(&updated).expect("result is valid TOML");
    let added = parsed
        .get("model")
        .and_then(|m| m.get("kimi-k2.7-code"))
        .expect("new entry exists");
    assert_eq!(
        added.get("model").and_then(toml::Value::as_str),
        Some("kimi-k2.7-code")
    );
    assert_eq!(
        added.get("base_url").and_then(toml::Value::as_str),
        Some("https://api.moonshot.ai/v1")
    );
    assert_eq!(
        added.get("api_backend").and_then(toml::Value::as_str),
        Some("chat_completions")
    );
    assert_eq!(
        added.get("env_key").and_then(toml::Value::as_str),
        Some("MOONSHOT_API_KEY")
    );
    assert_eq!(
        added
            .get("context_window")
            .and_then(toml::Value::as_integer),
        Some(262_144),
        "no provider metadata, so the sibling's window is copied"
    );
    assert_eq!(
        added
            .get("reasoning_efforts")
            .and_then(toml::Value::as_array)
            .map(|a| a.len()),
        Some(2),
        "the sibling's effort menu is copied"
    );

    // Everything else in the document round-trips: only the two entries moved.
    let before: toml::Value = toml::from_str(CONFIG).expect("fixture is valid TOML");
    assert_eq!(before.get("ui"), parsed.get("ui"));
    assert_eq!(before.get("models"), parsed.get("models"));
    assert_eq!(
        before.get("model").and_then(|m| m.get("claude-opus-4-8")),
        parsed.get("model").and_then(|m| m.get("claude-opus-4-8"))
    );
}

#[test]
fn provider_metadata_wins_over_the_sibling_for_new_entries() {
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k2-thinking"],
    );
    let live = Ok(vec![
        DiscoveredModel {
            id: "kimi-k2-thinking".to_string(),
            context_window: None,
            reasoning_efforts: Vec::new(),
        },
        DiscoveredModel {
            id: "kimi-next".to_string(),
            context_window: Some(1_000_000),
            reasoning_efforts: vec!["medium".to_string()],
        },
    ]);
    let plan = ProviderPlan {
        base_url: group.base_url.clone(),
        backend: group.backend.clone(),
        entry_keys: vec!["kimi-k2-thinking".to_string()],
        outcome: reconcile(&group, live, &ProtectedModels::default(), &all_keys(&group)),
    };
    let updated = write::apply_plan(CONFIG, &[plan], "2026-09-23").expect("plan applies");
    let parsed: toml::Value = toml::from_str(&updated).expect("result is valid TOML");
    let added = parsed
        .get("model")
        .and_then(|m| m.get("kimi-next"))
        .expect("new entry exists");
    assert_eq!(
        added
            .get("context_window")
            .and_then(toml::Value::as_integer),
        Some(1_000_000)
    );
    assert_eq!(
        added
            .get("reasoning_efforts")
            .and_then(toml::Value::as_array)
            .and_then(|a| a.first())
            .and_then(toml::Value::as_str),
        Some("medium")
    );
}

#[test]
fn unchecked_providers_write_nothing() {
    let plan = ProviderPlan {
        base_url: "https://api.x.ai/v1".to_string(),
        backend: ApiBackend::Responses,
        entry_keys: vec!["grok-4-6".to_string()],
        outcome: ProviderOutcome::Unchecked {
            reason: "responses backend: the xAI catalog is managed upstream".to_string(),
        },
    };
    assert!(!plan.writes_anything());
    assert_eq!(
        write::apply_plan(CONFIG, &[plan], "2026-09-23").unwrap(),
        CONFIG
    );
}

// ── end to end, against a local provider ──────────────────────────────────

/// Write `config.toml` into a temp `$GROK_HOME` pointed at `provider`, then run
/// the command for real: parse, discover, reconcile, and (optionally) write.
async fn run_against(
    provider: &MockServer,
    apply: bool,
) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let home = tempfile::tempdir().expect("temp grok home");
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "# keep me\n[models]\ndefault = \"kimi-k2-thinking\"\n\n\
             [model.kimi-k2-thinking]\nbase_url = \"{}/v1\"\n\
             api_backend = \"chat_completions\"\napi_key = \"moonshot-secret\"\n\
             context_window = 262144\n\n\
             [model.kimi-retired]\nbase_url = \"{}/v1\"\n\
             api_backend = \"chat_completions\"\napi_key = \"moonshot-secret\"\n\
             context_window = 131072\n",
            provider.uri(),
            provider.uri()
        ),
    )
    .expect("write fixture config");
    let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
    let report = run(UpdateRequest {
        apply,
        provider_filter: None,
        session_model: None,
    })
    .await;
    (home, path, report)
}

fn moonshot_mock() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "data": [
            {"id": "kimi-k2-thinking", "object": "model"},
            {"id": "kimi-k2.7-code", "object": "model"}
        ]
    }))
}

#[tokio::test]
#[serial_test::serial(GROK_HOME)]
async fn dry_run_reports_the_plan_and_leaves_the_file_alone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer moonshot-secret"))
        .respond_with(moonshot_mock())
        .mount(&server)
        .await;

    let (home, path, report) = run_against(&server, false).await;
    let after = std::fs::read_to_string(&path).expect("config still there");
    assert!(after.contains("[model.kimi-retired]"), "{after}");
    assert!(report.contains("dry run (nothing written)"), "{report}");
    assert!(report.contains("+ kimi-k2.7-code"), "{report}");
    assert!(report.contains("- kimi-retired"), "{report}");
    assert!(
        std::fs::read_dir(home.path())
            .expect("list grok home")
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().contains(".bak-")),
        "a dry run must not write a backup"
    );
}

#[tokio::test]
#[serial_test::serial(GROK_HOME)]
async fn apply_writes_the_plan_and_backs_up_first() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(moonshot_mock())
        .mount(&server)
        .await;

    let (home, path, report) = run_against(&server, true).await;
    let after = std::fs::read_to_string(&path).expect("config still there");
    assert!(report.contains("— applied"), "{report}");
    assert!(after.contains("# keep me"), "{after}");
    assert!(after.contains(r#"[model."kimi-k2.7-code"]"#), "{after}");
    assert!(!after.contains("[model.kimi-retired]"), "{after}");
    assert!(
        after.contains("# added by /api-model-update on "),
        "{after}"
    );

    let backups: Vec<String> = std::fs::read_dir(home.path())
        .expect("list grok home")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("config.toml.bak-"))
        .collect();
    let [backup] = backups.as_slice() else {
        panic!("expected exactly one backup: {backups:?}");
    };
    let restored = std::fs::read_to_string(home.path().join(backup)).expect("backup readable");
    assert!(
        restored.contains("[model.kimi-retired]"),
        "the backup holds the pre-edit file: {restored}"
    );
}

#[tokio::test]
#[serial_test::serial(GROK_HOME)]
async fn a_provider_that_fails_leaves_the_file_byte_identical() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("nope"))
        .mount(&server)
        .await;

    let home = tempfile::tempdir().expect("temp grok home");
    let path = home.path().join("config.toml");
    let before = format!(
        "[model.kimi-k2-thinking]\nbase_url = \"{}/v1\"\napi_key = \"k\"\ncontext_window = 262144\n",
        server.uri()
    );
    std::fs::write(&path, &before).expect("write fixture config");
    let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());

    let report = run(UpdateRequest {
        apply: true,
        provider_filter: None,
        session_model: None,
    })
    .await;
    assert!(report.contains("! 500"), "{report}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("config still there"),
        before,
        "a failed provider must not rewrite config.toml"
    );
}

// ── report ────────────────────────────────────────────────────────────────

#[test]
fn dry_run_report_names_every_verdict_and_writes_nothing() {
    let mut protected = pins(&[("default", "kimi-k2-thinking")]);
    protected.session_model = None;
    let group = group(
        "https://api.moonshot.ai/v1",
        ApiBackend::ChatCompletions,
        &["kimi-k2-thinking", "kimi-k1.5", "kimi-legacy"],
    );
    let plans = vec![
        ProviderPlan {
            base_url: group.base_url.clone(),
            backend: group.backend.clone(),
            entry_keys: vec![],
            outcome: reconcile(
                &group,
                listed(&["kimi-k1.5", "kimi-new"]),
                &protected,
                &all_keys(&group),
            ),
        },
        ProviderPlan {
            base_url: "https://api.anthropic.com/v1".to_string(),
            backend: ApiBackend::Messages,
            entry_keys: vec!["claude-opus-4-8".to_string()],
            outcome: ProviderOutcome::Failed {
                error: "401 Unauthorized".to_string(),
            },
        },
    ];
    let orphans = vec![OrphanEntry {
        key: "local-llama".to_string(),
        reason: "no base_url in config.toml".to_string(),
    }];
    let report = render(&plans, &orphans, None);
    assert!(report.contains("dry run (nothing written)"), "{report}");
    assert!(report.contains("+ kimi-new"), "{report}");
    assert!(report.contains("- kimi-legacy"), "{report}");
    assert!(report.contains("= kimi-k1.5"), "{report}");
    assert!(
        report.contains("~ kimi-k2-thinking  (would trim but referenced by [models] default"),
        "{report}"
    );
    assert!(report.contains("! 401 Unauthorized"), "{report}");
    assert!(report.contains("unchanged: claude-opus-4-8"), "{report}");
    assert!(report.contains("local-llama — no base_url"), "{report}");
    assert!(
        report.contains("Run `/api-model-update apply` to write 1 add(s) and 1 trim(s)."),
        "{report}"
    );
}
