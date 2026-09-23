use super::types::{WebSearchConfig, WebSearchWireFormat};
use crate::attribution::{SharedAttributionCallback, ToolConsumer};
use crate::types::SharedApiKeyProvider;
use async_openai::types::responses as rs;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
/// A minimal, purpose-built HTTP client for calling upstream web search APIs.
///
// FORK PATCH 9 (web search backends): supports OpenAI/xAI Responses,
/// Moonshot/Kimi builtin search, and Anthropic Messages server tool.
#[derive(Clone)]
pub struct WebSearchClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    wire_format: WebSearchWireFormat,
    /// Authoritative domain allowlist from `[toolset.web_search] allowed_domains`. When set it
    /// governs the search and the model's per-call `allowed_domains` is ignored (see
    /// [`Self::resolve_filters`]). Mutually exclusive with `default_excluded_domains`.
    default_allowed_domains: Option<Vec<String>>,
    /// Authoritative domain blocklist from `[toolset.web_search] excluded_domains`.
    /// The model cannot un-set it by naming a blocked domain in its own
    /// `allowed_domains`. Mutually exclusive with `default_allowed_domains`.
    default_excluded_domains: Option<Vec<String>>,
    api_key_provider: Option<SharedApiKeyProvider>,
    /// Optional 401-attribution hook. Callers can wire this so a 401
    /// from the Responses API emits an `auth_401_attribution` event
    /// with `consumer == "WebSearch"`.
    attribution_callback: Option<SharedAttributionCallback>,
}
impl WebSearchClient {
    /// Create a new web search client from `WebSearchConfig::Enabled`.
    ///
    /// Returns `Err` if the config is `Disabled` or if header values are invalid.
    pub fn new(
        config: &WebSearchConfig,
        api_key_provider: Option<SharedApiKeyProvider>,
    ) -> Result<Self, xai_tool_runtime::ToolError> {
        let WebSearchConfig::Enabled {
            api_key,
            base_url,
            model,
            extra_headers,
            alpha_test_key,
            allowed_domains,
            excluded_domains,
            wire_format,
        } = config
        else {
            return Err(xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                "Cannot create WebSearchClient from disabled config".to_string(),
            ));
        };
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // FORK PATCH 9 (web search backends): Anthropic authenticates with
        // `x-api-key` supplied through `extra_headers`; do not send `Authorization`.
        if *wire_format != WebSearchWireFormat::AnthropicServerTool {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|e| {
                    xai_tool_runtime::ToolError::execution(
                        xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                        format!("Invalid API key for header: {e}"),
                    )
                })?,
            );
        }
        // FORK PATCH 9 (web search backends): Moonshot does not support
        // server-side domain filtering; warn once per client construction.
        if *wire_format == WebSearchWireFormat::MoonshotBuiltin
            && (allowed_domains.is_some() || excluded_domains.is_some())
        {
            tracing::warn!(
                "web_search domain filters are configured but are not enforceable on the Moonshot backend; ignoring"
            );
        }
        for (key, value) in extra_headers {
            let header_name = HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("Invalid header name '{key}': {e}"),
                )
            })?;
            let header_value = HeaderValue::from_str(value).map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("Invalid header value for '{key}': {e}"),
                )
            })?;
            headers.insert(header_name, header_value);
        }
        let _ = alpha_test_key;
        let key = crate::util::shared_http::cache_key("web_search", &headers);
        let http = crate::util::shared_http::cached_client(key, || {
            xai_grok_extra_ca::build_reqwest_client(|builder| {
                builder.default_headers(headers.clone())
            })
        })
        .map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to build HTTP client: {e}"),
            )
        })?;
        Ok(Self {
            http,
            base_url: base_url.clone(),
            model: model.clone(),
            wire_format: *wire_format,
            default_allowed_domains: allowed_domains.clone(),
            default_excluded_domains: excluded_domains.clone(),
            api_key_provider,
            attribution_callback: None,
        })
    }
    /// Resolve the effective domain filters for a request. This is required for `excluded_domains` to be a real block. Otherwise the model could
    /// bypass the user's blocklist simply by naming the blocked domain in its own `allowed_domains`. Only when no config policy is set does the
    /// model's per-call allowlist apply. The two lists are mutually exclusive, so at most one of the returned options is `Some`.
    fn resolve_filters(
        &self,
        model_allowed: Option<Vec<String>>,
    ) -> (Option<Vec<String>>, Option<Vec<String>>) {
        if let Some(allowed) = self
            .default_allowed_domains
            .clone()
            .filter(|d| !d.is_empty())
        {
            return (Some(allowed), None);
        }
        if let Some(excluded) = self
            .default_excluded_domains
            .clone()
            .filter(|d| !d.is_empty())
        {
            return (None, Some(excluded));
        }
        (model_allowed.filter(|d| !d.is_empty()), None)
    }
    /// Build the serialized `/responses` request body for a single web search. The request always
    /// carries exactly one tool (`web_search`) at index 0.
    fn build_request_json(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
        excluded_domains: Option<Vec<String>>,
    ) -> Result<serde_json::Value, xai_tool_runtime::ToolError> {
        let err = |msg: String| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                msg,
            )
        };
        let web_search = rs::WebSearchToolArgs::default()
            .filters(rs::WebSearchToolFilters { allowed_domains })
            .build()
            .map_err(|e| err(format!("Failed to build web search tool: {e}")))?;
        let request = rs::CreateResponseArgs::default()
            .model(self.model.clone())
            .input(query.to_string())
            .tools(vec![rs::Tool::WebSearch(web_search)])
            .store(false)
            .temperature(0.1)
            .top_p(0.95)
            .max_output_tokens(8192u32)
            .build()
            .map_err(|e| err(format!("Failed to build request: {e}")))?;
        let mut body = serde_json::to_value(&request)
            .map_err(|e| err(format!("Failed to serialize request: {e}")))?;
        if let Some(excluded) = excluded_domains.filter(|d| !d.is_empty()) {
            let tool = body
                .get_mut("tools")
                .and_then(|t| t.as_array_mut())
                .and_then(|arr| arr.first_mut())
                .and_then(|t| t.as_object_mut());
            if let Some(tool) = tool {
                let filters = tool
                    .entry("filters")
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(obj) = filters.as_object_mut() {
                    obj.insert("excluded_domains".to_owned(), serde_json::json!(excluded));
                }
            }
        }
        Ok(body)
    }
    /// Wire a 401-attribution callback into this client. Idempotent;
    /// safe to call before or after the first request.
    pub fn with_attribution_callback(
        mut self,
        callback: Option<SharedAttributionCallback>,
    ) -> Self {
        self.attribution_callback = callback;
        self
    }
    async fn current_bearer(&self) -> Option<String> {
        crate::types::api_key_provider::resolve_bearer(self.api_key_provider.as_ref()).await
    }
    fn record_401_attribution(&self, sent_bearer: Option<&str>) {
        crate::attribution::emit_401(
            self.attribution_callback.as_ref(),
            ToolConsumer::WebSearch,
            sent_bearer,
        );
    }
    // FORK PATCH 9 (web search backends): dispatch the search to the
    /// configured wire format. Returns `(content, citations)` where content
    /// is the assistant's text and citations are unique source URLs.
    pub async fn search(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>), xai_tool_runtime::ToolError> {
        match self.wire_format {
            WebSearchWireFormat::Responses => self.search_responses(query, allowed_domains).await,
            WebSearchWireFormat::MoonshotBuiltin => {
                self.search_moonshot(query, allowed_domains).await
            }
            WebSearchWireFormat::AnthropicServerTool => {
                let (content, pairs) = self.search_anthropic(query, allowed_domains).await?;
                Ok((content, pairs.into_iter().map(|(_t, u)| u).collect()))
            }
        }
    }
    /// Same as [`Self::search`] but also extracts per-citation titles when
    /// the upstream format surfaces them. Returns `(content, citations_with_titles)`
    /// where each citation is `(title, url)`. Empty `title` strings indicate
    /// the upstream didn't supply one for that URL.
    ///
    /// Used by the cursor-compat `WebSearch` adapter to render a
    /// `Links:\n1. [title](url)` list instead of the LLM synthesis text.
    pub async fn search_with_titles(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<(String, String)>), xai_tool_runtime::ToolError> {
        match self.wire_format {
            WebSearchWireFormat::Responses => {
                self.search_responses_with_titles(query, allowed_domains)
                    .await
            }
            WebSearchWireFormat::MoonshotBuiltin => {
                let (content, _citations) = self.search_moonshot(query, allowed_domains).await?;
                // Moonshot does not return per-source titles on this path.
                Ok((content, Vec::new()))
            }
            WebSearchWireFormat::AnthropicServerTool => {
                self.search_anthropic(query, allowed_domains).await
            }
        }
    }
    /// OpenAI/xAI Responses API path. Kept byte-for-byte versus the pre-patch
    /// implementation except for the method name.
    async fn search_responses(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>), xai_tool_runtime::ToolError> {
        let (allowed, excluded) = self.resolve_filters(allowed_domains);
        let request = self.build_request_json(query, allowed, excluded)?;
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        let sent_bearer = self.current_bearer().await;
        let mut req = self.http.post(&url).json(&request);
        if let Some(ref key) = sent_bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {key}"));
        }
        let response = req.send().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("HTTP request failed: {e}"),
            )
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.record_401_attribution(sent_bearer.as_deref());
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::unauthorized(format!(
                "Responses API returned 401 Unauthorized: {body}"
            ))
            .with_details(serde_json::json!({
                "tool_id": "web_search",
                "status": 401,
            })));
        }
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Responses API returned {status}: {body}"),
            ));
        }
        let bytes = response.bytes().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to read response body: {e}"),
            )
        })?;
        let response_obj: rs::Response = serde_json::from_slice(&bytes).map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to parse response: {e}"),
            )
        })?;
        let content = response_obj
            .output_text()
            .unwrap_or_else(|| "No search results found.".to_string());
        let citations = extract_citations(&response_obj);
        Ok((content, citations))
    }
    /// Responses API path returning titled citations.
    async fn search_responses_with_titles(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<(String, String)>), xai_tool_runtime::ToolError> {
        let (allowed, excluded) = self.resolve_filters(allowed_domains);
        let request = self.build_request_json(query, allowed, excluded)?;
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        let sent_bearer = self.current_bearer().await;
        let mut req = self.http.post(&url).json(&request);
        if let Some(ref key) = sent_bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {key}"));
        }
        let response = req.send().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("HTTP request failed: {e}"),
            )
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.record_401_attribution(sent_bearer.as_deref());
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::unauthorized(format!(
                "Responses API returned 401 Unauthorized: {body}"
            ))
            .with_details(serde_json::json!({
                "tool_id": "web_search",
                "status": 401,
            })));
        }
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Responses API returned {status}: {body}"),
            ));
        }
        let bytes = response.bytes().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to read response body: {e}"),
            )
        })?;
        let response_obj: rs::Response = serde_json::from_slice(&bytes).map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to parse response: {e}"),
            )
        })?;
        let content = response_obj
            .output_text()
            .unwrap_or_else(|| "No search results found.".to_string());
        let pairs = extract_citation_pairs(&response_obj);
        Ok((content, pairs))
    }
    // FORK PATCH 9 (web search backends): Moonshot/Kimi two-hop builtin search.
    async fn search_moonshot(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<String>), xai_tool_runtime::ToolError> {
        let (allowed, excluded) = self.resolve_filters(allowed_domains);
        // Domain filters are not enforceable server-side; resolve_filters still
        // returns them, but we ignore them on this backend.
        let _ = (allowed, excluded);
        let request = self.build_moonshot_hop1_json(query);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let response = self
            .http
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("HTTP request failed: {e}"),
                )
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Moonshot API returned {status}: {body}"),
            ));
        }
        let hop1: serde_json::Value = response.json().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to parse Moonshot response: {e}"),
            )
        })?;
        let message = hop1["choices"][0]["message"].clone();
        let tool_calls = message.get("tool_calls").and_then(|v| v.as_array());
        let finish_reason = hop1["choices"][0]["finish_reason"].as_str();
        // Direct answer with no tool_calls: return immediately.
        if tool_calls.is_none() || tool_calls.is_some_and(|arr| arr.is_empty()) {
            let content = message["content"]
                .as_str()
                .unwrap_or("No search results found.")
                .to_string();
            return Ok((content, Vec::new()));
        }
        let tool_call = tool_calls.expect("checked above")[0].clone();
        let tool_call_id = tool_call["id"]
            .as_str()
            .ok_or_else(|| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    "Moonshot tool_call missing id".to_string(),
                )
            })?
            .to_string();
        let arguments = tool_call["function"]["arguments"]
            .as_str()
            .ok_or_else(|| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    "Moonshot tool_call missing arguments".to_string(),
                )
            })?
            .to_string();
        // Guard against runaway tool-call loops: only one search hop is allowed.
        if finish_reason == Some("tool_calls") {
            let hop2 = self.build_moonshot_hop2_json(query, &message, &tool_call_id, &arguments);
            let response = self.http.post(&url).json(&hop2).send().await.map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("HTTP request failed: {e}"),
                )
            })?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Failed to read error body".to_string());
                return Err(xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("Moonshot API returned {status}: {body}"),
                ));
            }
            let hop2_response: serde_json::Value = response.json().await.map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("Failed to parse Moonshot response: {e}"),
                )
            })?;
            let final_message = hop2_response["choices"][0]["message"].clone();
            let content = final_message["content"]
                .as_str()
                .unwrap_or("No search results found.")
                .to_string();
            // If the model tries to search again, stop and return whatever we have.
            let final_tool_calls = final_message.get("tool_calls").and_then(|v| v.as_array());
            if final_tool_calls.is_some_and(|arr| !arr.is_empty()) && content.is_empty() {
                return Err(xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    "Moonshot returned a second search request instead of an answer".to_string(),
                ));
            }
            return Ok((content, Vec::new()));
        }
        // Fallback: return the assistant message content if present.
        let content = message["content"]
            .as_str()
            .unwrap_or("No search results found.")
            .to_string();
        Ok((content, Vec::new()))
    }
    fn build_moonshot_hop1_json(&self, query: &str) -> serde_json::Value {
        // FORK PATCH 9: no temperature/top_p — kimi-k2.7-code rejects any
        // temperature but 1 ("invalid temperature: only 1 is allowed for this
        // model"), so provider defaults apply.
        serde_json::json!({
            "model": self.model,
            "messages": [{"role": "user", "content": query}],
            "tools": [{"type": "builtin_function", "function": {"name": "$web_search"}}],
            "max_tokens": 8192,
        })
    }
    fn build_moonshot_hop2_json(
        &self,
        query: &str,
        assistant_message: &serde_json::Value,
        tool_call_id: &str,
        arguments: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": query},
                assistant_message,
                {
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "name": "$web_search",
                    "content": arguments,
                }
            ],
            "tools": [{"type": "builtin_function", "function": {"name": "$web_search"}}],
            "max_tokens": 8192,
        })
    }
    // FORK PATCH 9 (web search backends): Anthropic Messages server tool.
    async fn search_anthropic(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
    ) -> Result<(String, Vec<(String, String)>), xai_tool_runtime::ToolError> {
        let (allowed, excluded) = self.resolve_filters(allowed_domains);
        let request = self.build_anthropic_request_json(query, allowed, excluded);
        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let response = self
            .http
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                xai_tool_runtime::ToolError::execution(
                    xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                    format!("HTTP request failed: {e}"),
                )
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Anthropic API returned {status}: {body}"),
            ));
        }
        let body: serde_json::Value = response.json().await.map_err(|e| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                format!("Failed to parse Anthropic response: {e}"),
            )
        })?;
        let mut content_parts = Vec::new();
        let mut sources: Vec<(String, String)> = Vec::new();
        let content_blocks = body["content"].as_array().ok_or_else(|| {
            xai_tool_runtime::ToolError::execution(
                xai_tool_protocol::ToolId::new("web_search").expect("valid"),
                "Anthropic response missing content array".to_string(),
            )
        })?;
        for block in content_blocks {
            let block_type = block["type"].as_str().unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block["text"].as_str() {
                        content_parts.push(text.to_string());
                    }
                }
                "web_search_tool_result" => {
                    // The tool result's own `content` array carries the search results.
                    if let Some(inner) = block["content"].as_array() {
                        for result in inner {
                            if result["type"].as_str() == Some("web_search_result")
                                && let Some(url) = result["url"].as_str()
                            {
                                let title = result["title"].as_str().unwrap_or("").to_string();
                                sources.push((title, url.to_string()));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let mut seen = std::collections::HashSet::new();
        sources.retain(|(_t, url)| seen.insert(url.clone()));
        let content = if content_parts.is_empty() {
            "No search results found.".to_string()
        } else {
            content_parts.join("")
        };
        Ok((content, sources))
    }
    fn build_anthropic_request_json(
        &self,
        query: &str,
        allowed_domains: Option<Vec<String>>,
        excluded_domains: Option<Vec<String>>,
    ) -> serde_json::Value {
        let mut tool = serde_json::json!({
            "type": "web_search_20250305",
            "name": "web_search",
            "max_uses": 3,
        });
        if let Some(allowed) = allowed_domains.filter(|d| !d.is_empty()) {
            tool["allowed_domains"] = serde_json::json!(allowed);
        } else if let Some(excluded) = excluded_domains.filter(|d| !d.is_empty()) {
            tool["blocked_domains"] = serde_json::json!(excluded);
        }
        serde_json::json!({
            "model": self.model,
            "max_tokens": 8192,
            "tools": [tool],
            "messages": [{"role": "user", "content": query}],
        })
    }
}
/// Extract citation URLs from the Response output items.
/// The async-openai crate doesn't provide a helper for this, and the `url` field
/// in `UrlCitationBody` is private, so we serialize to JSON to extract it.
fn extract_citations(response: &rs::Response) -> Vec<String> {
    let mut citations = Vec::new();
    for output_item in &response.output {
        if let rs::OutputItem::Message(output_message) = output_item {
            for message_content in &output_message.content {
                if let rs::OutputMessageContent::OutputText(text_content) = message_content {
                    for annotation in &text_content.annotations {
                        if let rs::Annotation::UrlCitation(url_citation) = annotation
                            && let Ok(json) = serde_json::to_value(url_citation)
                            && let Some(url) = json.get("url").and_then(|v| v.as_str())
                        {
                            citations.push(url.to_string());
                        }
                    }
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    citations.retain(|url| seen.insert(url.clone()));
    citations
}
/// Extract `(title, url)` pairs from the Responses API annotations. `title` may be an empty string
/// when upstream doesn't supply one. URLs are deduplicated while preserving the first-seen order so
/// the rendered `Links:` list is stable and free of duplicates.
fn extract_citation_pairs(response: &rs::Response) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for output_item in &response.output {
        if let rs::OutputItem::Message(output_message) = output_item {
            for message_content in &output_message.content {
                if let rs::OutputMessageContent::OutputText(text_content) = message_content {
                    for annotation in &text_content.annotations {
                        if let rs::Annotation::UrlCitation(url_citation) = annotation
                            && let Ok(json) = serde_json::to_value(url_citation)
                        {
                            let url = json.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            if url.is_empty() {
                                continue;
                            }
                            let title = json
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            pairs.push((title, url.to_string()));
                        }
                    }
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    pairs.retain(|(_t, url)| seen.insert(url.clone()));
    pairs
}
#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    /// Helper to create a Response from JSON for testing.
    fn response_from_json(json: serde_json::Value) -> rs::Response {
        serde_json::from_value(json).expect("Failed to parse test Response JSON")
    }
    /// Build a client with the given configured domain defaults and wire format.
    fn client_with_defaults(
        allowed: Option<Vec<String>>,
        excluded: Option<Vec<String>>,
        wire_format: WebSearchWireFormat,
    ) -> WebSearchClient {
        let config = WebSearchConfig::Enabled {
            api_key: "test-key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: allowed,
            excluded_domains: excluded,
            wire_format,
        };
        WebSearchClient::new(&config, None).expect("client should build")
    }
    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn resolve_filters_config_allowlist_wins_over_model() {
        let client = client_with_defaults(
            Some(v(&["config.com"])),
            None,
            WebSearchWireFormat::Responses,
        );
        let (allowed, excluded) = client.resolve_filters(Some(v(&["model.com"])));
        assert_eq!(allowed, Some(v(&["config.com"])));
        assert!(excluded.is_none());
    }
    #[test]
    fn resolve_filters_uses_config_allowlist_when_model_silent() {
        let client = client_with_defaults(
            Some(v(&["config.com"])),
            None,
            WebSearchWireFormat::Responses,
        );
        let (allowed, excluded) = client.resolve_filters(None);
        assert_eq!(allowed, Some(v(&["config.com"])));
        assert!(excluded.is_none());
    }
    #[test]
    fn resolve_filters_config_blocklist_applies_when_no_allowlist() {
        let client = client_with_defaults(
            None,
            Some(v(&["reddit.com"])),
            WebSearchWireFormat::Responses,
        );
        let (allowed, excluded) = client.resolve_filters(None);
        assert!(allowed.is_none());
        assert_eq!(excluded, Some(v(&["reddit.com"])));
    }
    #[test]
    fn resolve_filters_config_blocklist_cannot_be_bypassed_by_model() {
        let client = client_with_defaults(
            None,
            Some(v(&["github.com"])),
            WebSearchWireFormat::Responses,
        );
        let (allowed, excluded) = client.resolve_filters(Some(v(&["github.com"])));
        assert!(
            allowed.is_none(),
            "model allowlist must not override the block"
        );
        assert_eq!(excluded, Some(v(&["github.com"])));
    }
    #[test]
    fn resolve_filters_no_config_honors_model_allowlist() {
        let client = client_with_defaults(None, None, WebSearchWireFormat::Responses);
        let (allowed, excluded) = client.resolve_filters(Some(v(&["model.com"])));
        assert_eq!(allowed, Some(v(&["model.com"])));
        assert!(excluded.is_none());
    }
    #[test]
    fn build_request_json_injects_excluded_domains() {
        let client = client_with_defaults(None, None, WebSearchWireFormat::Responses);
        let body = client
            .build_request_json("q", None, Some(v(&["reddit.com"])))
            .expect("request json builds");
        let Some(filters) = body.pointer("/tools/0/filters") else {
            panic!("missing tools[0].filters: {body}");
        };
        assert_eq!(
            filters.get("excluded_domains"),
            Some(&serde_json::json!(["reddit.com"]))
        );
        assert!(filters.get("allowed_domains").is_none());
    }
    #[test]
    fn build_request_json_allowlist_only_has_no_excluded_key() {
        let client = client_with_defaults(None, None, WebSearchWireFormat::Responses);
        let body = client
            .build_request_json("q", Some(v(&["docs.x.ai"])), None)
            .expect("request json builds");
        let Some(filters) = body.pointer("/tools/0/filters") else {
            panic!("missing tools[0].filters: {body}");
        };
        assert_eq!(
            filters.get("allowed_domains"),
            Some(&serde_json::json!(["docs.x.ai"]))
        );
        assert!(filters.get("excluded_domains").is_none());
    }
    #[test]
    fn test_new_client_uses_configured_model() {
        let config = WebSearchConfig::Enabled {
            api_key: "test-key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "custom-enterprise-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::Responses,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        assert_eq!(client.model, "custom-enterprise-model");
    }
    /// Counts attribution callback invocations for the test below.
    #[derive(Default, Debug)]
    struct CountingCallback {
        invocations: std::sync::Mutex<Vec<(ToolConsumer, Option<String>)>>,
    }
    impl crate::attribution::Auth401AttributionCallback for CountingCallback {
        fn record_401(&self, consumer: ToolConsumer, sent_bearer_suffix: Option<&str>) {
            self.invocations
                .lock()
                .unwrap()
                .push((consumer, sent_bearer_suffix.map(|s| s.to_string())));
        }
    }
    /// `record_401_attribution` invokes the wired callback with
    /// `ToolConsumer::WebSearch` and the truncated bearer prefix.
    /// The full bearer never crosses the trait boundary.
    #[test]
    fn record_401_attribution_passes_truncated_prefix_to_callback() {
        let cb = std::sync::Arc::new(CountingCallback::default());
        let cb_dyn: crate::attribution::SharedAttributionCallback = cb.clone();
        let config = WebSearchConfig::Enabled {
            api_key: "ignored".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::Responses,
        };
        let client = WebSearchClient::new(&config, None)
            .expect("client should build")
            .with_attribution_callback(Some(cb_dyn));
        client.record_401_attribution(Some("bearer-with-long-tail-aaaadistinct"));
        let calls = cb.invocations.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let Some(call) = calls.first() else {
            panic!("expected one attribution call");
        };
        assert_eq!(call.0, ToolConsumer::WebSearch);
        assert_eq!(call.1.as_deref(), Some("aaaadistinct"));
        assert_eq!(
            call.1.as_deref().map(str::len),
            Some(crate::attribution::BEARER_SUFFIX_LEN),
        );
    }
    /// `record_401_attribution` is a no-op when no callback is wired
    /// -- the BYOK / standalone case must not panic or allocate.
    #[test]
    fn record_401_attribution_is_noop_without_callback() {
        let config = WebSearchConfig::Enabled {
            api_key: "test-key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::Responses,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        client.record_401_attribution(Some("any-bearer"));
        client.record_401_attribution(None);
    }
    #[test]
    fn test_extract_citations_empty_response() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "output": [],
            "model": "test-model"
        }));
        let citations = extract_citations(&response);
        assert!(citations.is_empty());
    }
    #[test]
    fn test_extract_citations_with_url_citations() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Here is some info about Rust.",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://www.rust-lang.org/",
                                    "title": "Rust Programming Language",
                                    "start_index": 0,
                                    "end_index": 10
                                },
                                {
                                    "type": "url_citation",
                                    "url": "https://docs.rs/",
                                    "title": "Docs.rs",
                                    "start_index": 11,
                                    "end_index": 20
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 2);
        let [first, second] = citations.as_slice() else {
            panic!("expected 2 citations: {citations:?}");
        };
        assert_eq!(first, "https://www.rust-lang.org/");
        assert_eq!(second, "https://docs.rs/");
    }
    #[test]
    fn test_extract_citations_deduplicates() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Info with duplicate citations.",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/page1",
                                    "title": "Page 1",
                                    "start_index": 0,
                                    "end_index": 5
                                },
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/page2",
                                    "title": "Page 2",
                                    "start_index": 6,
                                    "end_index": 10
                                },
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/page1",
                                    "title": "Page 1 Again",
                                    "start_index": 11,
                                    "end_index": 15
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 2);
        let [first, second] = citations.as_slice() else {
            panic!("expected 2 citations: {citations:?}");
        };
        assert_eq!(first, "https://example.com/page1");
        assert_eq!(second, "https://example.com/page2");
    }
    #[test]
    fn test_extract_citations_multiple_messages() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "First message",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://first.com/",
                                    "title": "First",
                                    "start_index": 0,
                                    "end_index": 5
                                }
                            ]
                        }
                    ]
                },
                {
                    "type": "message",
                    "id": "msg_2",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Second message",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://second.com/",
                                    "title": "Second",
                                    "start_index": 0,
                                    "end_index": 6
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 2);
        let [first, second] = citations.as_slice() else {
            panic!("expected 2 citations: {citations:?}");
        };
        assert_eq!(first, "https://first.com/");
        assert_eq!(second, "https://second.com/");
    }
    #[test]
    fn test_extract_citations_ignores_non_url_annotations() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Some text",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://valid.com/",
                                    "title": "Valid",
                                    "start_index": 0,
                                    "end_index": 4
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert_eq!(citations.len(), 1);
        assert_eq!(
            citations.first().map(String::as_str),
            Some("https://valid.com/")
        );
    }
    /// A provider that always returns `None`, simulating an API-key user
    /// whose token has aged past the client-side TTL.
    struct NoneProvider;
    impl crate::types::ApiKeyProvider for NoneProvider {
        fn current_api_key(&self) -> Option<String> {
            None
        }
    }
    /// When the dynamic provider returns `None`, the static `api_key` from config must still be
    /// sent as the Authorization header. This is a regression scenario: API-key users past the
    /// 30-day client TTL saw 401 because no auth was sent.
    #[tokio::test]
    async fn static_api_key_is_fallback_when_provider_returns_none() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("Authorization", "Bearer static-key-from-config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_test",
                "object": "response",
                "created_at": 1234567890,
                "status": "completed",
                "model": "test-model",
                "output": [{
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "search result",
                        "annotations": []
                    }]
                }]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Enabled {
            api_key: "static-key-from-config".to_string(),
            base_url: server.uri(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::Responses,
        };
        let provider: SharedApiKeyProvider = std::sync::Arc::new(NoneProvider);
        let client = WebSearchClient::new(&config, Some(provider)).expect("client should build");
        let (content, _citations) = client
            .search("test query", None)
            .await
            .expect("search must succeed with static key fallback");
        assert_eq!(content, "search result");
    }
    /// When the provider returns a fresh key, it overrides the static one.
    #[tokio::test]
    async fn provider_key_overrides_static_key() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        struct FreshProvider;
        impl crate::types::ApiKeyProvider for FreshProvider {
            fn current_api_key(&self) -> Option<String> {
                Some("fresh-key-from-provider".to_string())
            }
        }
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("Authorization", "Bearer fresh-key-from-provider"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "resp_test",
                "object": "response",
                "created_at": 1234567890,
                "status": "completed",
                "model": "test-model",
                "output": [{
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "fresh result",
                        "annotations": []
                    }]
                }]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Enabled {
            api_key: "stale-static-key".to_string(),
            base_url: server.uri(),
            model: "test-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::Responses,
        };
        let provider: SharedApiKeyProvider = std::sync::Arc::new(FreshProvider);
        let client = WebSearchClient::new(&config, Some(provider)).expect("client should build");
        let (content, _citations) = client
            .search("test query", None)
            .await
            .expect("search must succeed with provider key");
        assert_eq!(content, "fresh result");
    }
    #[test]
    fn test_extract_citations_no_annotations() {
        let response = response_from_json(serde_json::json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1234567890,
            "status": "completed",
            "model": "test-model",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Plain text with no annotations",
                            "annotations": []
                        }
                    ]
                }
            ]
        }));
        let citations = extract_citations(&response);
        assert!(citations.is_empty());
    }
    // FORK PATCH 9 (web search backends): Moonshot request-body tests.
    #[test]
    fn moonshot_hop1_body_shape() {
        let client = client_with_defaults(None, None, WebSearchWireFormat::MoonshotBuiltin);
        let body = client.build_moonshot_hop1_json("what is rust");
        assert_eq!(body["model"], "test-model");
        assert_eq!(
            body["messages"],
            serde_json::json!([{"role": "user", "content": "what is rust"}])
        );
        assert_eq!(
            body["tools"],
            serde_json::json!([{"type": "builtin_function", "function": {"name": "$web_search"}}])
        );
        assert_eq!(body["max_tokens"], 8192);
        // kimi-k2.7-code rejects any temperature but 1 — sampling params are
        // omitted so provider defaults apply.
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
    }
    #[test]
    fn moonshot_hop2_message_assembly() {
        let client = client_with_defaults(None, None, WebSearchWireFormat::MoonshotBuiltin);
        let assistant = serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "t-web_search-abc",
                "type": "builtin_function",
                "function": {"name": "$web_search", "arguments": "{\"x\":1}"}
            }]
        });
        let body = client.build_moonshot_hop2_json(
            "what is rust",
            &assistant,
            "t-web_search-abc",
            "{\"x\":1}",
        );
        assert_eq!(body["model"], "test-model");
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[0],
            serde_json::json!({"role": "user", "content": "what is rust"})
        );
        assert_eq!(messages[1], assistant);
        assert_eq!(
            messages[2],
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "t-web_search-abc",
                "name": "$web_search",
                "content": "{\"x\":1}"
            })
        );
    }
    #[tokio::test]
    async fn moonshot_direct_answer_edge_returns_immediately() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "chat_1",
                "object": "chat.completion",
                "created": 1234567890,
                "model": "kimi-k2.7-code",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "Rust is a systems language."},
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;
        let config = WebSearchConfig::Enabled {
            api_key: "moonshot-key".to_string(),
            base_url: server.uri(),
            model: "kimi-k2.7-code".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::MoonshotBuiltin,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let (content, citations) = client
            .search("what is rust", None)
            .await
            .expect("search ok");
        assert_eq!(content, "Rust is a systems language.");
        assert!(citations.is_empty());
    }
    #[tokio::test]
    async fn moonshot_two_hop_synthesis() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let hop1 = serde_json::json!({
            "id": "chat_1",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "kimi-k2.7-code",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "t-web_search-abc",
                        "type": "builtin_function",
                        "function": {"name": "$web_search", "arguments": "{\"search_result\":{\"search_id\":\"s1\"}}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let hop2 = serde_json::json!({
            "id": "chat_2",
            "object": "chat.completion",
            "created": 1234567891,
            "model": "kimi-k2.7-code",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Rust is fast and safe."
                },
                "finish_reason": "stop"
            }]
        });
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(hop2))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(hop1))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let config = WebSearchConfig::Enabled {
            api_key: "moonshot-key".to_string(),
            base_url: server.uri(),
            model: "kimi-k2.7-code".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::MoonshotBuiltin,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let (content, citations) = client
            .search("what is rust", None)
            .await
            .expect("search ok");
        assert_eq!(content, "Rust is fast and safe.");
        assert!(citations.is_empty());
    }
    #[tokio::test]
    async fn moonshot_hop_cap_errors_when_second_hop_is_tool_call_without_content() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let hop1 = serde_json::json!({
            "id": "chat_1",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "kimi-k2.7-code",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "t-web_search-abc",
                        "type": "builtin_function",
                        "function": {"name": "$web_search", "arguments": "{\"x\":1}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let hop2 = serde_json::json!({
            "id": "chat_2",
            "object": "chat.completion",
            "created": 1234567891,
            "model": "kimi-k2.7-code",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "t-web_search-def",
                        "type": "builtin_function",
                        "function": {"name": "$web_search", "arguments": "{\"x\":2}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(hop2))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(hop1))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let config = WebSearchConfig::Enabled {
            api_key: "moonshot-key".to_string(),
            base_url: server.uri(),
            model: "kimi-k2.7-code".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::MoonshotBuiltin,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let result = client.search("what is rust", None).await;
        assert!(
            result.is_err(),
            "expected error when second hop is another tool call without content"
        );
    }
    // FORK PATCH 9 (web search backends): Anthropic request/response tests.
    #[test]
    fn anthropic_body_shape_with_allowed_domains() {
        let client = client_with_defaults(None, None, WebSearchWireFormat::AnthropicServerTool);
        let body = client.build_anthropic_request_json(
            "what is rust",
            Some(vec!["docs.rs".to_string()]),
            None,
        );
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(
            body["messages"],
            serde_json::json!([{"role": "user", "content": "what is rust"}])
        );
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "web_search_20250305");
        assert_eq!(tool["name"], "web_search");
        assert_eq!(tool["max_uses"], 3);
        assert_eq!(tool["allowed_domains"], serde_json::json!(["docs.rs"]));
        assert!(tool.get("blocked_domains").is_none());
    }
    #[test]
    fn anthropic_body_shape_with_blocked_domains() {
        let client = client_with_defaults(None, None, WebSearchWireFormat::AnthropicServerTool);
        let body = client.build_anthropic_request_json(
            "what is rust",
            None,
            Some(vec!["reddit.com".to_string()]),
        );
        let tool = &body["tools"][0];
        assert_eq!(tool["blocked_domains"], serde_json::json!(["reddit.com"]));
        assert!(tool.get("allowed_domains").is_none());
    }
    #[tokio::test]
    async fn anthropic_no_authorization_header() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-opus-4-8",
                "content": [{"type": "text", "text": "Rust is safe."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 3}
            })))
            .mount(&server)
            .await;
        let mut headers = IndexMap::new();
        headers.insert("x-api-key".to_string(), "anthropic-key".to_string());
        let config = WebSearchConfig::Enabled {
            api_key: "should-not-be-sent".to_string(),
            base_url: server.uri(),
            model: "claude-opus-4-8".to_string(),
            extra_headers: headers,
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::AnthropicServerTool,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let (content, citations) = client
            .search("what is rust", None)
            .await
            .expect("search ok");
        assert_eq!(content, "Rust is safe.");
        assert!(citations.is_empty());
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(requests.len(), 1);
        assert!(
            !requests[0].headers.contains_key("authorization"),
            "Anthropic format must not send Authorization header"
        );
    }
    #[tokio::test]
    async fn anthropic_parse_block_array_into_answer_and_sources() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-opus-4-8",
                "content": [
                    {"type": "server_tool_use", "id": "st_1", "name": "web_search", "input": {"queries": ["what is rust"]}},
                    {
                        "type": "web_search_tool_result",
                        "tool_use_id": "st_1",
                        "content": [
                            {"type": "web_search_result", "title": "Rust Lang", "url": "https://www.rust-lang.org/"},
                            {"type": "web_search_result", "title": "Docs.rs", "url": "https://docs.rs/"}
                        ]
                    },
                    {"type": "text", "text": "Rust is a systems programming language."}
                ],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 50, "output_tokens": 20}
            })))
            .mount(&server)
            .await;
        let mut headers = IndexMap::new();
        headers.insert("x-api-key".to_string(), "anthropic-key".to_string());
        headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        let config = WebSearchConfig::Enabled {
            api_key: "ignored".to_string(),
            base_url: server.uri(),
            model: "claude-opus-4-8".to_string(),
            extra_headers: headers,
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
            wire_format: WebSearchWireFormat::AnthropicServerTool,
        };
        let client = WebSearchClient::new(&config, None).expect("client should build");
        let (content, pairs) = client
            .search_with_titles("what is rust", None)
            .await
            .expect("search ok");
        assert_eq!(content, "Rust is a systems programming language.");
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            pairs[0],
            (
                "Rust Lang".to_string(),
                "https://www.rust-lang.org/".to_string()
            )
        );
        assert_eq!(
            pairs[1],
            ("Docs.rs".to_string(), "https://docs.rs/".to_string())
        );
    }
}
