//! One model-list request per provider, in that provider's own dialect.
//!
//! Every request carries only the credential the `[model.*]` entries of that
//! same `base_url` already use for chat, so a key never reaches a host its
//! own entry does not name (the patch 12 trust rule).

use std::time::Duration;

/// The `anthropic-version` pin the fork's Messages paths already send.
pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Whole-request budget. A provider that misses it is reported, never trimmed.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Anthropic pages its model list; ask for one page big enough that a real
/// catalog fits, and treat `has_more` as "list incomplete" rather than trimming
/// against a partial answer.
const PAGE_LIMIT: u32 = 1000;

/// Effort ids a provider may advertise; anything else is ignored rather than
/// written into a config the catalog loader would then warn about.
const KNOWN_EFFORT_IDS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Provider metadata keys that mean "context window", in precedence order.
const CONTEXT_WINDOW_KEYS: [&str; 4] = [
    "context_window",
    "context_length",
    "max_context_length",
    "max_input_tokens",
];

/// How a provider authenticates its model-list endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderCredential {
    /// `Authorization: Bearer <key>` (OpenAI-style, Moonshot included).
    Bearer(String),
    /// `x-api-key` + `anthropic-version` (Anthropic Messages).
    AnthropicKey { key: String, version: String },
}

/// One model the provider currently lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredModel {
    pub(crate) id: String,
    pub(crate) context_window: Option<u64>,
    /// Effort ids the provider advertises, already filtered to ones the catalog accepts.
    pub(crate) reasoning_efforts: Vec<String>,
}

/// `GET {base_url}/models` in the dialect `credential` implies.
///
/// The error string is user-facing: it is printed in the plan and the provider
/// is left untouched, so a transport, auth, or parse failure can never trim.
pub(crate) async fn list_models(
    client: &reqwest::Client,
    base_url: &str,
    credential: &ProviderCredential,
) -> Result<Vec<DiscoveredModel>, String> {
    list_models_with_timeout(client, base_url, credential, REQUEST_TIMEOUT).await
}

/// [`list_models`] with an explicit budget, so tests can exercise the timeout path.
pub(crate) async fn list_models_with_timeout(
    client: &reqwest::Client,
    base_url: &str,
    credential: &ProviderCredential,
    timeout: Duration,
) -> Result<Vec<DiscoveredModel>, String> {
    let url = models_url(base_url);
    let request = match credential {
        ProviderCredential::Bearer(key) => client
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}")),
        ProviderCredential::AnthropicKey { key, version } => client
            .get(&url)
            .query(&[("limit", PAGE_LIMIT.to_string())])
            .header("x-api-key", key.as_str())
            .header("anthropic-version", version.as_str()),
    };

    let response = request
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", xai_grok_http::error_cause_chain(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("could not read response: {e}"))?;
    if !status.is_success() {
        return Err(format!("{} — {}", status, first_line(&body)));
    }
    parse_model_list(&body)
}

/// `{base_url}/models`, tolerating a trailing slash on the configured base.
fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

/// Both dialects answer `{ "data": [ { "id": … } ] }`.
/// A paginated Anthropic answer is an error, not a short list: trimming against
/// a truncated page would delete models the provider still serves.
pub(crate) fn parse_model_list(body: &str) -> Result<Vec<DiscoveredModel>, String> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed JSON: {e}"))?;
    let Some(data) = json.get("data").and_then(serde_json::Value::as_array) else {
        return Err(format!(
            "unexpected response shape: no `data` array ({})",
            first_line(body)
        ));
    };
    if json
        .get("has_more")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err("provider returned a partial list (has_more = true)".to_string());
    }
    let models: Vec<DiscoveredModel> = data
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(serde_json::Value::as_str)?;
            Some(DiscoveredModel {
                id: id.to_string(),
                context_window: context_window_of(entry),
                reasoning_efforts: reasoning_efforts_of(entry),
            })
        })
        .collect();
    if models.is_empty() {
        return Err("provider listed no model ids".to_string());
    }
    Ok(models)
}

fn context_window_of(entry: &serde_json::Value) -> Option<u64> {
    CONTEXT_WINDOW_KEYS
        .iter()
        .find_map(|key| entry.get(*key).and_then(serde_json::Value::as_u64))
        .filter(|value| *value > 0)
}

fn reasoning_efforts_of(entry: &serde_json::Value) -> Vec<String> {
    ["reasoning_efforts", "supported_reasoning_efforts"]
        .iter()
        .find_map(|key| entry.get(*key).and_then(serde_json::Value::as_array))
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_ascii_lowercase)
                .filter(|id| KNOWN_EFFORT_IDS.contains(&id.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// Provider error bodies are often HTML or multi-line JSON; one line keeps the plan readable.
fn first_line(body: &str) -> String {
    const MAX: usize = 160;
    let line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return "(empty body)".to_string();
    }
    match trimmed.char_indices().nth(MAX) {
        Some((cut, _)) => format!("{}…", trimmed.get(..cut).unwrap_or(trimmed)),
        None => trimmed.to_string(),
    }
}
