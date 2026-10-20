use std::collections::VecDeque;
use std::env;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client as ReqwestClient, Response, header};
use tokio::time::sleep;
use tracing::{Instrument, Span, field};

use crate::AccumulatingStream;
use crate::backoff::ExponentialBackoff;
use crate::error::{Error, HttpMetadata, RequestPhase, Result, map_reqwest_error};
use crate::observability;
use crate::sse::process_message_stream_sse;
use crate::types::{
    CompactionRequest, ContextManagement, CostReport, CostReportParams, DeletedFile,
    DeletedMessageBatch, FileInfo, FileList, FileListParams, McpServer, Message, MessageBatch,
    MessageBatchCreateParams, MessageBatchListParams, MessageBatchListResponse, MessageBatchResult,
    MessageCountTokensParams, MessageCreateParams, MessageStreamEvent, MessageTokensCount, Model,
    ModelInfo, ModelListParams, ModelListResponse, OutputConfig, RateLimits, ToolUnionParam, Usage,
    UsageReport, UsageReportParams, retry_after_from_headers,
};

fn final_failure(error: &Error) {
    let span = Span::current();
    span.record("otel.status_code", "ERROR");
    if let Some(id) = error.request_id() {
        span.record("request_id", id);
    }
    if let Some(status) = error.status_code() {
        span.record("http.status_code", status);
    }
    tracing::error!(
        error = error as &dyn std::error::Error,
        "API request failed"
    );
}

fn record_usage(model: &Model, usage: &Usage) {
    let span = Span::current();
    span.record("input_tokens", usage.input_tokens);
    span.record("output_tokens", usage.output_tokens);
    span.record(
        "cache_read_input_tokens",
        usage.cache_read_input_tokens.unwrap_or(0),
    );
    span.record(
        "cache_creation_input_tokens",
        usage.cache_creation_input_tokens.unwrap_or(0),
    );
    if let Model::Known(known) = model {
        span.record("cost_micro_cents", known.cost_micro_cents(usage));
    }
}

/// Emit one `claudius::usage` event for a completed request.
///
/// Fires once per completed `send` call and once per stream that reaches `message_stop`
/// (from the merged usage).  `cost_micro_cents` is `None` for models that are not a
/// `KnownModel`.  Batch results are not reported; call `KnownModel::cost_micro_cents` per
/// result.  Its tier modifier is whatever `usage.service_tier` the API reports: batch results
/// are expected to report `"batch"`, but that is unverified.
fn count_usage(model: &Model, usage: &Usage) {
    let tokens = |tokens: Option<i32>| u64::try_from(tokens.unwrap_or(0)).unwrap_or(0);
    let cost_micro_cents = match model {
        Model::Known(known) => Some(known.cost_micro_cents(usage)),
        _ => None,
    };
    tracing::info!(
        target: "claudius::usage",
        model = %model,
        input_tokens = tokens(Some(usage.input_tokens)),
        output_tokens = tokens(Some(usage.output_tokens)),
        cache_read_input_tokens = tokens(usage.cache_read_input_tokens),
        cache_creation_input_tokens = tokens(usage.cache_creation_input_tokens),
        cost_micro_cents = ?cost_micro_cents,
        "usage"
    );
}

// Own the span until EOF, failure, or drop. Enter it only while polling, never across
// another task's execution. Fuse the unfold so repeated polls after EOF are safe.
fn trace_stream<S, T>(inner: S, span: Span) -> impl Stream<Item = Result<T>>
where
    S: Stream<Item = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    stream::unfold(Some((Box::pin(inner), span)), |state| async move {
        let (mut inner, span) = state?;
        let item = async {
            let item = inner.next().await;
            if let Some(Err(error)) = &item {
                final_failure(error);
            }
            item
        }
        .instrument(span.clone())
        .await;
        item.map(|item| {
            let next = if item.is_err() {
                None
            } else {
                Some((inner, span))
            };
            (item, next)
        })
    })
    .fuse()
}

const DEFAULT_API_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_API_VERSION: &str = "2023-06-01";
/// Total deadline for non-streaming requests; streams use a separate read timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";
const COMPACT_EDIT_BETA: &str = "compact-2026-01-12";
const COMPACTION_BETA: &str = "compact-2026-09-04";
const MCP_CLIENT_BETA: &str = "mcp-client-2025-11-20";
const TASK_BUDGETS_BETA: &str = "task-budgets-2026-03-13";
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const FALLBACK_CREDIT_BETA: &str = "fallback-credit-2026-07-01";
const THINKING_DISPLAY_UPDATES_BETA: &str = "thinking-display-updates-2026-08-18";

/// Allowed lifetime of an uploaded file: one hour to ninety days.
const FILE_EXPIRY_SECONDS: std::ops::RangeInclusive<u64> = 3_600..=7_776_000;

/// A multipart boundary built from a process-wide counter and the current time.
fn multipart_boundary() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("claudius-boundary-{nanos:x}-{count:x}")
}

/// Builds the `multipart/form-data` body of a file upload.
fn build_file_upload_body(
    boundary: &str,
    filename: &str,
    mime_type: &str,
    bytes: &[u8],
    expires_in_seconds: Option<u64>,
) -> Vec<u8> {
    let escaped_filename = filename.replace('\\', "\\\\").replace('"', "\\\"");
    let mut body = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{escaped_filename}\"\r\nContent-Type: {mime_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    if let Some(seconds) = expires_in_seconds {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; \
                 name=\"expires_in_seconds\"\r\n\r\n{seconds}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// Betas implied by the request fields shared by messages and count-tokens requests.
fn push_feature_betas(
    auto_betas: &mut Vec<&'static str>,
    context_management: Option<&ContextManagement>,
    compaction: Option<&CompactionRequest>,
    mcp_servers: Option<&[McpServer]>,
    tools: Option<&[ToolUnionParam]>,
    output_config: Option<&OutputConfig>,
) {
    if let Some(context_management) = context_management {
        auto_betas.push(CONTEXT_MANAGEMENT_BETA);
        if context_management.has_compact_edit() {
            auto_betas.push(COMPACT_EDIT_BETA);
        }
    }
    if compaction.is_some() {
        auto_betas.push(COMPACTION_BETA);
    }
    let has_mcp_toolset = tools.is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| matches!(tool, ToolUnionParam::McpToolset(_)))
    });
    if mcp_servers.is_some() || has_mcp_toolset {
        auto_betas.push(MCP_CLIENT_BETA);
    }
    if output_config.is_some_and(|config| config.task_budget.is_some()) {
        auto_betas.push(TASK_BUDGETS_BETA);
    }
}

/// Betas implied by the features a Messages request uses.
fn create_params_betas(params: &MessageCreateParams) -> Vec<&'static str> {
    let mut auto_betas = Vec::new();
    push_feature_betas(
        &mut auto_betas,
        params.context_management.as_ref(),
        params.compaction.as_ref(),
        params.mcp_servers.as_deref(),
        params.tools.as_deref(),
        params.output_config.as_ref(),
    );
    auto_betas
}

/// Betas implied by the features a count-tokens request uses.
fn count_params_betas(params: &MessageCountTokensParams) -> Vec<&'static str> {
    let mut auto_betas = Vec::new();
    push_feature_betas(
        &mut auto_betas,
        params.context_management.as_ref(),
        None,
        params.mcp_servers.as_deref(),
        params.tools.as_deref(),
        params.output_config.as_ref(),
    );
    auto_betas
}

/// Base URLs of providers that authenticate via the standard HTTP
/// `Authorization: Bearer ${API_KEY}` header rather than (or in addition to)
/// Anthropic's `x-api-key` header. When the client's configured base URL
/// matches one of these entries, the API key is automatically sent as
/// `Authorization: Bearer ${API_KEY}` on every request.
///
/// Baseten's Anthropic-compatible inference gateway is one such provider: it
/// expects a bearer token instead of `x-api-key`.
const BASE_URLS_REQUIRING_AUTH_HEADER: &[&str] = &["https://inference.baseten.co"];

/// Returns `true` when `base_url` (after trimming any trailing slash) matches
/// one of the entries in [`BASE_URLS_REQUIRING_AUTH_HEADER`].
fn requires_auth_header(base_url: &str) -> bool {
    let normalized = base_url.trim_end_matches('/');
    BASE_URLS_REQUIRING_AUTH_HEADER
        .iter()
        .any(|url| url.trim_end_matches('/') == normalized)
}

const MAX_MESSAGE_BATCH_RESULT_LINE_BYTES: usize = 64 * 1024 * 1024;

struct MessageBatchJsonlState<S> {
    byte_stream: S,
    buffer: Vec<u8>,
    pending_lines: VecDeque<Vec<u8>>,
    finished: bool,
}

fn parse_message_batch_result_line(line: &[u8]) -> Result<MessageBatchResult> {
    let text = std::str::from_utf8(line).map_err(|e| {
        Error::encoding(
            format!("Invalid UTF-8 in message batch results JSONL: {e}"),
            Some(Box::new(e)),
        )
    })?;

    serde_json::from_str::<MessageBatchResult>(text).map_err(|e| {
        Error::serialization(
            format!("Failed to parse message batch results JSONL line: {e}"),
            Some(Box::new(e)),
        )
    })
}

fn trim_jsonl_line(mut line: Vec<u8>) -> Vec<u8> {
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
    line
}

fn process_message_batch_result_jsonl<S>(
    byte_stream: S,
) -> impl Stream<Item = Result<MessageBatchResult>>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin + 'static,
{
    let state = MessageBatchJsonlState {
        byte_stream,
        buffer: Vec::new(),
        pending_lines: VecDeque::new(),
        finished: false,
    };

    stream::unfold(state, |mut state| async move {
        loop {
            if let Some(line) = state.pending_lines.pop_front() {
                if line.is_empty() {
                    continue;
                }
                return Some((parse_message_batch_result_line(&line), state));
            }

            if state.finished {
                if state.buffer.is_empty() {
                    return None;
                }
                let line = trim_jsonl_line(std::mem::take(&mut state.buffer));
                if line.is_empty() {
                    continue;
                }
                return Some((parse_message_batch_result_line(&line), state));
            }

            match state.byte_stream.next().await {
                Some(Ok(bytes)) => {
                    state.buffer.extend_from_slice(&bytes);
                    if state.buffer.len() > MAX_MESSAGE_BATCH_RESULT_LINE_BYTES {
                        state.buffer.clear();
                        state.finished = true;
                        return Some((
                            Err(Error::streaming(
                                format!(
                                    "Message batch results JSONL line exceeded maximum size of {} bytes",
                                    MAX_MESSAGE_BATCH_RESULT_LINE_BYTES
                                ),
                                None,
                            )),
                            state,
                        ));
                    }

                    while let Some(newline) = state.buffer.iter().position(|byte| *byte == b'\n') {
                        let line = trim_jsonl_line(state.buffer.drain(..=newline).collect());
                        if !line.is_empty() {
                            state.pending_lines.push_back(line);
                        }
                    }
                }
                Some(Err(err)) => {
                    state.finished = true;
                    return Some((Err(map_reqwest_error(err, RequestPhase::Body)), state));
                }
                None => {
                    state.finished = true;
                }
            }
        }
    })
}

fn response_metadata(response: &Response) -> HttpMetadata {
    let headers = response.headers();
    Span::current().record("http.status_code", response.status().as_u16());
    if let Some(id) = headers
        .get("request-id")
        .and_then(|value| value.to_str().ok())
    {
        Span::current().record("request_id", id);
    }
    let retry_after = retry_after_from_headers(headers);
    HttpMetadata {
        status: response.status().as_u16(),
        error_type: None,
        request_id: headers
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(String::from),
        retry_after,
    }
}

fn configured_value(names: &[&str]) -> Result<Option<String>> {
    for name in names {
        match env::var(name) {
            Ok(value) if !value.trim().is_empty() => return Ok(Some(value.trim().to_string())),
            Ok(_) | Err(env::VarError::NotPresent) => {}
            Err(env::VarError::NotUnicode(_)) => {
                return Err(Error::validation(
                    format!("Configuration variable {name} must contain Unicode text"),
                    Some((*name).into()),
                ));
            }
        }
    }
    Ok(None)
}

#[derive(Clone, Copy)]
enum ResponseMode {
    Buffered,
    Streaming,
}

/// Client for the Anthropic API with performance optimizations.
#[derive(Clone)]
pub struct Anthropic {
    api_key: String,
    client: ReqwestClient,
    stream_client: ReqwestClient,
    base_url: String,
    timeout: Duration,
    max_retries: usize,
    /// Cached headers for performance - Arc for cheap cloning
    cached_headers: Arc<HeaderMap>,
    /// Beta feature headers included on every request.
    default_betas: Vec<String>,
    /// Rate-limit headers from the most recent response, shared by clones.
    last_rate_limits: Arc<Mutex<Option<RateLimits>>>,
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anthropic")
            .field("api_key", &"[REDACTED]")
            .field("cached_headers", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl Anthropic {
    fn build_http_client(timeout: Duration, mode: ResponseMode) -> Result<ReqwestClient> {
        let builder = ReqwestClient::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout.min(Duration::from_secs(60)))
            .pool_max_idle_per_host(10) // Connection pooling optimization
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60));
        let builder = if matches!(mode, ResponseMode::Streaming) {
            builder.read_timeout(timeout.min(Duration::from_secs(60)))
        } else {
            builder.timeout(timeout)
        };
        builder.build().map_err(|e| {
            Error::http_client(
                format!("Failed to build HTTP client: {e}"),
                Some(Box::new(e)),
            )
        })
    }

    /// Resolve an API key value, handling file:// URLs
    fn resolve_api_key(key_value: &str) -> Result<String> {
        let key_value = key_value.trim();
        let Some(path) = key_value.strip_prefix("file://") else {
            return Ok(key_value.to_string());
        };
        fs::read_to_string(path)
            .map(|content| content.trim().to_string())
            .map_err(|error| Error::io(format!("Failed to read API key file '{path}'"), error))
    }

    /// Create a new Anthropic client.
    ///
    /// The API key can be provided directly or read from the CLAUDIUS_API_KEY or ANTHROPIC_API_KEY
    /// environment variables. If an environment variable value starts with "file://", it will be
    /// treated as a file path and the API key will be read from that file.
    ///
    /// The base URL is resolved from the CLAUDIUS_BASE_URL or ANTHROPIC_BASE_URL environment
    /// variables, in that order. If neither is set, the default Anthropic API URL is used.
    pub fn new(api_key: Option<String>) -> Result<Self> {
        let key = match api_key {
            Some(key) => key,
            None => {
                configured_value(&["CLAUDIUS_API_KEY", "ANTHROPIC_API_KEY"])?.ok_or_else(|| {
                    Error::authentication(
                        "No nonempty API key configured in CLAUDIUS_API_KEY or ANTHROPIC_API_KEY",
                    )
                })?
            }
        };
        let api_key = Self::resolve_api_key(&key)?;

        let timeout = DEFAULT_TIMEOUT;
        let client = Self::build_http_client(timeout, ResponseMode::Buffered)?;

        // Resolve base URL from environment variables, defaulting to the API URL.
        // This is resolved before building headers because some providers
        // (see `BASE_URLS_REQUIRING_AUTH_HEADER`) require an `Authorization`
        // header derived from the base URL.
        let base_url = configured_value(&["CLAUDIUS_BASE_URL", "ANTHROPIC_BASE_URL"])?
            .unwrap_or_else(|| DEFAULT_API_URL.to_string());

        // Pre-build headers for performance
        let cached_headers = Arc::new(Self::build_default_headers(&api_key, &base_url)?);

        Ok(Self {
            api_key,
            client,
            stream_client: Self::build_http_client(timeout, ResponseMode::Streaming)?,
            base_url,
            timeout,
            max_retries: 3,
            cached_headers,
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        })
    }

    /// Set a custom base URL for this client.
    ///
    /// This method allows you to specify a different API endpoint for the client.
    /// The base URL should be the root URL without the `/v1/` suffix - this will
    /// be added automatically when constructing request URLs.
    ///
    /// # Examples
    ///
    /// ```
    /// # use claudius::Anthropic;
    /// // For Anthropic's API (default)
    /// let client = Anthropic::new(Some("api-key".to_string()))?
    ///     .with_base_url("https://api.anthropic.com".to_string())?;
    ///
    /// // For Minimax (international)
    /// let client = Anthropic::new(Some("api-key".to_string()))?
    ///     .with_base_url("https://api.minimax.io/anthropic".to_string())?;
    ///
    /// // For Minimax (China)
    /// let client = Anthropic::new(Some("api-key".to_string()))?
    ///     .with_base_url("https://api.minimaxi.com/anthropic".to_string())?;
    /// # Ok::<(), claudius::Error>(())
    /// ```
    pub fn with_base_url(mut self, base_url: String) -> Result<Self> {
        // The set of default headers is derived from the base URL: providers
        // listed in `BASE_URLS_REQUIRING_AUTH_HEADER` get an `Authorization`
        // header. Rebuild the cached headers when switching to or from such a
        // provider so every subsequent request carries the right auth.
        self.cached_headers = Arc::new(Self::build_default_headers(&self.api_key, &base_url)?);
        self.base_url = base_url;
        Ok(self)
    }

    /// Set a custom timeout for this client.
    ///
    /// Sets the total deadline for non-streaming requests. Streaming reads use
    /// an inactivity timeout of the smaller of this value and 60 seconds.
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.timeout = timeout;
        self.stream_client = Self::build_http_client(timeout, ResponseMode::Streaming)?;

        self.client =
            Self::build_http_client(timeout, ResponseMode::Buffered).map_err(|e| match e {
                Error::HttpClient { source, .. } => Error::http_client(
                    "Failed to build HTTP client with new timeout",
                    source.map(|src| Box::new(src) as Box<dyn std::error::Error + Send + Sync>),
                ),
                other => other,
            })?;
        Ok(self)
    }

    /// Set the maximum number of retries for this client.
    ///
    /// This method allows you to specify how many times to retry failed requests.
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Get the API key being used by this client.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Set default beta feature headers included on every request.
    ///
    /// These are merged with any per-request betas and auto-detected betas
    /// (like `context-management-2025-06-27`). Duplicates are removed.
    pub fn with_default_betas(
        mut self,
        betas: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.default_betas = betas.into_iter().map(Into::into).collect();
        self
    }

    /// Set both a custom base URL and timeout for this client.
    ///
    /// This is a convenience method that chains with_base_url and with_timeout.
    pub fn with_base_url_and_timeout(self, base_url: String, timeout: Duration) -> Result<Self> {
        self.with_base_url(base_url)?.with_timeout(timeout)
    }

    /// Build default headers for API requests (static method for initialization).
    ///
    /// When `base_url` matches one of the entries in
    /// [`BASE_URLS_REQUIRING_AUTH_HEADER`] (e.g. Baseten's inference gateway),
    /// the API key is additionally sent as `Authorization: Bearer ${API_KEY}`.
    fn build_default_headers(api_key: &str, base_url: &str) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(api_key).map_err(|e| {
                Error::validation(
                    format!("Invalid API key format: {e}"),
                    Some("api_key".to_string()),
                )
            })?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_API_VERSION),
        );
        if requires_auth_header(base_url) {
            let bearer = format!("Bearer {api_key}");
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&bearer).map_err(|e| {
                    Error::validation(
                        format!("Invalid Authorization header value: {e}"),
                        Some("api_key".to_string()),
                    )
                })?,
            );
        }
        for name in ["x-api-key", "authorization"] {
            if let Some(value) = headers.get_mut(name) {
                value.set_sensitive(true);
            }
        }
        Ok(headers)
    }

    /// Get cached headers for performance (no allocation needed).
    pub fn default_headers(&self) -> HeaderMap {
        (*self.cached_headers).clone()
    }

    /// Build a full endpoint URL from the base URL and endpoint path.
    ///
    /// This method handles trailing slashes gracefully and always inserts `/v1/`
    /// between the base URL and endpoint path. This allows the base URL to be
    /// specified without requiring a specific format (with or without trailing slash,
    /// with or without `/v1/` suffix).
    ///
    /// # Examples
    ///
    /// - Base: `https://api.anthropic.com`, endpoint: `messages` → `https://api.anthropic.com/v1/messages`
    /// - Base: `https://api.minimax.io/anthropic`, endpoint: `messages` → `https://api.minimax.io/anthropic/v1/messages`
    /// - Base: `https://example.com/`, endpoint: `models` → `https://example.com/v1/models`
    fn build_url(&self, endpoint: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{}/v1/{}", base, endpoint)
    }

    /// Collect all beta strings from client defaults, per-request, and auto-detected sources.
    ///
    /// Returns a deduplicated, ordered list.
    fn collect_betas(&self, request_betas: Option<&[String]>, auto_betas: &[&str]) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.default_betas
            .iter()
            .map(String::as_str)
            .chain(request_betas.into_iter().flatten().map(String::as_str))
            .chain(auto_betas.iter().copied())
            .filter(|beta| seen.insert(*beta))
            .map(String::from)
            .collect()
    }

    /// Build headers with the `anthropic-beta` header set from the given betas.
    ///
    /// Returns `None` if there are no betas, avoiding an unnecessary header clone.
    fn headers_with_betas(&self, betas: &[String]) -> Option<HeaderMap> {
        if betas.is_empty() {
            return None;
        }
        let mut headers = self.default_headers();
        let value = betas.join(", ");
        // Beta header values are ASCII, so from_str should not fail.
        if let Ok(hv) = HeaderValue::from_str(&value) {
            headers.insert("anthropic-beta", hv);
        }
        Some(headers)
    }

    /// Retry wrapper that implements exponential backoff with header-based retry-after
    async fn retry_with_backoff<F, Fut, T>(&self, operation: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match operation().await {
                Ok(result) => {
                    Span::current().record("attempts", attempt + 1);
                    return Ok(result);
                }
                Err(error) => {
                    // Check if error is retryable
                    if !error.is_retryable()
                        || error
                            .retry_after()
                            .is_some_and(|delay| delay > Duration::from_secs(60))
                    {
                        Span::current().record("attempts", attempt + 1);
                        final_failure(&error);
                        return Err(error);
                    }

                    // Don't sleep on the last attempt
                    if attempt == self.max_retries {
                        Span::current().record("attempts", attempt + 1);
                        last_error = Some(error);
                        break;
                    }

                    let sleep_duration = ExponentialBackoff::next(attempt)
                        .max(error.retry_after().unwrap_or_default());

                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        attempt = attempt + 1,
                        sleep_seconds = sleep_duration.as_secs_f64(),
                        "Retrying API request"
                    );
                    observability::retry(sleep_duration);
                    sleep(sleep_duration).await;
                    last_error = Some(error);
                }
            }
        }

        let error = last_error
            .unwrap_or_else(|| Error::unknown("Failed after retries without capturing error"));
        final_failure(&error);
        Err(error)
    }

    /// Process API response errors and convert to our Error type
    async fn process_error_response(response: Response) -> Error {
        let mut metadata = response_metadata(&response);
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => {
                return Error::from_http(
                    "Failed to read error response",
                    metadata,
                    Some(Arc::new(error)),
                );
            }
        };
        let parsed = serde_json::from_slice::<serde_json::Value>(&body).ok();
        if let Some(value) = parsed.as_ref() {
            metadata.error_type = value["error"]["type"].as_str().map(String::from);
            metadata.request_id = metadata
                .request_id
                .or_else(|| value["request_id"].as_str().map(String::from));
        }
        let message = parsed
            .as_ref()
            .and_then(|value| value["error"]["message"].as_str())
            .unwrap_or("HTTP error response");
        Error::from_http(message, metadata, None)
    }

    /// Remember the rate-limit headers of a received response.
    ///
    /// Responses without any `anthropic-ratelimit-*`, `anthropic-priority-*`, `anthropic-fast-*`
    /// or `retry-after` header (for example an error page from a proxy) leave the previously
    /// recorded state in place.
    fn record_rate_limits(&self, response: &Response) {
        let headers = response.headers();
        let has_rate_limit_headers = headers.keys().any(|name| {
            let name = name.as_str();
            name.starts_with("anthropic-ratelimit-")
                || name.starts_with("anthropic-priority-")
                || name.starts_with("anthropic-fast-")
                || name == header::RETRY_AFTER
        });
        if !has_rate_limit_headers {
            return;
        }
        let limits = RateLimits::from_headers(headers);
        if let Ok(mut slot) = self.last_rate_limits.lock() {
            *slot = Some(limits);
        }
    }

    /// Rate-limit state from the headers of the most recent HTTP response that carried them.
    ///
    /// Updated on every response the client receives, successful or not, including streaming
    /// responses, that carries at least one `anthropic-ratelimit-*`, `anthropic-priority-*`,
    /// `anthropic-fast-*` or `retry-after` header; responses without them leave the previous
    /// value in place.  Clones of this client share the same slot.  Returns `None` before the
    /// first such response.
    pub fn last_rate_limits(&self) -> Option<RateLimits> {
        self.last_rate_limits.lock().ok()?.clone()
    }

    fn map_request_error(&self, error: reqwest::Error) -> Error {
        map_reqwest_error(error, RequestPhase::Headers)
    }

    async fn decode_response<T: serde::de::DeserializeOwned>(response: Response) -> Result<T> {
        let metadata = response_metadata(&response);
        let body = response.bytes().await.map_err(|error| {
            map_reqwest_error(error, RequestPhase::Body).with_http_metadata(metadata.clone())
        })?;
        tracing::trace!(target: "claudius::wire", response = %String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).map_err(|error| {
            Error::from_http(
                format!("Failed to parse response ({} bytes): {error}", body.len()),
                metadata,
                Some(Arc::new(error)),
            )
        })
    }

    /// Execute a POST request with error handling
    async fn execute_post_request<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &impl serde::Serialize,
        headers: Option<HeaderMap>,
    ) -> Result<T> {
        let headers = headers.unwrap_or_else(|| self.default_headers());

        tracing::trace!(target: "claudius::wire", request = %serde_json::to_string(body).unwrap_or_default());
        let response = self
            .client
            .post(url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        self.record_rate_limits(&response);
        if !response.status().is_success() {
            return Err(Self::process_error_response(response).await);
        }

        Self::decode_response(response).await
    }

    /// Execute a POST request with a raw body and an explicit content type.
    async fn execute_post_body_request<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: Bytes,
        content_type: &str,
        headers: Option<HeaderMap>,
    ) -> Result<T> {
        let mut headers = headers.unwrap_or_else(|| self.default_headers());
        let content_type = HeaderValue::from_str(content_type).map_err(|error| {
            Error::validation(
                format!("Invalid content type: {error}"),
                Some("content_type".to_string()),
            )
        })?;
        headers.insert(header::CONTENT_TYPE, content_type);

        let response = self
            .client
            .post(url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        self.record_rate_limits(&response);
        if !response.status().is_success() {
            return Err(Self::process_error_response(response).await);
        }

        Self::decode_response(response).await
    }

    /// Execute a GET request with error handling
    async fn execute_get_request<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        query_params: Option<&[(String, String)]>,
        headers: Option<HeaderMap>,
    ) -> Result<T> {
        let headers = headers.unwrap_or_else(|| self.default_headers());
        let mut request = self.client.get(url).headers(headers);

        if let Some(params) = query_params {
            for (key, value) in params {
                request = request.query(&[(key, value)]);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        self.record_rate_limits(&response);
        if !response.status().is_success() {
            return Err(Self::process_error_response(response).await);
        }

        Self::decode_response(response).await
    }

    /// Execute an empty-body POST request with error handling.
    async fn execute_post_empty_request<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        headers: Option<HeaderMap>,
    ) -> Result<T> {
        let headers = headers.unwrap_or_else(|| self.default_headers());

        let response = self
            .client
            .post(url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        self.record_rate_limits(&response);
        if !response.status().is_success() {
            return Err(Self::process_error_response(response).await);
        }

        Self::decode_response(response).await
    }

    /// Execute a DELETE request with error handling.
    async fn execute_delete_request<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        headers: Option<HeaderMap>,
    ) -> Result<T> {
        let headers = headers.unwrap_or_else(|| self.default_headers());

        let response = self
            .client
            .delete(url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        self.record_rate_limits(&response);
        if !response.status().is_success() {
            return Err(Self::process_error_response(response).await);
        }

        Self::decode_response(response).await
    }

    /// Execute a streaming GET request with error handling.
    async fn execute_get_stream_request(
        &self,
        url: &str,
        headers: Option<HeaderMap>,
    ) -> Result<Response> {
        let headers = headers.unwrap_or_else(|| self.default_headers());

        let response = self
            .stream_client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        self.record_rate_limits(&response);
        if !response.status().is_success() {
            return Err(Self::process_error_response(response).await);
        }

        Ok(response)
    }

    /// Send a message to the API and get a non-streaming response.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "send",
        otel.kind = "client", gen_ai.request.model = %params.model, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty,
        cache_read_input_tokens = field::Empty, cache_creation_input_tokens = field::Empty,
        cost_micro_cents = field::Empty
    ))]
    pub async fn send(&self, mut params: MessageCreateParams) -> Result<Message> {
        let start = Instant::now();

        // Validate parameters first
        if let Err(err) = params.validate() {
            final_failure(&err);
            observability::request_failed(start.elapsed());
            return Err(err);
        }

        // Ensure stream is disabled
        params.stream = false;

        // Collect all betas: client defaults + per-request + auto-detected
        let mut auto_betas = create_params_betas(&params);
        if params.fallbacks.is_some() {
            auto_betas.push(SERVER_SIDE_FALLBACK_BETA);
        }
        if params.fallback_credit_token.is_some() {
            auto_betas.push(FALLBACK_CREDIT_BETA);
        }
        if params.requires_thinking_display_updates_beta() {
            auto_betas.push(THINKING_DISPLAY_UPDATES_BETA);
        }
        let all_betas = self.collect_betas(params.betas.as_deref(), &auto_betas);
        let headers = self.headers_with_betas(&all_betas);

        let result: Result<Message> = self
            .retry_with_backoff(|| async {
                let url = self.build_url("messages");
                self.execute_post_request(&url, &params, headers.clone())
                    .await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        if let Ok(message) = &result {
            record_usage(&params.model, &message.usage);
            count_usage(&params.model, &message.usage);
        }
        result
    }

    /// Send a message to the API and get a streaming response.
    ///
    /// Returns a stream of MessageStreamEvent objects that can be processed incrementally.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "stream",
        otel.kind = "client", gen_ai.request.model = %params.model, stream = true,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty,
        cache_read_input_tokens = field::Empty, cache_creation_input_tokens = field::Empty,
        cost_micro_cents = field::Empty
    ))]
    pub async fn stream(
        &self,
        params: &MessageCreateParams,
    ) -> Result<impl Stream<Item = Result<MessageStreamEvent>> + use<>> {
        let start = Instant::now();

        // Validate parameters first
        if let Err(err) = params.validate() {
            final_failure(&err);
            observability::request_failed(start.elapsed());
            return Err(err);
        }

        // Ensure stream is enabled
        if !params.stream {
            let err = Error::validation(
                "stream must be true for streaming requests",
                Some("stream".to_string()),
            );
            final_failure(&err);
            observability::request_failed(start.elapsed());
            return Err(err);
        }

        // Collect all betas: client defaults + per-request + auto-detected
        let mut auto_betas = create_params_betas(params);
        if params.fallbacks.is_some() {
            auto_betas.push(SERVER_SIDE_FALLBACK_BETA);
        }
        if params.fallback_credit_token.is_some() {
            auto_betas.push(FALLBACK_CREDIT_BETA);
        }
        if params.requires_thinking_display_updates_beta() {
            auto_betas.push(THINKING_DISPLAY_UPDATES_BETA);
        }
        let all_betas = self.collect_betas(params.betas.as_deref(), &auto_betas);

        let response = self
            .retry_with_backoff(|| async {
                let url = self.build_url("messages");

                let mut headers = self
                    .headers_with_betas(&all_betas)
                    .unwrap_or_else(|| self.default_headers());
                headers.insert(
                    header::ACCEPT,
                    HeaderValue::from_static("text/event-stream"),
                );

                tracing::trace!(target: "claudius::wire", request = ?params);
                let response = self
                    .stream_client
                    .post(&url)
                    .headers(headers)
                    .json(params)
                    .send()
                    .await
                    .map_err(|e| self.map_request_error(e))?;

                self.record_rate_limits(&response);
                if !response.status().is_success() {
                    return Err(Self::process_error_response(response).await);
                }

                Ok(response)
            })
            .await;

        observability::request_finished(start.elapsed(), &response);
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                return Err(err);
            }
        };

        // Get the byte stream from the response
        let request_id = response_metadata(&response).request_id;
        let stream = response.bytes_stream();
        let (events, _receiver) = AccumulatingStream::new(process_message_stream_sse(stream));
        let model = params.model.clone();
        let mut usage: Option<Usage> = None;
        let events = events.map(move |item| {
            let item = item.map_err(|error| error.with_request_id(request_id.clone()));
            if let Ok(event) = &item {
                match event {
                    MessageStreamEvent::MessageStart(start) => {
                        record_usage(&model, &start.message.usage);
                        usage = Some(start.message.usage.clone());
                    }
                    MessageStreamEvent::MessageDelta(delta) => {
                        let running = usage.get_or_insert_with(|| Usage::new(0, 0));
                        delta.usage.merge_into(running);
                        record_usage(&model, running);
                    }
                    MessageStreamEvent::MessageStop(_) => {
                        // Deltas carry cumulative counts and may repeat, so the counters
                        // take the merged usage once, here.  A stream that ends without
                        // `message_stop` is not counted.
                        if let Some(running) = &usage {
                            count_usage(&model, running);
                        }
                    }
                    _ => {}
                }
            }
            item
        });
        Ok(trace_stream(events, Span::current()))
    }

    /// Count tokens for a message.
    ///
    /// This method counts the number of tokens that would be used by a message with the given parameters.
    /// It's useful for estimating costs or making sure your messages fit within the model's context window.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "count_tokens",
        otel.kind = "client", gen_ai.request.model = %params.model, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn count_tokens(
        &self,
        params: MessageCountTokensParams,
    ) -> Result<MessageTokensCount> {
        let start = Instant::now();

        // Collect betas: client defaults + per-request + auto-detected
        let auto_betas = count_params_betas(&params);
        let all_betas = self.collect_betas(params.betas.as_deref(), &auto_betas);
        let headers = self.headers_with_betas(&all_betas);

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url("messages/count_tokens");
                self.execute_post_request(&url, &params, headers.clone())
                    .await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Create a Message Batch for asynchronous processing.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "create_message_batch",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn create_message_batch(
        &self,
        params: MessageBatchCreateParams,
    ) -> Result<MessageBatch> {
        let start = Instant::now();

        if let Err(err) = params.validate() {
            final_failure(&err);
            observability::request_failed(start.elapsed());
            return Err(err);
        }

        let mut request_betas = Vec::new();
        if let Some(betas) = &params.betas {
            request_betas.extend(betas.iter().cloned());
        }
        for request in &params.requests {
            if let Some(betas) = &request.params.betas {
                request_betas.extend(betas.iter().cloned());
            }
        }

        let auto_betas: Vec<&str> = params
            .requests
            .iter()
            .flat_map(|request| create_params_betas(&request.params))
            .collect();
        let all_betas = if request_betas.is_empty() {
            self.collect_betas(None, &auto_betas)
        } else {
            self.collect_betas(Some(&request_betas), &auto_betas)
        };
        let headers = self.headers_with_betas(&all_betas);

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url("messages/batches");
                self.execute_post_request(&url, &params, headers.clone())
                    .await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Retrieve a Message Batch by ID.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "get_message_batch",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn get_message_batch(&self, message_batch_id: &str) -> Result<MessageBatch> {
        let start = Instant::now();

        let all_betas = self.collect_betas(None, &[]);
        let headers = self.headers_with_betas(&all_betas);

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("messages/batches/{message_batch_id}"));
                self.execute_get_request(&url, None, headers.clone()).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Retrieve a Message Batch by ID.
    pub async fn retrieve_message_batch(&self, message_batch_id: &str) -> Result<MessageBatch> {
        self.get_message_batch(message_batch_id).await
    }

    /// List Message Batches in the current Workspace.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "list_message_batches",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn list_message_batches(
        &self,
        params: Option<MessageBatchListParams>,
    ) -> Result<MessageBatchListResponse> {
        let start = Instant::now();

        let request_betas = params.as_ref().and_then(|p| p.betas.as_deref());
        let all_betas = self.collect_betas(request_betas, &[]);
        let headers = self.headers_with_betas(&all_betas);

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url("messages/batches");
                let query_params = params.as_ref().map(|p| {
                    let mut params = Vec::new();
                    if let Some(ref after_id) = p.after_id {
                        params.push(("after_id".to_string(), after_id.clone()));
                    }
                    if let Some(ref before_id) = p.before_id {
                        params.push(("before_id".to_string(), before_id.clone()));
                    }
                    if let Some(limit) = p.limit {
                        params.push(("limit".to_string(), limit.to_string()));
                    }
                    params
                });

                self.execute_get_request(&url, query_params.as_deref(), headers.clone())
                    .await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Cancel a Message Batch that is currently processing.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "cancel_message_batch",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn cancel_message_batch(&self, message_batch_id: &str) -> Result<MessageBatch> {
        let start = Instant::now();

        let all_betas = self.collect_betas(None, &[]);
        let headers = self.headers_with_betas(&all_betas);

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("messages/batches/{message_batch_id}/cancel"));
                self.execute_post_empty_request(&url, headers.clone()).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Delete a Message Batch after processing has ended.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "delete_message_batch",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn delete_message_batch(
        &self,
        message_batch_id: &str,
    ) -> Result<DeletedMessageBatch> {
        let start = Instant::now();

        let all_betas = self.collect_betas(None, &[]);
        let headers = self.headers_with_betas(&all_betas);

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("messages/batches/{message_batch_id}"));
                self.execute_delete_request(&url, headers.clone()).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Stream results for an ended Message Batch as JSONL records.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "stream_message_batch_results",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = true,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn stream_message_batch_results(
        &self,
        message_batch_id: &str,
    ) -> Result<impl Stream<Item = Result<MessageBatchResult>> + use<>> {
        let start = Instant::now();

        let all_betas = self.collect_betas(None, &[]);
        let headers = self.headers_with_betas(&all_betas);

        let response = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("messages/batches/{message_batch_id}/results"));
                self.execute_get_stream_request(&url, headers.clone()).await
            })
            .await;

        observability::request_finished(start.elapsed(), &response);
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                return Err(err);
            }
        };

        let request_id = response_metadata(&response).request_id;
        let results = process_message_batch_result_jsonl(response.bytes_stream())
            .map(move |result| result.map_err(|error| error.with_request_id(request_id.clone())));
        Ok(trace_stream(results, Span::current()))
    }

    /// List available models from the API.
    ///
    /// Returns a paginated list of all available models. Use the parameters to control
    /// pagination and filter results.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "list_models",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn list_models(&self, params: Option<ModelListParams>) -> Result<ModelListResponse> {
        let start = Instant::now();

        // Collect betas: client defaults + per-request from ModelListParams
        let request_betas = params.as_ref().and_then(|p| p.betas.as_deref());
        let all_betas = self.collect_betas(request_betas, &[]);
        let headers = self.headers_with_betas(&all_betas);

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url("models");

                let query_params = params.as_ref().map(ModelListParams::query_pairs);

                self.execute_get_request(&url, query_params.as_deref(), headers.clone())
                    .await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Fetch the organization's Messages usage report.
    ///
    /// This is an Admin API endpoint: the client must be built with an Admin API key
    /// (`sk-ant-admin01-...`), which is sent in the same `x-api-key` header.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "usage_report",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn usage_report(&self, params: UsageReportParams) -> Result<UsageReport> {
        let start = Instant::now();

        let all_betas = self.collect_betas(None, &[]);
        let headers = self.headers_with_betas(&all_betas);
        let query = params.to_query();

        let result = self
            .retry_with_backoff(|| async {
                let url = format!(
                    "{}?{query}",
                    self.build_url("organizations/usage_report/messages")
                );
                self.execute_get_request(&url, None, headers.clone()).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Fetch the organization's daily cost report.
    ///
    /// This is an Admin API endpoint: the client must be built with an Admin API key
    /// (`sk-ant-admin01-...`), which is sent in the same `x-api-key` header.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "cost_report",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn cost_report(&self, params: CostReportParams) -> Result<CostReport> {
        let start = Instant::now();

        let all_betas = self.collect_betas(None, &[]);
        let headers = self.headers_with_betas(&all_betas);
        let query = params.to_query();

        let result = self
            .retry_with_backoff(|| async {
                let url = format!("{}?{query}", self.build_url("organizations/cost_report"));
                self.execute_get_request(&url, None, headers.clone()).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Retrieve information about a specific model.
    ///
    /// Returns detailed information about the specified model, including its
    /// ID, creation date, display name, and type.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "get_model",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn get_model(&self, model_id: &str) -> Result<ModelInfo> {
        let start = Instant::now();
        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("models/{}", model_id));
                self.execute_get_request(&url, None, None).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Upload a file to the Files API.
    ///
    /// The file is sent as `multipart/form-data`. `expires_in_seconds`, when given, must be
    /// between 3,600 (one hour) and 7,776,000 (90 days).
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "upload_file",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn upload_file(
        &self,
        filename: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        expires_in_seconds: Option<u64>,
    ) -> Result<FileInfo> {
        if let Some(seconds) = expires_in_seconds
            && !FILE_EXPIRY_SECONDS.contains(&seconds)
        {
            return Err(Error::validation(
                format!(
                    "expires_in_seconds must be between {} and {}, got {seconds}",
                    FILE_EXPIRY_SECONDS.start(),
                    FILE_EXPIRY_SECONDS.end()
                ),
                Some("expires_in_seconds".to_string()),
            ));
        }
        if [filename, mime_type]
            .iter()
            .any(|value| value.contains(['\r', '\n']))
        {
            return Err(Error::validation(
                "filename and mime_type must not contain line breaks",
                Some("filename".to_string()),
            ));
        }

        let start = Instant::now();

        let boundary = multipart_boundary();
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let body = Bytes::from(build_file_upload_body(
            &boundary,
            filename,
            mime_type,
            &bytes,
            expires_in_seconds,
        ));
        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url("files");
                self.execute_post_body_request(&url, body.clone(), &content_type, None)
                    .await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// List files stored through the Files API.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "list_files",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn list_files(&self, params: Option<FileListParams>) -> Result<FileList> {
        let start = Instant::now();

        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url("files");
                let query_params = params.as_ref().map(FileListParams::query_pairs);
                self.execute_get_request(&url, query_params.as_deref(), None)
                    .await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Fetch the metadata of a file.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "get_file",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn get_file(&self, file_id: &str) -> Result<FileInfo> {
        let start = Instant::now();
        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("files/{file_id}"));
                self.execute_get_request(&url, None, None).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Download the raw content of a file.
    ///
    /// Only files created by code execution or skills are downloadable.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "download_file",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn download_file(&self, file_id: &str) -> Result<Bytes> {
        let start = Instant::now();
        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("files/{file_id}/content"));
                let response = self.execute_get_stream_request(&url, None).await?;
                let metadata = response_metadata(&response);
                response.bytes().await.map_err(|error| {
                    map_reqwest_error(error, RequestPhase::Body).with_http_metadata(metadata)
                })
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }

    /// Delete a file.
    #[tracing::instrument(name = "claudius.request", skip_all, fields(
        operation = "delete_file",
        otel.kind = "client", gen_ai.request.model = field::Empty, stream = false,
        request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
        input_tokens = field::Empty, output_tokens = field::Empty, otel.status_code = field::Empty
    ))]
    pub async fn delete_file(&self, file_id: &str) -> Result<DeletedFile> {
        let start = Instant::now();
        let result = self
            .retry_with_backoff(|| async {
                let url = self.build_url(&format!("files/{file_id}"));
                self.execute_delete_request(&url, None).await
            })
            .await;

        observability::request_finished(start.elapsed(), &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        KnownModel, MessageBatchCreateParams, MessageBatchCreateRequest, MessageBatchListParams,
        MessageBatchResultVariant, MessageParam, Model,
    };
    use futures::StreamExt;
    use serde_json::Value;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn request_headers_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
    }

    async fn read_http_request_bytes(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let read = socket.read(&mut chunk).await.unwrap();
            assert!(
                read > 0,
                "client closed the connection before sending headers"
            );
            buffer.extend_from_slice(&chunk[..read]);
            if request_headers_end(&buffer).is_some() {
                break;
            }
        }

        let headers_end = request_headers_end(&buffer).unwrap();
        let headers = String::from_utf8_lossy(&buffer[..headers_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let mut parts = line.splitn(2, ':');
                let name = parts.next()?.trim();
                let value = parts.next()?.trim();
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);

        while buffer.len() - (headers_end + 4) < content_length {
            let read = socket.read(&mut chunk).await.unwrap();
            assert!(
                read > 0,
                "client closed the connection before sending the full body"
            );
            buffer.extend_from_slice(&chunk[..read]);
        }
        buffer
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) {
        read_http_request_bytes(socket).await;
    }

    fn split_http_request(request: &[u8]) -> (String, String) {
        let headers_end = request_headers_end(request).unwrap();
        let headers = String::from_utf8_lossy(&request[..headers_end]).to_string();
        let body = String::from_utf8_lossy(&request[headers_end + 4..]).to_string();
        (headers, body)
    }

    fn request_target(headers: &str) -> &str {
        headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap()
    }

    fn request_method(headers: &str) -> &str {
        headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .unwrap()
    }

    fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
        headers.lines().find_map(|line| {
            let mut parts = line.splitn(2, ':');
            let header_name = parts.next()?.trim();
            let value = parts.next()?.trim();
            header_name.eq_ignore_ascii_case(name).then_some(value)
        })
    }

    async fn write_json_response(socket: &mut tokio::net::TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
Content-Type: application/json\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    }

    async fn start_test_server<F, Fut>(handler: F) -> String
    where
        F: FnOnce(tokio::net::TcpStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            socket.set_nodelay(true).unwrap();
            handler(socket).await;
        });
        format!("http://{}", address)
    }

    #[tokio::test(start_paused = true)]
    async fn retry_logic_with_backoff() {
        let client = Anthropic {
            api_key: "test".to_string(),
            client: ReqwestClient::new(),
            stream_client: ReqwestClient::new(),
            base_url: "http://localhost".to_string(),
            timeout: Duration::from_secs(1),
            max_retries: 2,
            cached_headers: Arc::new(HeaderMap::new()),
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        };

        let attempt_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = attempt_counter.clone();

        let result = client
            .retry_with_backoff(|| {
                let counter = counter_clone.clone();
                async move {
                    let attempt = counter.fetch_add(1, Ordering::SeqCst);
                    match attempt {
                        0 | 1 => Err(Error::rate_limit("Rate limited", Some(1))),
                        _ => Ok("success".to_string()),
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(attempt_counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_logic_with_non_retryable_error() {
        let client = Anthropic {
            api_key: "test".to_string(),
            client: ReqwestClient::new(),
            stream_client: ReqwestClient::new(),
            base_url: "http://localhost".to_string(),
            timeout: Duration::from_secs(1),
            max_retries: 2,
            cached_headers: Arc::new(HeaderMap::new()),
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        };

        let attempt_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = attempt_counter.clone();

        let result: Result<String> = client
            .retry_with_backoff(|| {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(Error::authentication("Invalid API key"))
                }
            })
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_authentication());
        // Should only attempt once since authentication errors are not retryable
        assert_eq!(attempt_counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_logic_max_retries_exceeded() {
        let client = Anthropic {
            api_key: "test".to_string(),
            client: ReqwestClient::new(),
            stream_client: ReqwestClient::new(),
            base_url: "http://localhost".to_string(),
            timeout: Duration::from_secs(1),
            max_retries: 2,
            cached_headers: Arc::new(HeaderMap::new()),
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        };

        let attempt_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = attempt_counter.clone();

        let result: Result<String> = client
            .retry_with_backoff(|| {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(Error::rate_limit("Always rate limited", Some(1)))
                }
            })
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_rate_limit());
        // Should attempt max_retries + 1 times (3 total: initial + 2 retries)
        assert_eq!(attempt_counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn error_529_is_retryable() {
        // Test that 529 errors are properly mapped to rate_limit and are retryable
        let client = Anthropic {
            api_key: "test".to_string(),
            client: ReqwestClient::new(),
            stream_client: ReqwestClient::new(),
            base_url: "http://localhost".to_string(),
            timeout: Duration::from_secs(1),
            max_retries: 2,
            cached_headers: Arc::new(HeaderMap::new()),
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        };

        let attempt_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = attempt_counter.clone();

        let result = client
            .retry_with_backoff(|| {
                let counter = counter_clone.clone();
                async move {
                    let attempt = counter.fetch_add(1, Ordering::SeqCst);
                    match attempt {
                        0 | 1 => {
                            // Simulate a 529 overloaded error
                            Err(Error::api(
                                529,
                                Some("overloaded_error".to_string()),
                                "Overloaded".to_string(),
                                None,
                            ))
                        }
                        _ => Ok("success".to_string()),
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        // Should retry: initial attempt + 2 retries = 3 total
        assert_eq!(attempt_counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn error_529_mapped_correctly() {
        // Test that a 529 API error is correctly identified as retryable
        let error = Error::api(
            529,
            Some("overloaded_error".to_string()),
            "Overloaded".to_string(),
            None,
        );
        assert!(error.is_retryable());

        // Test that rate_limit error (which 529 now maps to) is also retryable
        let rate_limit_error = Error::rate_limit("Overloaded", Some(5));
        assert!(rate_limit_error.is_retryable());
    }

    #[test]
    fn resolve_api_key_regular_value() {
        let result = Anthropic::resolve_api_key("sk-test-key-123");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "sk-test-key-123");
    }

    #[test]
    fn resolve_api_key_file_url_absolute() {
        let test_dir = std::env::temp_dir().join(format!("claudius_test_{}", std::process::id()));
        std::fs::create_dir_all(&test_dir).unwrap();
        let test_file = test_dir.join("test_api_key.txt");
        std::fs::write(&test_file, "sk-test-from-file-123\n").unwrap();

        let file_url = format!("file://{}", test_file.display());
        let result = Anthropic::resolve_api_key(&file_url);

        std::fs::remove_dir_all(&test_dir).unwrap();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "sk-test-from-file-123");
    }

    #[test]
    fn resolve_api_key_file_url_relative() {
        let test_file = "test_relative_key.txt";
        std::fs::write(test_file, "sk-relative-key-456\n").unwrap();

        let file_url = format!("file://{}", test_file);
        let result = Anthropic::resolve_api_key(&file_url);

        std::fs::remove_file(test_file).unwrap();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "sk-relative-key-456");
    }

    #[test]
    fn resolve_api_key_file_url_nonexistent() {
        let result = Anthropic::resolve_api_key("file:///nonexistent/path/to/key.txt");
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(matches!(error, Error::Io { .. }));
        assert!(std::error::Error::source(&error).is_some());
        assert!(format!("{}", error).contains("Failed to read API key file"));
    }

    #[test]
    fn resolve_api_key_file_url_with_whitespace() {
        let test_file = "test_whitespace_key.txt";
        std::fs::write(test_file, "  sk-whitespace-key-789  \n  ").unwrap();

        let file_url = format!("file://{}", test_file);
        let result = Anthropic::resolve_api_key(&file_url);

        std::fs::remove_file(test_file).unwrap();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "sk-whitespace-key-789");
    }

    #[test]
    fn client_builder_methods() {
        let client = Anthropic::new(Some("test_key".to_string())).unwrap();

        // Test builder pattern methods
        let configured_client = client
            .with_base_url("https://custom.api.com".to_string())
            .unwrap()
            .with_max_retries(5);

        assert_eq!(configured_client.base_url, "https://custom.api.com");
        assert_eq!(configured_client.max_retries, 5);
    }

    #[test]
    fn build_url_default_base() {
        let client = Anthropic::new(Some("test_key".to_string())).unwrap();
        // Default base URL: https://api.anthropic.com
        assert_eq!(
            client.build_url("messages"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            client.build_url("messages/count_tokens"),
            "https://api.anthropic.com/v1/messages/count_tokens"
        );
        assert_eq!(
            client.build_url("models"),
            "https://api.anthropic.com/v1/models"
        );
    }

    #[test]
    fn build_url_custom_base_without_trailing_slash() {
        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://api.minimax.io/anthropic".to_string())
            .unwrap();
        assert_eq!(
            client.build_url("messages"),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn build_url_custom_base_with_trailing_slash() {
        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://api.minimax.io/anthropic/".to_string())
            .unwrap();
        assert_eq!(
            client.build_url("messages"),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn default_headers_omit_authorization_for_default_base_url() {
        let client = Anthropic::new(Some("test_key".to_string())).unwrap();
        let headers = client.default_headers();
        assert!(headers.contains_key("x-api-key"));
        assert!(!headers.contains_key("authorization"));
    }

    #[test]
    fn default_headers_include_authorization_for_baseten() {
        // Baseten requires `Authorization: Bearer ${API_KEY}`; the env-based
        // base URL is simulated by constructing headers directly.
        let headers =
            Anthropic::build_default_headers("test_key", "https://inference.baseten.co").unwrap();
        assert!(headers.contains_key("x-api-key"));
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert_eq!(auth, "Bearer test_key");
    }

    #[test]
    fn default_headers_authorization_is_trailing_slash_insensitive() {
        let headers =
            Anthropic::build_default_headers("test_key", "https://inference.baseten.co/").unwrap();
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer test_key"
        );
    }

    #[test]
    fn with_base_url_to_baseten_rebuilds_headers_with_authorization() {
        let client = Anthropic::new(Some("test_key".to_string())).unwrap();
        assert!(!client.default_headers().contains_key("authorization"));

        let client = client
            .with_base_url("https://inference.baseten.co".to_string())
            .unwrap();
        let headers = client.default_headers();
        assert!(headers.contains_key("x-api-key"));
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer test_key"
        );
    }

    #[test]
    fn with_base_url_away_from_baseten_drops_authorization() {
        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://inference.baseten.co".to_string())
            .unwrap();
        assert!(client.default_headers().contains_key("authorization"));

        let client = client
            .with_base_url("https://api.anthropic.com".to_string())
            .unwrap();
        assert!(!client.default_headers().contains_key("authorization"));
    }

    #[test]
    fn build_url_minimax_china() {
        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://api.minimaxi.com/anthropic".to_string())
            .unwrap();
        assert_eq!(
            client.build_url("messages"),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );
        assert_eq!(
            client.build_url(&format!("models/{}", "claude-3-opus")),
            "https://api.minimaxi.com/anthropic/v1/models/claude-3-opus"
        );
    }

    #[test]
    fn client_timeout_configuration() {
        let client = Anthropic::new(Some("test_key".to_string())).unwrap();
        let timeout = Duration::from_secs(30);

        let configured_client = client.with_timeout(timeout).unwrap();
        assert_eq!(configured_client.timeout, timeout);
    }

    #[test]
    fn client_cached_headers_performance() {
        let client = Anthropic::new(Some("test_key".to_string())).unwrap();

        // Test that headers are cached and cloning is cheap
        let headers1 = client.default_headers();
        let headers2 = client.default_headers();

        assert_eq!(headers1.len(), headers2.len());
        assert!(headers1.contains_key("x-api-key"));
        assert!(headers1.contains_key("anthropic-version"));
        assert!(headers1.contains_key("content-type"));
    }

    #[test]
    fn request_error_mapping() {
        let client = Anthropic::new(Some("test_key".to_string())).unwrap();

        // Test different types of reqwest errors are mapped correctly
        // Note: These are unit tests for the mapping logic, not integration tests
        let _timeout = Duration::from_secs(30);
        assert_eq!(client.timeout, DEFAULT_TIMEOUT); // Should use default initially
    }

    #[tokio::test]
    async fn concurrent_retry_safety() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::spawn;

        let client = Anthropic {
            api_key: "test".to_string(),
            client: ReqwestClient::new(),
            stream_client: ReqwestClient::new(),
            base_url: "http://localhost".to_string(),
            timeout: Duration::from_secs(1),
            max_retries: 1,
            cached_headers: Arc::new(HeaderMap::new()),
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        };

        let attempt_counter = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        // Spawn multiple concurrent retry operations
        for _ in 0..3 {
            let client_clone = client.clone();
            let counter_clone = attempt_counter.clone();

            let handle = spawn(async move {
                client_clone
                    .retry_with_backoff(|| {
                        let counter = counter_clone.clone();
                        async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            Ok::<String, Error>("success".to_string())
                        }
                    })
                    .await
            });
            handles.push(handle);
        }

        // Wait for all operations to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }

        // Verify all operations executed
        assert_eq!(attempt_counter.load(Ordering::SeqCst), 3);
    }

    fn test_client() -> Anthropic {
        Anthropic {
            api_key: "test".to_string(),
            client: ReqwestClient::new(),
            stream_client: ReqwestClient::new(),
            base_url: "http://localhost".to_string(),
            timeout: Duration::from_secs(1),
            max_retries: 0,
            cached_headers: Arc::new(HeaderMap::new()),
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        }
    }

    fn sample_create_params() -> MessageCreateParams {
        MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
    }

    #[test]
    fn create_params_betas_none_by_default() {
        assert!(create_params_betas(&sample_create_params()).is_empty());
    }

    #[test]
    fn create_params_betas_context_management() {
        use crate::types::ContextEdit;
        let params = sample_create_params()
            .with_context_management(ContextManagement::new(vec![ContextEdit::clear_tool_uses()]));
        assert_eq!(create_params_betas(&params), vec![CONTEXT_MANAGEMENT_BETA]);
    }

    #[test]
    fn create_params_betas_compact_edit_adds_both() {
        use crate::types::ContextEdit;
        let params = sample_create_params()
            .with_context_management(ContextManagement::new(vec![ContextEdit::compact()]));
        assert_eq!(
            create_params_betas(&params),
            vec![CONTEXT_MANAGEMENT_BETA, COMPACT_EDIT_BETA]
        );
    }

    #[test]
    fn create_params_betas_compaction() {
        let params = sample_create_params().with_compaction(CompactionRequest::new());
        assert_eq!(create_params_betas(&params), vec![COMPACTION_BETA]);
    }

    #[test]
    fn create_params_betas_mcp_servers() {
        let params = sample_create_params()
            .with_mcp_servers(vec![McpServer::new("a", "https://mcp.example.com")]);
        assert_eq!(create_params_betas(&params), vec![MCP_CLIENT_BETA]);
    }

    #[test]
    fn create_params_betas_task_budget() {
        let params = sample_create_params().with_output_config(
            OutputConfig::new().with_task_budget(crate::types::TaskBudget::new(64_000)),
        );
        assert_eq!(create_params_betas(&params), vec![TASK_BUDGETS_BETA]);
    }

    #[test]
    fn structured_outputs_never_adds_a_beta() {
        use crate::types::{OutputFormat, ToolParam, ToolUnionParam};
        let tool = ToolParam::new(
            "get_weather".to_string(),
            serde_json::json!({"type": "object", "properties": {}}),
        )
        .with_strict(true);
        let params = sample_create_params()
            .with_output_config(OutputConfig::new().with_format(OutputFormat::json_schema(
                serde_json::json!({"type": "object", "additionalProperties": false}),
            )))
            .with_tools(vec![ToolUnionParam::CustomTool(tool)]);
        assert!(create_params_betas(&params).is_empty());
    }

    #[test]
    fn count_params_betas_mirror_create_params() {
        use crate::types::{ContextEdit, MessageParam};
        let params = MessageCountTokensParams::new(
            vec![MessageParam::new_with_string(
                "Hi".to_string(),
                crate::types::MessageRole::User,
            )],
            Model::Known(KnownModel::ClaudeSonnet55),
        )
        .with_context_management(ContextManagement::new(vec![ContextEdit::compact()]))
        .with_mcp_servers(vec![McpServer::new("a", "https://mcp.example.com")])
        .with_output_config(
            OutputConfig::new().with_task_budget(crate::types::TaskBudget::new(20_000)),
        );
        assert_eq!(
            count_params_betas(&params),
            vec![
                CONTEXT_MANAGEMENT_BETA,
                COMPACT_EDIT_BETA,
                MCP_CLIENT_BETA,
                TASK_BUDGETS_BETA
            ]
        );
    }

    #[test]
    fn collect_betas_empty() {
        let client = test_client();
        let result = client.collect_betas(None, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn collect_betas_client_defaults_only() {
        let client = test_client().with_default_betas(["alpha", "bravo"]);
        let result = client.collect_betas(None, &[]);
        assert_eq!(result, vec!["alpha", "bravo"]);
    }

    #[test]
    fn collect_betas_request_only() {
        let client = test_client();
        let request = vec!["compact-2026-01-12".to_string()];
        let result = client.collect_betas(Some(&request), &[]);
        assert_eq!(result, vec!["compact-2026-01-12"]);
    }

    #[test]
    fn collect_betas_auto_only() {
        let client = test_client();
        let result = client.collect_betas(None, &[CONTEXT_MANAGEMENT_BETA]);
        assert_eq!(result, vec![CONTEXT_MANAGEMENT_BETA]);
    }

    #[test]
    fn collect_betas_merges_all_sources() {
        let client = test_client().with_default_betas(["default-beta"]);
        let request = vec!["request-beta".to_string()];
        let result = client.collect_betas(Some(&request), &["auto-beta"]);
        assert_eq!(result, vec!["default-beta", "request-beta", "auto-beta"]);
    }

    #[test]
    fn collect_betas_deduplicates() {
        let client = test_client().with_default_betas(["shared-beta", "default-only"]);
        let request = vec!["shared-beta".to_string(), "request-only".to_string()];
        let result = client.collect_betas(Some(&request), &["shared-beta"]);
        assert_eq!(result, vec!["shared-beta", "default-only", "request-only"]);
    }

    #[test]
    fn headers_with_betas_none_when_empty() {
        let client = test_client();
        assert!(client.headers_with_betas(&[]).is_none());
    }

    #[test]
    fn headers_with_betas_joins_with_comma() {
        let client = test_client();
        let betas = vec!["alpha".to_string(), "bravo".to_string()];
        let headers = client.headers_with_betas(&betas).unwrap();
        assert_eq!(
            headers.get("anthropic-beta").unwrap().to_str().unwrap(),
            "alpha, bravo"
        );
    }

    #[test]
    fn with_default_betas_builder() {
        let client = test_client().with_default_betas(["a", "b", "c"]);
        assert_eq!(client.default_betas, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn create_message_batch_posts_expected_body_and_betas() {
        let response_body = r#"{
            "id": "msgbatch_123",
            "type": "message_batch",
            "processing_status": "in_progress",
            "request_counts": {
                "processing": 1,
                "succeeded": 0,
                "errored": 0,
                "canceled": 0,
                "expired": 0
            },
            "ended_at": null,
            "created_at": "2024-09-24T18:37:24Z",
            "expires_at": "2024-09-25T18:37:24Z",
            "cancel_initiated_at": null,
            "results_url": null
        }"#;

        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            assert_eq!(request_method(&headers), "POST");
            assert_eq!(request_target(&headers), "/v1/messages/batches");
            assert_eq!(
                header_value(&headers, "anthropic-beta"),
                Some("default-beta, batch-beta, request-beta")
            );

            let body_json: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body_json["requests"][0]["custom_id"], "request-1");
            assert_eq!(
                body_json["requests"][0]["params"]["model"],
                "claude-haiku-5-5"
            );
            assert!(
                body_json["requests"][0]["params"].get("stream").is_none(),
                "stream must not be serialized inside batch params"
            );
            assert!(body_json.get("betas").is_none());

            write_json_response(&mut socket, response_body).await;
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_default_betas(["default-beta"]);
        let request_params = MessageCreateParams::new(
            16,
            vec![MessageParam::user("ping")],
            Model::Known(KnownModel::ClaudeHaiku55),
        )
        .with_beta("request-beta");
        let params = MessageBatchCreateParams::new(vec![MessageBatchCreateRequest::new(
            "request-1",
            request_params,
        )])
        .with_beta("batch-beta");

        let batch = client.create_message_batch(params).await.unwrap();
        assert_eq!(batch.id, "msgbatch_123");
    }

    #[tokio::test]
    async fn get_message_batch_uses_retrieve_endpoint() {
        let response_body = r#"{
            "id": "msgbatch_123",
            "type": "message_batch",
            "processing_status": "ended",
            "request_counts": {
                "processing": 0,
                "succeeded": 1,
                "errored": 0,
                "canceled": 0,
                "expired": 0
            },
            "ended_at": "2024-09-24T18:39:24Z",
            "created_at": "2024-09-24T18:37:24Z",
            "expires_at": "2024-09-25T18:37:24Z",
            "cancel_initiated_at": null,
            "results_url": "https://api.anthropic.com/results"
        }"#;

        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            assert_eq!(request_method(&headers), "GET");
            assert_eq!(
                request_target(&headers),
                "/v1/messages/batches/msgbatch_123"
            );
            assert!(body.is_empty());
            write_json_response(&mut socket, response_body).await;
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let batch = client.get_message_batch("msgbatch_123").await.unwrap();
        assert_eq!(batch.id, "msgbatch_123");
        assert_eq!(batch.request_counts.succeeded, 1);
    }

    #[tokio::test]
    async fn list_message_batches_sends_pagination_query_and_beta() {
        let response_body = r#"{
            "data": [],
            "has_more": false,
            "first_id": null,
            "last_id": null
        }"#;

        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            assert_eq!(request_method(&headers), "GET");
            assert_eq!(
                request_target(&headers),
                "/v1/messages/batches?after_id=msgbatch_a&limit=20"
            );
            assert_eq!(header_value(&headers, "anthropic-beta"), Some("list-beta"));
            assert!(body.is_empty());
            write_json_response(&mut socket, response_body).await;
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let params = MessageBatchListParams::new()
            .with_after_id("msgbatch_a")
            .with_limit(20)
            .with_beta("list-beta");
        let page = client.list_message_batches(Some(params)).await.unwrap();
        assert!(page.data.is_empty());
        assert!(!page.has_more);
    }

    #[tokio::test]
    async fn last_rate_limits_reflects_response_headers() {
        let body = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;
        let base_url = start_test_server(move |mut socket| async move {
            let _ = read_http_request_bytes(&mut socket).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
Content-Type: application/json\r\n\
anthropic-ratelimit-requests-limit: 50\r\n\
anthropic-ratelimit-requests-remaining: 49\r\n\
anthropic-ratelimit-requests-reset: 2026-10-19T12:00:00Z\r\n\
request-id: req_abc\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        assert_eq!(client.last_rate_limits(), None);
        let params = MessageCreateParams::simple("hi", KnownModel::ClaudeHaiku55);
        client.send(params).await.unwrap();

        let limits = client.last_rate_limits().unwrap();
        let requests = limits.requests.unwrap();
        assert_eq!((requests.limit, requests.remaining), (50, 49));
        assert!(requests.reset.is_some());
        assert_eq!(limits.request_id.as_deref(), Some("req_abc"));
        assert_eq!(limits.tokens, None);
    }

    #[tokio::test]
    async fn response_without_rate_limit_headers_keeps_last_rate_limits() {
        let body = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;
        let with_headers = start_test_server(move |mut socket| async move {
            let _ = read_http_request_bytes(&mut socket).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
Content-Type: application/json\r\n\
anthropic-ratelimit-requests-limit: 50\r\n\
anthropic-ratelimit-requests-remaining: 49\r\n\
request-id: req_abc\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        })
        .await;
        let without_headers = start_test_server(move |mut socket| async move {
            let _ = read_http_request_bytes(&mut socket).await;
            write_json_response(&mut socket, body).await;
        })
        .await;
        let fast_only = start_test_server(move |mut socket| async move {
            let _ = read_http_request_bytes(&mut socket).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
Content-Type: application/json\r\n\
anthropic-fast-input-tokens-limit: 1000\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(with_headers)
            .unwrap();
        let params = MessageCreateParams::simple("hi", KnownModel::ClaudeHaiku55);
        client.send(params.clone()).await.unwrap();
        let recorded = client.last_rate_limits().unwrap();

        let other = client.clone().with_base_url(without_headers).unwrap();
        other.send(params.clone()).await.unwrap();
        assert_eq!(client.last_rate_limits(), Some(recorded.clone()));
        assert_eq!(
            client.last_rate_limits().unwrap().request_id.as_deref(),
            Some("req_abc")
        );

        let fast = client.clone().with_base_url(fast_only).unwrap();
        fast.send(params).await.unwrap();
        let updated = client.last_rate_limits().unwrap();
        assert_ne!(updated, recorded);
        assert_eq!(updated.requests, None);
        assert_eq!(updated.request_id, None);
    }

    #[tokio::test]
    async fn cancel_message_batch_posts_empty_body() {
        let response_body = r#"{
            "id": "msgbatch_123",
            "type": "message_batch",
            "processing_status": "canceling",
            "request_counts": {
                "processing": 1,
                "succeeded": 0,
                "errored": 0,
                "canceled": 0,
                "expired": 0
            },
            "ended_at": null,
            "created_at": "2024-09-24T18:37:24Z",
            "expires_at": "2024-09-25T18:37:24Z",
            "cancel_initiated_at": "2024-09-24T18:39:03Z",
            "results_url": null
        }"#;

        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            assert_eq!(request_method(&headers), "POST");
            assert_eq!(
                request_target(&headers),
                "/v1/messages/batches/msgbatch_123/cancel"
            );
            assert!(body.is_empty());
            write_json_response(&mut socket, response_body).await;
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let batch = client.cancel_message_batch("msgbatch_123").await.unwrap();
        assert_eq!(batch.id, "msgbatch_123");
    }

    #[tokio::test]
    async fn delete_message_batch_uses_delete_endpoint() {
        let response_body = r#"{
            "id": "msgbatch_123",
            "type": "message_batch_deleted"
        }"#;

        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            assert_eq!(request_method(&headers), "DELETE");
            assert_eq!(
                request_target(&headers),
                "/v1/messages/batches/msgbatch_123"
            );
            assert!(body.is_empty());
            write_json_response(&mut socket, response_body).await;
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let deleted = client.delete_message_batch("msgbatch_123").await.unwrap();
        assert_eq!(deleted.r#type, "message_batch_deleted");
    }

    #[tokio::test]
    async fn stream_message_batch_results_preserves_jsonl_order_without_trailing_newline() {
        let results_body = concat!(
            r#"{"custom_id":"second","result":{"type":"expired"}}"#,
            "\n",
            r#"{"custom_id":"first","result":{"type":"succeeded","message":{"id":"msg_123","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}}"#
        );

        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            assert_eq!(request_method(&headers), "GET");
            assert_eq!(
                request_target(&headers),
                "/v1/messages/batches/msgbatch_123/results"
            );
            assert!(body.is_empty());

            let response_headers = format!(
                "HTTP/1.1 200 OK\r\n\
Content-Type: application/jsonl\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\r\n",
                results_body.len()
            );
            socket.write_all(response_headers.as_bytes()).await.unwrap();
            let split_at = results_body.find('\n').unwrap() + 1;
            socket
                .write_all(&results_body.as_bytes()[..split_at])
                .await
                .unwrap();
            socket
                .write_all(&results_body.as_bytes()[split_at..])
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let stream = client
            .stream_message_batch_results("msgbatch_123")
            .await
            .unwrap();
        let mut stream = std::pin::pin!(stream);

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.custom_id, "second");
        assert!(matches!(first.result, MessageBatchResultVariant::Expired));

        let second = stream.next().await.unwrap().unwrap();
        assert_eq!(second.custom_id, "first");
        assert!(matches!(
            second.result,
            MessageBatchResultVariant::Succeeded { .. }
        ));

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn streaming_timeout_is_inactivity_based() {
        let base_url = start_test_server(|mut socket| async move {
            read_http_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
Content-Type: text/event-stream\r\n\
Cache-Control: no-cache\r\n\
Connection: close\r\n\r\n",
                )
                .await
                .unwrap();

            for chunk in [
                b"event: ping\n".as_slice(),
                b"data: {}\n".as_slice(),
                b"\n".as_slice(),
            ] {
                socket.write_all(chunk).await.unwrap();
                socket.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_timeout(Duration::from_millis(75))
            .unwrap();
        let params = MessageCreateParams::new_streaming(
            16,
            vec![MessageParam::user("ping")],
            Model::Known(KnownModel::ClaudeHaiku55),
        );

        let mut stream = std::pin::pin!(client.stream(&params).await.unwrap());
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, MessageStreamEvent::Ping);
    }

    #[tokio::test]
    async fn streaming_stall_reports_timeout_error() {
        let base_url = start_test_server(|mut socket| async move {
            read_http_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
Content-Type: text/event-stream\r\n\
Cache-Control: no-cache\r\n\
Connection: close\r\n\r\n\
event: ping\n",
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(120)).await;
            socket.write_all(b"data: {}\n\n").await.unwrap();
            socket.flush().await.unwrap();
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_timeout(Duration::from_millis(50))
            .unwrap();
        let params = MessageCreateParams::new_streaming(
            16,
            vec![MessageParam::user("ping")],
            Model::Known(KnownModel::ClaudeHaiku55),
        );

        let mut stream = std::pin::pin!(client.stream(&params).await.unwrap());
        let err = stream.next().await.unwrap().unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }));
        assert!(err.to_string().contains("operation timed out"));
    }

    #[tokio::test]
    async fn non_streaming_body_stall_reports_timeout_error() {
        let base_url = start_test_server(|mut socket| async move {
            read_http_request(&mut socket).await;
            let body_prefix = b"{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-haiku-5-5\",\"content\":[{\"type\":\"text\",\"text\":\"hel";
            let body_suffix = b"lo\"}],\"stop_reason\":\"end_turn\",\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}";
            let headers = format!(
                "HTTP/1.1 200 OK\r\n\
Content-Type: application/json\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\r\n",
                body_prefix.len() + body_suffix.len()
            );
            socket
                .write_all(headers.as_bytes())
                .await
                .unwrap();
            socket.write_all(body_prefix).await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(120)).await;
            socket.write_all(body_suffix).await.unwrap();
            socket.flush().await.unwrap();
            socket.shutdown().await.unwrap();
        })
        .await;

        let client = Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_timeout(Duration::from_millis(50))
            .unwrap()
            .with_max_retries(0);
        let params = MessageCreateParams::new(
            16,
            vec![MessageParam::user("ping")],
            Model::Known(KnownModel::ClaudeHaiku55),
        );

        let err = client.send(params).await.unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }), "{err:?}");
    }

    fn file_info_json() -> &'static str {
        r#"{"id":"file_01","type":"file","filename":"notes.txt","mime_type":"text/plain","size_bytes":5,"created_at":"2026-10-19T00:00:00Z","downloadable":false}"#
    }

    fn files_client(base_url: String) -> Anthropic {
        Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
    }

    #[test]
    fn create_params_betas_mcp_toolset() {
        let params = sample_create_params().with_tools(vec![ToolUnionParam::new_mcp_toolset("a")]);
        assert_eq!(create_params_betas(&params), vec![MCP_CLIENT_BETA]);
    }

    #[test]
    fn file_upload_body_framing() {
        let body = build_file_upload_body("BOUND", "a\"b.txt", "text/plain", b"hello", Some(3_600));
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "--BOUND\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"a\\\"b.txt\"\r\n\
             Content-Type: text/plain\r\n\r\nhello\r\n\
             --BOUND\r\n\
             Content-Disposition: form-data; name=\"expires_in_seconds\"\r\n\r\n3600\r\n\
             --BOUND--\r\n"
        );
        assert_ne!(multipart_boundary(), multipart_boundary());
    }

    #[tokio::test]
    async fn upload_file_sends_multipart_body() {
        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            assert_eq!(request_method(&headers), "POST");
            assert_eq!(request_target(&headers), "/v1/files");
            let content_type = header_value(&headers, "content-type").unwrap();
            let boundary = content_type
                .strip_prefix("multipart/form-data; boundary=")
                .expect("multipart content type");
            assert!(header_value(&headers, "anthropic-beta").is_none());
            assert_eq!(
                body,
                format!(
                    "--{boundary}\r\n\
                     Content-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\n\
                     Content-Type: text/plain\r\n\r\nhello\r\n\
                     --{boundary}\r\n\
                     Content-Disposition: form-data; name=\"expires_in_seconds\"\r\n\r\n7200\r\n\
                     --{boundary}--\r\n"
                )
            );
            write_json_response(&mut socket, file_info_json()).await;
        })
        .await;

        let info = files_client(base_url)
            .upload_file("notes.txt", "text/plain", b"hello".to_vec(), Some(7_200))
            .await
            .unwrap();
        assert_eq!(info.id, "file_01");
        assert_eq!(info.size_bytes, 5);
        assert!(!info.downloadable);
    }

    #[tokio::test]
    async fn upload_file_rejects_bad_expiry_and_line_breaks() {
        let client = test_client();
        let too_short = client
            .upload_file("a.txt", "text/plain", Vec::new(), Some(60))
            .await
            .unwrap_err();
        assert!(too_short.is_validation());
        let too_long = client
            .upload_file("a.txt", "text/plain", Vec::new(), Some(7_776_001))
            .await
            .unwrap_err();
        assert!(too_long.is_validation());
        let bad_name = client
            .upload_file("a\r\nb.txt", "text/plain", Vec::new(), None)
            .await
            .unwrap_err();
        assert!(bad_name.is_validation());
    }

    #[tokio::test]
    async fn list_files_sends_query() {
        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, _) = split_http_request(&request);
            assert_eq!(request_method(&headers), "GET");
            assert_eq!(request_target(&headers), "/v1/files?limit=2&page=p2");
            let body = format!(
                r#"{{"data":[{}],"has_more":true,"next_page":"p3"}}"#,
                file_info_json()
            );
            write_json_response(&mut socket, &body).await;
        })
        .await;

        let list = files_client(base_url)
            .list_files(Some(FileListParams::new().with_limit(2).with_page("p2")))
            .await
            .unwrap();
        assert_eq!(list.data.len(), 1);
        assert_eq!(list.next_page.as_deref(), Some("p3"));
        assert_eq!(list.has_more, Some(true));
    }

    #[tokio::test]
    async fn get_file_uses_file_endpoint() {
        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, _) = split_http_request(&request);
            assert_eq!(request_method(&headers), "GET");
            assert_eq!(request_target(&headers), "/v1/files/file_01");
            write_json_response(&mut socket, file_info_json()).await;
        })
        .await;

        let info = files_client(base_url).get_file("file_01").await.unwrap();
        assert_eq!(info.filename, "notes.txt");
    }

    #[tokio::test]
    async fn delete_file_uses_delete_endpoint() {
        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, _) = split_http_request(&request);
            assert_eq!(request_method(&headers), "DELETE");
            assert_eq!(request_target(&headers), "/v1/files/file_01");
            write_json_response(&mut socket, r#"{"id":"file_01","type":"file_deleted"}"#).await;
        })
        .await;

        let deleted = files_client(base_url).delete_file("file_01").await.unwrap();
        assert_eq!(deleted.id, "file_01");
        assert_eq!(deleted.r#type, "file_deleted");
    }

    #[tokio::test]
    async fn download_file_returns_raw_bytes() {
        let base_url = start_test_server(move |mut socket| async move {
            let request = read_http_request_bytes(&mut socket).await;
            let (headers, _) = split_http_request(&request);
            assert_eq!(request_method(&headers), "GET");
            assert_eq!(request_target(&headers), "/v1/files/file_01/content");
            let payload: &[u8] = &[0, 159, 146, 150, 255];
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(payload).await.unwrap();
            socket.shutdown().await.unwrap();
        })
        .await;

        let bytes = files_client(base_url)
            .download_file("file_01")
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), &[0, 159, 146, 150, 255]);
    }
}
