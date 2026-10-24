//! The [`Anthropic`] client: request building, retries, telemetry and every API endpoint.
use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::env;
use std::fmt;
use std::fs;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client as ReqwestClient, Method, Response, header, redirect};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::{Instrument, Span, field};
use url::Url;

use crate::backoff::{self, RetryPolicy};
use crate::base_url;
use crate::error::{
    Error, ErrorCode, ErrorKind, RequestPhase, Result, Service, describe_json_error,
    map_reqwest_error,
};
use crate::http;
use crate::observability;
use crate::sse::process_message_stream_sse;
use crate::types::{
    CompactionRequest, ContextManagement, CostReport, CostReportParams, DeletedFile,
    DeletedMessageBatch, ErrorType, FileInfo, FileList, FileListParams, McpServer, Message,
    MessageBatch, MessageBatchCreateParams, MessageBatchListParams, MessageBatchListResponse,
    MessageBatchResult, MessageCountTokensParams, MessageCreateParams, MessageStreamEvent,
    MessageTokensCount, Model, ModelInfo, ModelListParams, ModelListResponse, OutputConfig,
    RateLimits, ToolUnionParam, Usage, UsageReport, UsageReportParams, retry_after_from_headers,
};

fn final_failure(error: &Error) {
    let span = Span::current();
    span.record("otel.status_code", "ERROR");
    if let Some(id) = error.request_id() {
        span.record("request_id", id);
    }
    if let Some(status) = error.status() {
        span.record("http.status_code", status);
    }
    tracing::error!(
        target: "carpa::client",
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

/// Emit one `carpa::usage` event for a completed request.
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
        target: "carpa::usage",
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
const ANTHROPIC_API_VERSION: &str = "2026-10-21";
/// Total deadline for non-streaming requests and inactivity timeout for stream reads.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
/// A longer `retry-after` gives up at once instead of waiting.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";
const COMPACT_EDIT_BETA: &str = "compact-2026-01-12";
const COMPACTION_BETA: &str = "compact-2026-09-04";
const MCP_CLIENT_BETA: &str = "mcp-client-2025-11-20";
const TASK_BUDGETS_BETA: &str = "task-budgets-2026-03-13";
const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const FALLBACK_CREDIT_BETA: &str = "fallback-credit-2026-07-01";
const THINKING_DISPLAY_UPDATES_BETA: &str = "thinking-display-updates-2026-08-18";

/// Allowed lifetime of an uploaded file: one hour to ninety days.
const FILE_EXPIRY_SECONDS: RangeInclusive<u64> = 3_600..=7_776_000;

/// A multipart boundary built from a process-wide counter and the current time.
fn multipart_boundary() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("carpa-boundary-{nanos:x}-{count:x}")
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
fn feature_betas(
    context_management: Option<&ContextManagement>,
    compaction: Option<&CompactionRequest>,
    mcp_servers: Option<&[McpServer]>,
    tools: Option<&[ToolUnionParam]>,
    output_config: Option<&OutputConfig>,
) -> Vec<&'static str> {
    let mut betas = Vec::new();
    if let Some(context_management) = context_management {
        betas.push(CONTEXT_MANAGEMENT_BETA);
        if context_management.has_compact_edit() {
            betas.push(COMPACT_EDIT_BETA);
        }
    }
    if compaction.is_some() {
        betas.push(COMPACTION_BETA);
    }
    let has_mcp_toolset = tools.is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| matches!(tool, ToolUnionParam::McpToolset(_)))
    });
    if mcp_servers.is_some() || has_mcp_toolset {
        betas.push(MCP_CLIENT_BETA);
    }
    if output_config.is_some_and(|config| config.task_budget.is_some()) {
        betas.push(TASK_BUDGETS_BETA);
    }
    betas
}

/// Betas implied by the features a Messages request uses, alone or inside a batch.
fn message_betas(params: &MessageCreateParams) -> Vec<&'static str> {
    let mut betas = feature_betas(
        params.context_management.as_ref(),
        params.compaction.as_ref(),
        params.mcp_servers.as_deref(),
        params.tools.as_deref(),
        params.output_config.as_ref(),
    );
    if params.fallbacks.is_some() {
        betas.push(SERVER_SIDE_FALLBACK_BETA);
    }
    if params.fallback_credit_token.is_some() {
        betas.push(FALLBACK_CREDIT_BETA);
    }
    if params.requires_thinking_display_updates_beta() {
        betas.push(THINKING_DISPLAY_UPDATES_BETA);
    }
    betas
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

/// Checks a base URL with the shared https rule (see [`base_url::checked_url`]) and normalizes
/// it: removes trailing slashes and one final `v1` path segment, which [`Anthropic::build_url`]
/// adds back. `setting` names where the URL came from in the validation error.
fn normalized_base_url(setting: &str, base_url: &str) -> Result<String> {
    let mut url = base_url::checked_url("Anthropic", setting, base_url)?;
    // The path starts with `/`, so a trailing `/v1` is always a whole final segment.
    let path = url.path().trim_end_matches('/');
    let path = path
        .strip_suffix("/v1")
        .unwrap_or(path)
        .trim_end_matches('/')
        .to_string();
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

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
        Error::new(
            ErrorKind::Decode,
            "Invalid UTF-8 in message batch results JSONL",
        )
        .with_source(e)
    })?;

    // serde's message can quote the line, so neither it nor the error itself is kept.
    serde_json::from_str::<MessageBatchResult>(text).map_err(|error| {
        Error::new(
            ErrorKind::Decode,
            format!(
                "Failed to parse message batch results JSONL line ({} bytes): {}",
                line.len(),
                describe_json_error(&error)
            ),
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

/// Decodes the JSONL body of batch results. An error item carries
/// [`Error::events_delivered`]: the number of results yielded before it.
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
                            Err(Error::new(
                                ErrorKind::Decode,
                                format!(
                                    "Message batch results JSONL line exceeded maximum size of {} bytes",
                                    MAX_MESSAGE_BATCH_RESULT_LINE_BYTES
                                ),
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
    .scan(0_u64, |delivered, result| {
        futures::future::ready(Some(match result {
            Ok(result) => {
                *delivered += 1;
                Ok(result)
            }
            Err(error) => Err(error.with_events_delivered(*delivered)),
        }))
    })
}

/// The diagnostics of a received response.
struct ResponseMetadata {
    status: u16,
    request_id: Option<String>,
    retry_after: Option<Duration>,
}

fn response_metadata(response: &Response) -> ResponseMetadata {
    let span = Span::current();
    let status = response.status().as_u16();
    span.record("http.status_code", status);
    let request_id = http::header_str(response.headers(), "request-id").map(String::from);
    if let Some(id) = &request_id {
        span.record("request_id", id.as_str());
    }
    ResponseMetadata {
        status,
        request_id,
        retry_after: retry_after_from_headers(response.headers()),
    }
}

/// Reads one environment variable: `Ok(None)` when it is unset, an error when it is not Unicode.
fn env_value(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(Error::validation(
            format!("Configuration variable {name} must contain Unicode text"),
            Some(name.into()),
        )),
    }
}

/// Returns the first of `names` that `get` reads as nonblank text, trimmed, with its name.
fn configured_value<'a>(
    get: &impl Fn(&str) -> Result<Option<String>>,
    names: &[&'a str],
) -> Result<Option<(&'a str, String)>> {
    for name in names {
        if let Some(value) = get(name)?
            && !value.trim().is_empty()
        {
            return Ok(Some((name, value.trim().to_string())));
        }
    }
    Ok(None)
}

#[derive(Clone, Copy)]
enum ResponseMode {
    Buffered,
    Streaming,
}

/// How an HTTP client bounds its responses.
#[derive(Clone, Copy)]
enum Deadline {
    /// A total deadline for the whole request.
    Total,
    /// An inactivity timeout between reads of the body.
    Read(Duration),
}

/// Client for the Anthropic API.
#[derive(Clone)]
pub struct Anthropic {
    api_key: String,
    client: ReqwestClient,
    stream_client: ReqwestClient,
    base_url: String,
    timeout: Duration,
    /// Streaming inactivity timeout; `None` follows `timeout`.
    read_timeout: Option<Duration>,
    max_retries: usize,
    /// Headers sent on every request, built once from the API key and base URL.
    cached_headers: HeaderMap,
    /// Beta feature headers included on every request.
    default_betas: Vec<String>,
    /// Rate-limit headers from the most recent response, shared by clones.
    last_rate_limits: Arc<Mutex<Option<RateLimits>>>,
}

impl fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Anthropic")
            .field("api_key", &"[REDACTED]")
            .field("cached_headers", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .field("read_timeout", &self.stream_read_timeout())
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl Anthropic {
    /// Builds an HTTP client for `base_url`, which decides whether proxies are used (see
    /// [`base_url::with_proxy_rule`]). The connect timeout follows `timeout` alone.
    fn build_http_client(
        timeout: Duration,
        deadline: Deadline,
        base_url: &str,
    ) -> Result<ReqwestClient> {
        let builder = ReqwestClient::builder()
            .redirect(redirect::Policy::none())
            .connect_timeout(timeout.min(Duration::from_secs(60)))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60));
        let builder = match deadline {
            Deadline::Read(read_timeout) => builder.read_timeout(read_timeout),
            Deadline::Total => builder.timeout(timeout),
        };
        base_url::with_proxy_rule(builder, base_url)
            .build()
            .map_err(|e| Error::validation("Failed to build HTTP client", None).with_source(e))
    }

    fn stream_read_timeout(&self) -> Duration {
        self.read_timeout.unwrap_or(self.timeout)
    }

    /// Rebuilds both HTTP clients from the current timeouts and base URL.
    fn rebuild_http_clients(&mut self) -> Result<()> {
        self.client = Self::build_http_client(self.timeout, Deadline::Total, &self.base_url)?;
        self.stream_client = Self::build_http_client(
            self.timeout,
            Deadline::Read(self.stream_read_timeout()),
            &self.base_url,
        )?;
        Ok(())
    }

    /// Resolve an API key value, handling file:// URLs
    fn resolve_api_key(key_value: &str) -> Result<String> {
        let key_value = key_value.trim();
        let Some(path) = key_value.strip_prefix("file://") else {
            return Ok(key_value.to_string());
        };
        fs::read_to_string(path)
            .map(|content| content.trim().to_string())
            .map_err(|error| {
                Error::new(
                    ErrorKind::Io {
                        path: Some(path.into()),
                    },
                    format!("Failed to read API key file '{path}'"),
                )
                .with_source(error)
            })
    }

    /// Create a new Anthropic client.
    ///
    /// The API key can be provided directly or read from the CARPA_API_KEY or ANTHROPIC_API_KEY
    /// environment variables. If an environment variable value starts with "file://", it will be
    /// treated as a file path and the API key will be read from that file.
    ///
    /// The base URL is resolved from the CARPA_BASE_URL or ANTHROPIC_BASE_URL environment
    /// variables, in that order. If neither is set, the default Anthropic API URL is used. A
    /// configured base URL follows the rule of [`Anthropic::with_base_url`]; a URL that breaks it
    /// is a validation error naming the variable.
    pub fn try_new(api_key: Option<String>) -> Result<Self> {
        Self::try_new_with(api_key, env_value)
    }

    /// [`Anthropic::try_new`] with the configuration read through `get` instead of the process
    /// environment.
    fn try_new_with(
        api_key: Option<String>,
        get: impl Fn(&str) -> Result<Option<String>>,
    ) -> Result<Self> {
        let key = match api_key {
            Some(key) => key,
            None => configured_value(&get, &["CARPA_API_KEY", "ANTHROPIC_API_KEY"])?
                .map(|(_, key)| key)
                .ok_or_else(|| {
                    Error::validation(
                        "No nonempty API key configured in CARPA_API_KEY or ANTHROPIC_API_KEY",
                        Some("api_key".to_string()),
                    )
                })?,
        };
        let api_key = Self::resolve_api_key(&key)?;

        // Resolve base URL from environment variables, defaulting to the API URL.
        // This is resolved before building headers because some providers
        // (see `BASE_URLS_REQUIRING_AUTH_HEADER`) require an `Authorization`
        // header derived from the base URL.
        let configured = configured_value(&get, &["CARPA_BASE_URL", "ANTHROPIC_BASE_URL"])?;
        let base_url = match configured {
            Some((name, url)) => normalized_base_url(name, &url)?,
            None => DEFAULT_API_URL.to_string(),
        };

        // Pre-build headers for performance
        let cached_headers = Self::build_default_headers(&api_key, &base_url)?;
        let timeout = DEFAULT_TIMEOUT;

        Ok(Self {
            api_key,
            client: Self::build_http_client(timeout, Deadline::Total, &base_url)?,
            stream_client: Self::build_http_client(timeout, Deadline::Read(timeout), &base_url)?,
            base_url,
            timeout,
            read_timeout: None,
            max_retries: 3,
            cached_headers,
            default_betas: Vec::new(),
            last_rate_limits: Arc::new(Mutex::new(None)),
        })
    }

    /// Set a custom base URL for this client.
    ///
    /// This method allows you to specify a different API endpoint for the client.
    /// The `/v1` suffix is optional: one final `v1` path segment and any trailing slashes are
    /// removed, and `/v1/` is added back when constructing request URLs.
    ///
    /// The URL must use https; plain http is accepted only for a loopback host (`localhost`,
    /// `127.x`, `[::1]`), so the API key never travels in clear. Credentials, a query or a
    /// fragment in the URL are rejected. A URL that breaks these rules is a validation error
    /// naming `base_url`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use carpa::Anthropic;
    /// // For Anthropic's API (default)
    /// let client = Anthropic::try_new(Some("api-key".to_string()))?
    ///     .with_base_url("https://api.anthropic.com".to_string())?;
    ///
    /// // For Minimax (international)
    /// let client = Anthropic::try_new(Some("api-key".to_string()))?
    ///     .with_base_url("https://api.minimax.io/anthropic".to_string())?;
    ///
    /// // For Minimax (China)
    /// let client = Anthropic::try_new(Some("api-key".to_string()))?
    ///     .with_base_url("https://api.minimaxi.com/anthropic".to_string())?;
    /// # Ok::<(), carpa::Error>(())
    /// ```
    pub fn with_base_url(mut self, base_url: String) -> Result<Self> {
        // The set of default headers is derived from the base URL: providers
        // listed in `BASE_URLS_REQUIRING_AUTH_HEADER` get an `Authorization`
        // header. Rebuild the cached headers when switching to or from such a
        // provider so every subsequent request carries the right auth.
        let base_url = normalized_base_url("base_url", &base_url)?;
        self.cached_headers = Self::build_default_headers(&self.api_key, &base_url)?;
        // A loopback base URL bypasses proxies, so the HTTP clients depend on it too.
        self.base_url = base_url;
        self.rebuild_http_clients()?;
        Ok(self)
    }

    /// Set a custom timeout for this client.
    ///
    /// Sets the total deadline for non-streaming requests and the connect timeout (capped at 60
    /// seconds). Streaming reads use it as their inactivity timeout unless
    /// [`Self::with_read_timeout`] set one.
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.timeout = timeout;
        self.rebuild_http_clients()?;
        Ok(self)
    }

    /// Set the streaming read timeout for this client.
    ///
    /// A stream fails when no bytes arrive for this long. Defaults to 600 seconds, or the value
    /// given to [`Self::with_timeout`]. Later calls to [`Self::with_timeout`] keep it.
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Result<Self> {
        self.read_timeout = Some(read_timeout);
        self.rebuild_http_clients()?;
        Ok(self)
    }

    /// Set the maximum number of retries for this client.
    ///
    /// This method allows you to specify how many times to retry failed requests.
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Get the API key being used by this client.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Set default beta feature headers included on every request.
    ///
    /// These are merged with any per-request betas and auto-detected betas
    /// (like `context-management-2025-06-27`). Duplicates are removed.
    #[must_use]
    pub fn with_default_betas(
        mut self,
        betas: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.default_betas = betas.into_iter().map(Into::into).collect();
        self
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
                Error::validation("Invalid API key format", Some("api_key".to_string()))
                    .with_source(e)
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
                        "Invalid Authorization header value",
                        Some("api_key".to_string()),
                    )
                    .with_source(e)
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

    /// A copy of the headers sent on every request.
    #[must_use]
    pub fn default_headers(&self) -> HeaderMap {
        self.cached_headers.clone()
    }

    /// Build a full endpoint URL from the base URL and the endpoint's path segments.
    ///
    /// The stored base URL has no trailing slash and no `/v1` suffix (see
    /// [`normalized_base_url`]), so `/v1` is always inserted between it and the
    /// segments. Each segment is percent-encoded on its own, so an ID such as `../models`
    /// stays one segment.
    ///
    /// # Examples
    ///
    /// - Base: `https://api.anthropic.com`, segments: `["messages"]` → `https://api.anthropic.com/v1/messages`
    /// - Base: `https://api.minimax.io/anthropic`, segments: `["messages"]` → `https://api.minimax.io/anthropic/v1/messages`
    /// - Base: `https://example.com`, segments: `["files", "a/b"]` → `https://example.com/v1/files/a%2Fb`
    fn build_url(&self, segments: &[&str]) -> Result<Url> {
        let invalid = || Error::validation("Invalid base URL", Some("base_url".to_string()));
        let mut url = Url::parse(&format!("{}/v1", self.base_url))
            .map_err(|error| invalid().with_source(error))?;
        url.path_segments_mut()
            .map_err(|()| invalid())?
            .extend(segments);
        Ok(url)
    }

    /// Collect all beta strings from client defaults, per-request, and auto-detected sources.
    ///
    /// Returns a deduplicated, ordered list.
    fn collect_betas(&self, request_betas: Option<&[String]>, auto_betas: &[&str]) -> Vec<String> {
        let mut seen = HashSet::new();
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
    /// Returns `None` if there are no betas, avoiding an unnecessary header clone. A beta that
    /// cannot be part of a header value is a validation error naming `betas`.
    fn headers_with_betas(&self, betas: &[String]) -> Result<Option<HeaderMap>> {
        if betas.is_empty() {
            return Ok(None);
        }
        let mut headers = self.default_headers();
        let header_value = HeaderValue::from_str(&betas.join(", ")).map_err(|error| {
            Error::validation(
                "betas must be printable ASCII to go in the anthropic-beta header",
                Some("betas".to_string()),
            )
            .with_source(error)
        })?;
        headers.insert("anthropic-beta", header_value);
        Ok(Some(headers))
    }

    /// Sends `call` under the shared retry loop and reads each successful response with `read`.
    ///
    /// The call runs in its `carpa.request` span, and its outcome, a failure found before
    /// sending included, is reported once: the `request_completed` event and, on failure, the
    /// error log. Only GET requests resend an [`ErrorKind::Interrupted`] failure, since the server
    /// has already answered a POST or DELETE.
    async fn execute<T, Read, Fut>(&self, call: Call<'_>, read: Read) -> Result<T>
    where
        Read: Fn(Response) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let span = tracing::info_span!(
            target: "carpa::client",
            "carpa.request",
            operation = call.operation,
            otel.kind = "client", gen_ai.request.model = call.model.map(field::display),
            stream = call.stream,
            request_id = field::Empty, http.status_code = field::Empty, attempts = field::Empty,
            input_tokens = field::Empty, output_tokens = field::Empty,
            otel.status_code = field::Empty, cache_read_input_tokens = field::Empty,
            cache_creation_input_tokens = field::Empty, cost_micro_cents = field::Empty
        );
        async {
            let start = Instant::now();
            let result = self.send_with_retries(call, read).await;
            if let Err(error) = &result {
                final_failure(error);
            }
            observability::request_completed(start.elapsed(), result.is_ok());
            result
        }
        .instrument(span)
        .await
    }

    /// The body of [`Anthropic::execute`] without its reporting.
    async fn send_with_retries<T, Read, Fut>(&self, call: Call<'_>, read: Read) -> Result<T>
    where
        Read: Fn(Response) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let Call {
            method,
            path,
            query,
            body,
            content_type,
            accept,
            mode,
            betas,
            auto_betas,
            check,
            ..
        } = call;
        check?;
        let betas = self.collect_betas(Some(&betas), &auto_betas);
        let mut headers = self
            .headers_with_betas(&betas)?
            .unwrap_or_else(|| self.default_headers());
        if let Some(accept) = accept {
            headers.insert(header::ACCEPT, HeaderValue::from_static(accept));
        }
        match &content_type {
            Some(content_type) => {
                let value = HeaderValue::from_str(content_type).map_err(|error| {
                    Error::validation("Invalid content type", Some("content_type".to_string()))
                        .with_source(error)
                })?;
                headers.insert(header::CONTENT_TYPE, value);
            }
            None => {
                if let Some(body) = &body {
                    tracing::trace!(target: "carpa::wire", request = %String::from_utf8_lossy(body));
                }
            }
        }
        let mut url = self.build_url(path)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let client = match mode {
            ResponseMode::Buffered => &self.client,
            ResponseMode::Streaming => &self.stream_client,
        };
        let policy = RetryPolicy {
            max_retries: self.max_retries,
            max_retry_after: MAX_RETRY_AFTER,
            budget: None,
            resend_timeouts: false,
            resend_interrupted: method == Method::GET,
        };
        let (method, url, headers, body, read) = (&method, &url, &headers, &body, &read);
        let retried = backoff::retry(
            &policy,
            |_, _| async move {
                let mut request = client
                    .request(method.clone(), url.clone())
                    .headers(headers.clone());
                if let Some(body) = body {
                    request = request.body(body.clone());
                }
                let response = request
                    .send()
                    .await
                    .map_err(|error| map_reqwest_error(error, RequestPhase::Headers))?;
                self.record_rate_limits(&response);
                if !response.status().is_success() {
                    return Err(Self::process_error_response(response).await);
                }
                read(response).await
            },
            |error, attempt, delay| {
                tracing::warn!(
                    target: "carpa::client",
                    error = %error,
                    attempt = attempt + 1,
                    backoff_seconds = delay.as_secs_f64(),
                    "retry"
                );
            },
        )
        .await;
        Span::current().record("attempts", retried.attempts);
        retried.result
    }

    /// Process API response errors and convert to our Error type
    async fn process_error_response(response: Response) -> Error {
        let metadata = response_metadata(&response);
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => {
                return Error::http_status(
                    Service::Anthropic,
                    metadata.status,
                    Some("Failed to read error response".to_string()),
                    None,
                    None,
                    metadata.request_id,
                    metadata.retry_after,
                )
                .with_source(error);
            }
        };
        let parsed: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
        let field = |pointer: &str| {
            parsed
                .as_ref()
                .and_then(|value| value.pointer(pointer))
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        };
        Error::http_status(
            Service::Anthropic,
            metadata.status,
            field("/error/message"),
            parsed
                .as_ref()
                .and_then(|value| ErrorType::deserialize(&value["error"]["type"]).ok()),
            field("/error/details/error_code").map(ErrorCode::Anthropic),
            metadata.request_id.or_else(|| field("/request_id")),
            metadata.retry_after,
        )
    }

    /// Remember the rate-limit headers of a received response.
    ///
    /// Responses without any `anthropic-ratelimit-*`, `anthropic-priority-*`, `anthropic-fast-*`
    /// or `retry-after` header (for example an error page from a proxy) leave the previously
    /// recorded state in place.
    #[expect(
        clippy::expect_used,
        reason = "a poisoned lock means a thread panicked mid-update, so its state is not read"
    )]
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
        *self
            .last_rate_limits
            .lock()
            .expect("rate-limit lock poisoned") = Some(limits);
    }

    /// Rate-limit state from the headers of the most recent HTTP response that carried them.
    ///
    /// Updated on every response the client receives, successful or not, including streaming
    /// responses, that carries at least one `anthropic-ratelimit-*`, `anthropic-priority-*`,
    /// `anthropic-fast-*` or `retry-after` header; responses without them leave the previous
    /// value in place.  Clones of this client share the same slot.  Returns `None` before the
    /// first such response.
    #[expect(
        clippy::expect_used,
        reason = "a poisoned lock means a thread panicked mid-update, so its state is not read"
    )]
    #[must_use]
    pub fn last_rate_limits(&self) -> Option<RateLimits> {
        self.last_rate_limits
            .lock()
            .expect("rate-limit lock poisoned")
            .clone()
    }

    /// Reads the whole body of a successful response.
    async fn read_bytes(response: Response) -> Result<Bytes> {
        let request_id = response_metadata(&response).request_id;
        Self::body_bytes(response, request_id).await
    }

    /// Reads the whole body of a response whose metadata was already read.
    async fn body_bytes(response: Response, request_id: Option<String>) -> Result<Bytes> {
        response.bytes().await.map_err(|error| {
            map_reqwest_error(error, RequestPhase::Body).with_request_id(request_id)
        })
    }

    async fn decode_response<T: DeserializeOwned>(response: Response) -> Result<T> {
        let request_id = response_metadata(&response).request_id;
        let body = Self::body_bytes(response, request_id.clone()).await?;
        tracing::trace!(target: "carpa::wire", response = %String::from_utf8_lossy(&body));
        // serde's message can quote the body, so neither it nor the error itself is kept.
        serde_json::from_slice(&body).map_err(|error| {
            Error::new(
                ErrorKind::Decode,
                format!(
                    "Failed to parse response ({} bytes): {}",
                    body.len(),
                    describe_json_error(&error)
                ),
            )
            .with_request_id(request_id)
        })
    }

    /// Send a message to the API and get a non-streaming response.
    pub async fn send(&self, params: &MessageCreateParams) -> Result<Message> {
        // Send with stream disabled; copy the params only when the caller enabled it.
        let params = if params.stream {
            let mut buffered = params.clone();
            buffered.stream = false;
            Cow::Owned(buffered)
        } else {
            Cow::Borrowed(params)
        };
        let model = &params.model;
        self.execute(Call::message("send", &params), |response| async move {
            let message: Message = Self::decode_response(response).await?;
            record_usage(model, &message.usage);
            count_usage(model, &message.usage);
            Ok(message)
        })
        .await
    }

    /// Send a message to the API and get a streaming response.
    ///
    /// Returns a stream of MessageStreamEvent objects that can be processed incrementally. A
    /// stream that ends before `message_stop` yields an [`ErrorKind::Interrupted`] error last.
    pub async fn stream(
        &self,
        params: &MessageCreateParams,
    ) -> Result<impl Stream<Item = Result<MessageStreamEvent>> + use<>> {
        let stream_check = if params.stream {
            Ok(())
        } else {
            Err(Error::validation(
                "stream must be true for streaming requests",
                Some("stream".to_string()),
            ))
        };
        let call = Call {
            stream: true,
            mode: ResponseMode::Streaming,
            accept: Some("text/event-stream"),
            ..Call::message("stream", params).and_check(stream_check)
        };
        self.execute(call, |response| {
            let model = params.model.clone();
            async move {
                let request_id = response_metadata(&response).request_id;
                let mut usage: Option<Usage> = None;
                let events = process_message_stream_sse(response.bytes_stream())
                    .map(move |item| {
                        item.map_err(|error| error.with_request_id(request_id.clone()))
                    })
                    .inspect(move |item| {
                        let Ok(event) = item else { return };
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
                                // Deltas carry cumulative counts and may repeat, so the
                                // counters take the merged usage once, here.  A stream that
                                // ends without `message_stop` is not counted.
                                if let Some(running) = &usage {
                                    count_usage(&model, running);
                                }
                            }
                            _ => {}
                        }
                    });
                Ok(trace_stream(events, Span::current()))
            }
        })
        .await
    }

    /// Count tokens for a message.
    ///
    /// This method counts the number of tokens that would be used by a message with the given parameters.
    /// It's useful for estimating costs or making sure your messages fit within the model's context window.
    pub async fn count_tokens(
        &self,
        params: MessageCountTokensParams,
    ) -> Result<MessageTokensCount> {
        let call = Call {
            model: Some(&params.model),
            betas: params.betas.clone().unwrap_or_default(),
            auto_betas: feature_betas(
                params.context_management.as_ref(),
                None,
                params.mcp_servers.as_deref(),
                params.tools.as_deref(),
                params.output_config.as_ref(),
            ),
            check: params.check_known_values(),
            ..Call::new("count_tokens", Method::POST, &["messages", "count_tokens"])
        };
        self.execute(call.with_json(&params), Self::decode_response)
            .await
    }

    /// Create a Message Batch for asynchronous processing.
    ///
    /// The batch carries the betas of every request in it, including the ones implied by the
    /// features each request uses.
    pub async fn create_message_batch(
        &self,
        params: MessageBatchCreateParams,
    ) -> Result<MessageBatch> {
        let requests = params.requests.iter().map(|request| &request.params);
        let call = Call {
            betas: params
                .betas
                .iter()
                .chain(requests.clone().filter_map(|params| params.betas.as_ref()))
                .flatten()
                .cloned()
                .collect(),
            auto_betas: requests.flat_map(message_betas).collect(),
            check: params.validate(),
            ..Call::new(
                "create_message_batch",
                Method::POST,
                &["messages", "batches"],
            )
        };
        self.execute(call.with_json(&params), Self::decode_response)
            .await
    }

    /// Retrieve a Message Batch by ID.
    pub async fn get_message_batch(&self, message_batch_id: &str) -> Result<MessageBatch> {
        let path = ["messages", "batches", message_batch_id];
        let call = Call::new("get_message_batch", Method::GET, &path)
            .and_check(path_id("message_batch_id", message_batch_id));
        self.execute(call, Self::decode_response).await
    }

    /// List Message Batches in the current Workspace.
    pub async fn list_message_batches(
        &self,
        params: Option<MessageBatchListParams>,
    ) -> Result<MessageBatchListResponse> {
        let call = Call {
            betas: params
                .as_ref()
                .and_then(|params| params.betas.clone())
                .unwrap_or_default(),
            query: params
                .as_ref()
                .map(MessageBatchListParams::query_pairs)
                .unwrap_or_default(),
            ..Call::new(
                "list_message_batches",
                Method::GET,
                &["messages", "batches"],
            )
        };
        self.execute(call, Self::decode_response).await
    }

    /// Cancel a Message Batch that is currently processing.
    pub async fn cancel_message_batch(&self, message_batch_id: &str) -> Result<MessageBatch> {
        let path = ["messages", "batches", message_batch_id, "cancel"];
        let call = Call::new("cancel_message_batch", Method::POST, &path)
            .and_check(path_id("message_batch_id", message_batch_id));
        self.execute(call, Self::decode_response).await
    }

    /// Delete a Message Batch after processing has ended.
    pub async fn delete_message_batch(
        &self,
        message_batch_id: &str,
    ) -> Result<DeletedMessageBatch> {
        let path = ["messages", "batches", message_batch_id];
        let call = Call::new("delete_message_batch", Method::DELETE, &path)
            .and_check(path_id("message_batch_id", message_batch_id));
        self.execute(call, Self::decode_response).await
    }

    /// Stream results for an ended Message Batch as JSONL records.
    pub async fn stream_message_batch_results(
        &self,
        message_batch_id: &str,
    ) -> Result<impl Stream<Item = Result<MessageBatchResult>> + use<>> {
        let path = ["messages", "batches", message_batch_id, "results"];
        let call = Call {
            stream: true,
            mode: ResponseMode::Streaming,
            ..Call::new("stream_message_batch_results", Method::GET, &path)
                .and_check(path_id("message_batch_id", message_batch_id))
        };
        self.execute(call, |response| async move {
            let request_id = response_metadata(&response).request_id;
            let results =
                process_message_batch_result_jsonl(response.bytes_stream()).map(move |result| {
                    result.map_err(|error| error.with_request_id(request_id.clone()))
                });
            Ok(trace_stream(results, Span::current()))
        })
        .await
    }

    /// List available models from the API.
    ///
    /// Returns a paginated list of all available models. Use the parameters to control
    /// pagination and filter results.
    pub async fn list_models(&self, params: Option<ModelListParams>) -> Result<ModelListResponse> {
        let call = Call {
            betas: params
                .as_ref()
                .and_then(|params| params.betas.clone())
                .unwrap_or_default(),
            query: params
                .as_ref()
                .map(ModelListParams::query_pairs)
                .unwrap_or_default(),
            check: params
                .as_ref()
                .map_or(Ok(()), ModelListParams::check_known_values),
            ..Call::new("list_models", Method::GET, &["models"])
        };
        self.execute(call, Self::decode_response).await
    }

    /// Fetch the organization's Messages usage report.
    ///
    /// This is an Admin API endpoint: the client must be built with an Admin API key
    /// (`sk-ant-admin01-...`), which is sent in the same `x-api-key` header.
    pub async fn usage_report(&self, params: UsageReportParams) -> Result<UsageReport> {
        let path = ["organizations", "usage_report", "messages"];
        let call = Call {
            query: params.query_pairs(),
            check: params.check_known_values(),
            ..Call::new("usage_report", Method::GET, &path)
        };
        self.execute(call, Self::decode_response).await
    }

    /// Fetch the organization's daily cost report.
    ///
    /// This is an Admin API endpoint: the client must be built with an Admin API key
    /// (`sk-ant-admin01-...`), which is sent in the same `x-api-key` header.
    pub async fn cost_report(&self, params: CostReportParams) -> Result<CostReport> {
        let call = Call {
            query: params.query_pairs(),
            ..Call::new(
                "cost_report",
                Method::GET,
                &["organizations", "cost_report"],
            )
        };
        self.execute(call, Self::decode_response).await
    }

    /// Retrieve information about a specific model.
    ///
    /// Returns detailed information about the specified model, including its
    /// ID, creation date, display name, and type.
    pub async fn get_model(&self, model_id: &str) -> Result<ModelInfo> {
        let path = ["models", model_id];
        let call =
            Call::new("get_model", Method::GET, &path).and_check(path_id("model_id", model_id));
        self.execute(call, Self::decode_response).await
    }

    /// Upload a file to the Files API.
    ///
    /// The file is sent as `multipart/form-data`. `expires_in_seconds`, when given, must be
    /// between 3,600 (one hour) and 7,776,000 (90 days).
    pub async fn upload_file(
        &self,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
        expires_in_seconds: Option<u64>,
    ) -> Result<FileInfo> {
        let check = if let Some(seconds) =
            expires_in_seconds.filter(|seconds| !FILE_EXPIRY_SECONDS.contains(seconds))
        {
            Err(Error::validation(
                format!(
                    "expires_in_seconds must be between {} and {}, got {seconds}",
                    FILE_EXPIRY_SECONDS.start(),
                    FILE_EXPIRY_SECONDS.end()
                ),
                Some("expires_in_seconds".to_string()),
            ))
        } else if [filename, mime_type]
            .iter()
            .any(|value| value.contains(['\r', '\n']))
        {
            Err(Error::validation(
                "filename and mime_type must not contain line breaks",
                Some("filename".to_string()),
            ))
        } else {
            Ok(())
        };
        let boundary = multipart_boundary();
        let call = Call {
            body: Some(Bytes::from(build_file_upload_body(
                &boundary,
                filename,
                mime_type,
                bytes,
                expires_in_seconds,
            ))),
            content_type: Some(format!("multipart/form-data; boundary={boundary}")),
            check,
            ..Call::new("upload_file", Method::POST, &["files"])
        };
        self.execute(call, Self::decode_response).await
    }

    /// List files stored through the Files API.
    pub async fn list_files(&self, params: Option<FileListParams>) -> Result<FileList> {
        let call = Call {
            query: params
                .as_ref()
                .map(FileListParams::query_pairs)
                .unwrap_or_default(),
            ..Call::new("list_files", Method::GET, &["files"])
        };
        self.execute(call, Self::decode_response).await
    }

    /// Fetch the metadata of a file.
    pub async fn get_file(&self, file_id: &str) -> Result<FileInfo> {
        let path = ["files", file_id];
        let call = Call::new("get_file", Method::GET, &path).and_check(path_id("file_id", file_id));
        self.execute(call, Self::decode_response).await
    }

    /// Download the raw content of a file.
    ///
    /// Only files created by code execution or skills are downloadable.
    pub async fn download_file(&self, file_id: &str) -> Result<Bytes> {
        let path = ["files", file_id, "content"];
        let call = Call {
            mode: ResponseMode::Streaming,
            ..Call::new("download_file", Method::GET, &path).and_check(path_id("file_id", file_id))
        };
        self.execute(call, Self::read_bytes).await
    }

    /// Delete a file.
    pub async fn delete_file(&self, file_id: &str) -> Result<DeletedFile> {
        let path = ["files", file_id];
        let call =
            Call::new("delete_file", Method::DELETE, &path).and_check(path_id("file_id", file_id));
        self.execute(call, Self::decode_response).await
    }
}

/// Checks `id` for use as one path segment, rejecting the empty, `.` and `..` IDs that
/// [`Anthropic::build_url`] would drop or turn into another endpoint. `param` names the argument.
fn path_id(param: &str, id: &str) -> Result<()> {
    if matches!(id, "" | "." | "..") {
        return Err(Error::validation(
            format!("{param} must not be empty, `.` or `..`"),
            Some(param.to_string()),
        ));
    }
    Ok(())
}

/// One API call for [`Anthropic::execute`]: what to send and how its span is labelled.
struct Call<'a> {
    /// The span's `operation` field: the name of the public method.
    operation: &'static str,
    /// The span's `gen_ai.request.model` field.
    model: Option<&'a Model>,
    /// The span's `stream` field.
    stream: bool,
    method: Method,
    /// Path segments after `/v1`, each percent-encoded on its own.
    path: &'a [&'a str],
    /// The query-string pairs, encoded when the URL is built.
    query: Vec<(String, String)>,
    body: Option<Bytes>,
    /// Replaces the default `application/json` content type.
    content_type: Option<String>,
    /// Replaces the default `application/json` accept type.
    accept: Option<&'static str>,
    mode: ResponseMode,
    /// The request's own betas; the client's defaults go first.
    betas: Vec<String>,
    /// Betas implied by the features the request uses.
    auto_betas: Vec<&'static str>,
    /// A failure found before sending, reported like any other failure.
    check: Result<()>,
}

impl<'a> Call<'a> {
    fn new(operation: &'static str, method: Method, path: &'a [&'a str]) -> Self {
        Self {
            operation,
            model: None,
            stream: false,
            method,
            path,
            query: Vec::new(),
            body: None,
            content_type: None,
            accept: None,
            mode: ResponseMode::Buffered,
            betas: Vec::new(),
            auto_betas: Vec::new(),
            check: Ok(()),
        }
    }

    /// A Messages request for `params`.
    fn message(operation: &'static str, params: &'a MessageCreateParams) -> Self {
        Self {
            model: Some(&params.model),
            betas: params.betas.clone().unwrap_or_default(),
            auto_betas: message_betas(params),
            check: params.validate(),
            ..Self::new(operation, Method::POST, &["messages"])
        }
        .with_json(params)
    }

    /// Sends `body` as JSON. A body that cannot be serialized fails the call.
    fn with_json(self, body: &impl serde::Serialize) -> Self {
        match serde_json::to_vec(body) {
            Ok(bytes) => Self {
                body: Some(Bytes::from(bytes)),
                ..self
            },
            Err(error) => self.and_check(Err(Error::validation(
                "Failed to serialize the request body",
                None,
            )
            .with_source(error))),
        }
    }

    /// Fails the call with `check`'s error, unless an earlier check already failed it.
    fn and_check(mut self, check: Result<()>) -> Self {
        if self.check.is_ok() {
            self.check = check;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        Canned, Recorded, read_request, record_events, record_targets, serve, uses_proxies,
    };
    use crate::{
        KnownModel, McpToolset, MessageBatchCreateParams, MessageBatchCreateRequest,
        MessageBatchListParams, MessageBatchResultVariant, MessageParam, Model, TaskBudget,
        ThinkingConfig, ThinkingDisplay,
    };
    use futures::StreamExt;
    use serde_json::Value;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::runtime::Builder;

    /// The request line of a recorded request, such as `GET /v1/models HTTP/1.1`.
    fn request_line(recorded: &Recorded) -> &str {
        recorded.headers.lines().next().unwrap()
    }

    /// The value of the header `name` in a recorded request.
    fn header<'a>(recorded: &'a Recorded, name: &str) -> Option<&'a str> {
        recorded.headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    #[test]
    fn resolve_api_key_regular_value() {
        let result = Anthropic::resolve_api_key("sk-test-key-123");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "sk-test-key-123");
    }

    #[test]
    fn resolve_api_key_file_url_absolute() {
        let test_dir = std::env::temp_dir().join(format!("carpa_test_{}", std::process::id()));
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
        assert!(matches!(error.kind(), ErrorKind::Io { .. }));
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
    fn build_url_default_base() {
        let client = Anthropic::try_new(Some("test_key".to_string())).unwrap();
        // Default base URL: https://api.anthropic.com
        assert_eq!(
            client.build_url(&["messages"]).unwrap().as_str(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            client
                .build_url(&["messages", "count_tokens"])
                .unwrap()
                .as_str(),
            "https://api.anthropic.com/v1/messages/count_tokens"
        );
        assert_eq!(
            client.build_url(&["models"]).unwrap().as_str(),
            "https://api.anthropic.com/v1/models"
        );
    }

    #[test]
    fn build_url_custom_base_without_trailing_slash() {
        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://api.minimax.io/anthropic".to_string())
            .unwrap();
        assert_eq!(
            client.build_url(&["messages"]).unwrap().as_str(),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn build_url_custom_base_with_trailing_slash() {
        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://api.minimax.io/anthropic/".to_string())
            .unwrap();
        assert_eq!(
            client.build_url(&["messages"]).unwrap().as_str(),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn default_headers_omit_authorization_for_default_base_url() {
        let client = Anthropic::try_new(Some("test_key".to_string())).unwrap();
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
        let client = Anthropic::try_new(Some("test_key".to_string())).unwrap();
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
        let client = Anthropic::try_new(Some("test_key".to_string()))
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
    fn build_url_accepts_base_with_v1_suffix() {
        for base_url in [
            "https://gateway.example.com/anthropic/v1",
            "https://gateway.example.com/anthropic/v1/",
            "https://gateway.example.com/anthropic",
        ] {
            let client = Anthropic::try_new(Some("test_key".to_string()))
                .unwrap()
                .with_base_url(base_url.to_string())
                .unwrap();
            assert_eq!(
                client.build_url(&["messages"]).unwrap().as_str(),
                "https://gateway.example.com/anthropic/v1/messages",
                "{base_url}"
            );
        }
    }

    #[test]
    fn build_url_minimax_china() {
        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://api.minimaxi.com/anthropic".to_string())
            .unwrap();
        assert_eq!(
            client.build_url(&["messages"]).unwrap().as_str(),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );
        assert_eq!(
            client
                .build_url(&["models", "claude-3-opus"])
                .unwrap()
                .as_str(),
            "https://api.minimaxi.com/anthropic/v1/models/claude-3-opus"
        );
    }

    #[test]
    fn default_headers_carry_key_version_and_content_type() {
        let client = Anthropic::try_new(Some("test_key".to_string())).unwrap();

        let headers = client.default_headers();

        assert!(headers.contains_key("x-api-key"));
        assert_eq!(headers["anthropic-version"], "2026-10-21");
        assert!(headers.contains_key("content-type"));
    }

    fn test_client() -> Anthropic {
        Anthropic::try_new(Some("test".to_string()))
            .unwrap()
            .with_max_retries(0)
    }

    fn sample_create_params() -> MessageCreateParams {
        MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
    }

    #[test]
    fn message_betas_none_by_default() {
        assert!(message_betas(&sample_create_params()).is_empty());
    }

    #[test]
    fn message_betas_context_management() {
        use crate::types::ContextEdit;
        let params = sample_create_params()
            .with_context_management(ContextManagement::new(vec![ContextEdit::clear_tool_uses()]));
        assert_eq!(message_betas(&params), vec![CONTEXT_MANAGEMENT_BETA]);
    }

    #[test]
    fn message_betas_compact_edit_adds_both() {
        use crate::types::ContextEdit;
        let params = sample_create_params()
            .with_context_management(ContextManagement::new(vec![ContextEdit::compact()]));
        assert_eq!(
            message_betas(&params),
            vec![CONTEXT_MANAGEMENT_BETA, COMPACT_EDIT_BETA]
        );
    }

    #[test]
    fn message_betas_compaction() {
        let params = sample_create_params().with_compaction(CompactionRequest::new());
        assert_eq!(message_betas(&params), vec![COMPACTION_BETA]);
    }

    #[test]
    fn message_betas_mcp_servers() {
        let params = sample_create_params()
            .with_mcp_servers(vec![McpServer::new("a", "https://mcp.example.com")]);
        assert_eq!(message_betas(&params), vec![MCP_CLIENT_BETA]);
    }

    #[test]
    fn message_betas_task_budget() {
        let params = sample_create_params()
            .with_output_config(OutputConfig::new().with_task_budget(TaskBudget::new(64_000)));
        assert_eq!(message_betas(&params), vec![TASK_BUDGETS_BETA]);
    }

    #[test]
    fn structured_outputs_never_adds_a_beta() {
        use crate::types::{OutputFormat, ToolParam, ToolUnionParam};
        let tool = ToolParam::new(
            "get_weather",
            serde_json::json!({"type": "object", "properties": {}}),
        )
        .with_strict(true);
        let params = sample_create_params()
            .with_output_config(OutputConfig::new().with_format(OutputFormat::json_schema(
                serde_json::json!({"type": "object", "additionalProperties": false}),
            )))
            .with_tools(vec![ToolUnionParam::CustomTool(tool)]);
        assert!(message_betas(&params).is_empty());
    }

    #[test]
    fn count_tokens_feature_betas_mirror_create_params() {
        use crate::types::{ContextEdit, MessageParam};
        let params = MessageCountTokensParams::new(
            vec![MessageParam::user("Hi")],
            Model::Known(KnownModel::ClaudeSonnet55),
        )
        .with_context_management(ContextManagement::new(vec![ContextEdit::compact()]))
        .with_mcp_servers(vec![McpServer::new("a", "https://mcp.example.com")])
        .with_output_config(OutputConfig::new().with_task_budget(TaskBudget::new(20_000)));
        assert_eq!(
            feature_betas(
                params.context_management.as_ref(),
                None,
                params.mcp_servers.as_deref(),
                params.tools.as_deref(),
                params.output_config.as_ref(),
            ),
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
        assert!(client.headers_with_betas(&[]).unwrap().is_none());
    }

    #[test]
    fn headers_with_betas_joins_with_comma() {
        let client = test_client();
        let betas = vec!["alpha".to_string(), "bravo".to_string()];
        let headers = client.headers_with_betas(&betas).unwrap().unwrap();
        assert_eq!(
            headers.get("anthropic-beta").unwrap().to_str().unwrap(),
            "alpha, bravo"
        );
    }

    fn in_progress_batch_json() -> &'static str {
        r#"{
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
        }"#
    }

    #[tokio::test]
    async fn create_message_batch_posts_expected_body_and_betas() {
        let (base_url, recorded) = serve(vec![Canned::json(200, in_progress_batch_json())]).await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
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

        let requests = recorded.lock().unwrap();
        assert_eq!(
            request_line(&requests[0]),
            "POST /v1/messages/batches HTTP/1.1"
        );
        assert_eq!(
            header(&requests[0], "anthropic-beta"),
            Some("default-beta, batch-beta, request-beta")
        );
        let body_json: Value = serde_json::from_str(&requests[0].body).unwrap();
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
    }

    #[tokio::test]
    async fn batch_requests_carry_the_betas_their_features_need() {
        let (base_url, recorded) = serve(vec![Canned::json(200, in_progress_batch_json())]).await;
        let request = MessageCreateParams::new(
            16,
            vec![MessageParam::user("ping")],
            Model::Known(KnownModel::ClaudeSonnet55),
        )
        .with_fallback(KnownModel::ClaudeHaiku55)
        .with_fallback_credit_token("credit")
        .with_thinking(ThinkingConfig::adaptive().with_display(ThinkingDisplay::Updates));
        let params =
            MessageBatchCreateParams::new(vec![MessageBatchCreateRequest::new("r", request)]);

        files_client(base_url)
            .create_message_batch(params)
            .await
            .unwrap();

        assert_eq!(
            header(&recorded.lock().unwrap()[0], "anthropic-beta"),
            Some(
                "server-side-fallback-2026-07-01, fallback-credit-2026-07-01, \
                 thinking-display-updates-2026-08-18"
            )
        );
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
        let (base_url, recorded) = serve(vec![Canned::json(200, response_body)]).await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let batch = client.get_message_batch("msgbatch_123").await.unwrap();
        assert_eq!(batch.id, "msgbatch_123");
        assert_eq!(batch.request_counts.succeeded, 1);

        let requests = recorded.lock().unwrap();
        assert_eq!(
            request_line(&requests[0]),
            "GET /v1/messages/batches/msgbatch_123 HTTP/1.1"
        );
        assert!(requests[0].body.is_empty());
    }

    #[tokio::test]
    async fn list_message_batches_sends_pagination_query_and_beta() {
        let response_body = r#"{
            "data": [],
            "has_more": false,
            "first_id": null,
            "last_id": null
        }"#;
        let (base_url, recorded) = serve(vec![Canned::json(200, response_body)]).await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
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

        let requests = recorded.lock().unwrap();
        assert_eq!(
            request_line(&requests[0]),
            "GET /v1/messages/batches?after_id=msgbatch_a&limit=20 HTTP/1.1"
        );
        assert_eq!(header(&requests[0], "anthropic-beta"), Some("list-beta"));
        assert!(requests[0].body.is_empty());
    }

    #[tokio::test]
    async fn last_rate_limits_reflects_response_headers() {
        let body = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;
        let (base_url, _) = serve(vec![
            Canned::json(200, body)
                .with_header("anthropic-ratelimit-requests-limit", "50")
                .with_header("anthropic-ratelimit-requests-remaining", "49")
                .with_header("anthropic-ratelimit-requests-reset", "2026-10-19T12:00:00Z")
                .with_header("request-id", "req_abc"),
        ])
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        assert_eq!(client.last_rate_limits(), None);
        let params = MessageCreateParams::simple("hi", KnownModel::ClaudeHaiku55);
        client.send(&params).await.unwrap();

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
        let (with_headers, _) = serve(vec![
            Canned::json(200, body)
                .with_header("anthropic-ratelimit-requests-limit", "50")
                .with_header("anthropic-ratelimit-requests-remaining", "49")
                .with_header("request-id", "req_abc"),
        ])
        .await;
        let (without_headers, _) = serve(vec![Canned::json(200, body)]).await;
        let (fast_only, _) = serve(vec![
            Canned::json(200, body).with_header("anthropic-fast-input-tokens-limit", "1000"),
        ])
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(with_headers)
            .unwrap();
        let params = MessageCreateParams::simple("hi", KnownModel::ClaudeHaiku55);
        client.send(&params).await.unwrap();
        let recorded = client.last_rate_limits().unwrap();

        let other = client.clone().with_base_url(without_headers).unwrap();
        other.send(&params).await.unwrap();
        assert_eq!(client.last_rate_limits(), Some(recorded.clone()));
        assert_eq!(
            client.last_rate_limits().unwrap().request_id.as_deref(),
            Some("req_abc")
        );

        let fast = client.clone().with_base_url(fast_only).unwrap();
        fast.send(&params).await.unwrap();
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
        let (base_url, recorded) = serve(vec![Canned::json(200, response_body)]).await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let batch = client.cancel_message_batch("msgbatch_123").await.unwrap();
        assert_eq!(batch.id, "msgbatch_123");

        let requests = recorded.lock().unwrap();
        assert_eq!(
            request_line(&requests[0]),
            "POST /v1/messages/batches/msgbatch_123/cancel HTTP/1.1"
        );
        assert!(requests[0].body.is_empty());
    }

    #[tokio::test]
    async fn delete_message_batch_uses_delete_endpoint() {
        let response_body = r#"{
            "id": "msgbatch_123",
            "type": "message_batch_deleted"
        }"#;
        let (base_url, recorded) = serve(vec![Canned::json(200, response_body)]).await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let deleted = client.delete_message_batch("msgbatch_123").await.unwrap();
        assert_eq!(
            serde_json::to_value(&deleted).unwrap()["type"],
            "message_batch_deleted"
        );

        let requests = recorded.lock().unwrap();
        assert_eq!(
            request_line(&requests[0]),
            "DELETE /v1/messages/batches/msgbatch_123 HTTP/1.1"
        );
        assert!(requests[0].body.is_empty());
    }

    #[tokio::test]
    async fn batch_result_errors_say_how_many_results_were_delivered() {
        let good = r#"{"custom_id":"a","result":{"type":"expired"}}"#;
        let body = format!("not json\n{good}\n{good}\n{{\"custom_id\":5}}\n{good}\n");
        let bytes = futures::stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(body))]);
        let items: Vec<_> = process_message_batch_result_jsonl(bytes).collect().await;
        let delivered: Vec<_> = items
            .iter()
            .map(|item| item.as_ref().map_err(Error::events_delivered))
            .map(|item| item.map(|_| ()))
            .collect();
        assert_eq!(
            delivered,
            [Err(Some(0)), Ok(()), Ok(()), Err(Some(2)), Ok(())],
            "{items:?}"
        );
    }

    #[tokio::test]
    async fn stream_message_batch_results_preserves_jsonl_order_without_trailing_newline() {
        let results_body = concat!(
            r#"{"custom_id":"second","result":{"type":"expired"}}"#,
            "\n",
            r#"{"custom_id":"first","result":{"type":"succeeded","message":{"id":"msg_123","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}}"#
        );
        let (base_url, recorded) = serve(vec![
            Canned::json(200, results_body).with_content_type("application/jsonl"),
        ])
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
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
        assert_eq!(
            request_line(&recorded.lock().unwrap()[0]),
            "GET /v1/messages/batches/msgbatch_123/results HTTP/1.1"
        );
    }

    /// The event takes longer than the timeout to arrive, but no gap between its 25 chunks comes
    /// near it: each gap is a 20th of the timeout. (A paused clock does not work here: it jumps to
    /// the client's timeouts while the loopback server is still answering.)
    #[tokio::test]
    async fn streaming_timeout_is_inactivity_based() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
Content-Type: text/event-stream\r\n\
Cache-Control: no-cache\r\n\
Connection: close\r\n\r\nevent: ping\n",
                )
                .await
                .unwrap();
            for _ in 0..25 {
                socket.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                socket.write_all(b": keepalive\n").await.unwrap();
            }
            socket.write_all(b"data: {}\n\n").await.unwrap();
            socket.flush().await.unwrap();
        });

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_timeout(Duration::from_millis(400))
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

    /// Serves one SSE response: `head` at once, then `tail` after `stall`.
    async fn serve_stalled_stream(
        head: &'static [u8],
        stall: Duration,
        tail: &'static [u8],
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
Content-Type: text/event-stream\r\n\
Cache-Control: no-cache\r\n\
Connection: close\r\n\r\n",
                )
                .await
                .unwrap();
            socket.write_all(head).await.unwrap();
            socket.flush().await.unwrap();
            tokio::time::sleep(stall).await;
            socket.write_all(tail).await.unwrap();
            socket.flush().await.unwrap();
        });
        base_url
    }

    #[tokio::test]
    async fn streaming_read_timeout_outlasts_total_timeout() {
        let base_url = serve_stalled_stream(
            b"event: ping\n",
            Duration::from_millis(120),
            b"data: {}\n\n",
        )
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_timeout(Duration::from_millis(50))
            .unwrap()
            .with_read_timeout(Duration::from_secs(5))
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
    async fn with_timeout_keeps_configured_read_timeout() {
        let base_url = serve_stalled_stream(
            b"event: ping\n",
            Duration::from_millis(120),
            b"data: {}\n\n",
        )
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_read_timeout(Duration::from_secs(5))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_timeout(Duration::from_millis(50))
            .unwrap();
        assert_eq!(client.stream_read_timeout(), Duration::from_secs(5));
        let params = MessageCreateParams::new_streaming(
            16,
            vec![MessageParam::user("ping")],
            Model::Known(KnownModel::ClaudeHaiku55),
        );

        let mut stream = std::pin::pin!(client.stream(&params).await.unwrap());
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, MessageStreamEvent::Ping);
    }

    /// Fails if streams go back to a 60 second read cap. The clock is paused only once the stream
    /// is open, so the 61 second stall passes at once.
    #[tokio::test]
    async fn default_stream_read_timeout_outlasts_sixty_seconds() {
        let base_url = serve_stalled_stream(
            b"event: ping\ndata: {}\n\n",
            Duration::from_secs(61),
            b"event: ping\ndata: {}\n\n",
        )
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        assert_eq!(client.stream_read_timeout(), DEFAULT_TIMEOUT);
        let params = MessageCreateParams::new_streaming(
            16,
            vec![MessageParam::user("ping")],
            Model::Known(KnownModel::ClaudeHaiku55),
        );

        let mut stream = std::pin::pin!(client.stream(&params).await.unwrap());
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, MessageStreamEvent::Ping);
        tokio::time::pause();
        let second = stream.next().await.unwrap().unwrap();
        assert_eq!(second, MessageStreamEvent::Ping);
    }

    /// A stall far longer than any test run, so only the client's timeout can end it.
    const ENDLESS_STALL: Duration = Duration::from_secs(3600);

    #[tokio::test]
    async fn streaming_stall_reports_timeout_error() {
        let (base_url, _) = serve(vec![
            Canned::json(200, "event: ping\ndata: {}\n\n")
                .with_content_type("text/event-stream")
                .with_stall(ENDLESS_STALL),
        ])
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
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
        assert!(matches!(err.kind(), ErrorKind::Timeout));
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[tokio::test]
    async fn non_streaming_body_stall_reports_timeout_error() {
        let (base_url, _) = serve(vec![
            Canned::json(200, reply_json()).with_stall(ENDLESS_STALL),
        ])
        .await;

        let client = Anthropic::try_new(Some("test_key".to_string()))
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

        let err = client.send(&params).await.unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::Timeout), "{err:?}");
    }

    fn file_info_json() -> &'static str {
        r#"{"id":"file_01","type":"file","filename":"notes.txt","mime_type":"text/plain","size_bytes":5,"created_at":"2026-10-19T00:00:00Z","downloadable":false}"#
    }

    fn files_client(base_url: String) -> Anthropic {
        Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
    }

    #[test]
    fn message_betas_mcp_toolset() {
        let params = sample_create_params()
            .with_tools(vec![ToolUnionParam::McpToolset(McpToolset::new("a"))]);
        assert_eq!(message_betas(&params), vec![MCP_CLIENT_BETA]);
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
        let (base_url, recorded) = serve(vec![Canned::json(200, file_info_json())]).await;

        let info = files_client(base_url)
            .upload_file("notes.txt", "text/plain", b"hello", Some(7_200))
            .await
            .unwrap();
        assert_eq!(info.id, "file_01");
        assert_eq!(info.size_bytes, 5);
        assert!(!info.downloadable);

        let requests = recorded.lock().unwrap();
        assert_eq!(request_line(&requests[0]), "POST /v1/files HTTP/1.1");
        let boundary = header(&requests[0], "content-type")
            .unwrap()
            .strip_prefix("multipart/form-data; boundary=")
            .expect("multipart content type");
        assert!(header(&requests[0], "anthropic-beta").is_none());
        assert_eq!(
            requests[0].body,
            format!(
                "--{boundary}\r\n\
                 Content-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\n\
                 Content-Type: text/plain\r\n\r\nhello\r\n\
                 --{boundary}\r\n\
                 Content-Disposition: form-data; name=\"expires_in_seconds\"\r\n\r\n7200\r\n\
                 --{boundary}--\r\n"
            )
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn upload_file_rejects_bad_expiry_and_line_breaks() {
        let client = test_client();
        let (guard, targets) = record_targets();
        let too_short = client
            .upload_file("a.txt", "text/plain", &[], Some(60))
            .await
            .unwrap_err();
        drop(guard);
        assert!(matches!(too_short.kind(), ErrorKind::Validation { .. }));
        assert!(
            targets
                .lock()
                .unwrap()
                .contains(&"event carpa::client".to_string()),
            "a validation failure is reported like any other: {targets:?}"
        );
        let too_long = client
            .upload_file("a.txt", "text/plain", &[], Some(7_776_001))
            .await
            .unwrap_err();
        assert!(matches!(too_long.kind(), ErrorKind::Validation { .. }));
        let bad_name = client
            .upload_file("a\r\nb.txt", "text/plain", &[], None)
            .await
            .unwrap_err();
        assert!(matches!(bad_name.kind(), ErrorKind::Validation { .. }));
    }

    #[tokio::test]
    async fn list_files_sends_query() {
        let body = format!(
            r#"{{"data":[{}],"has_more":true,"next_page":"p3"}}"#,
            file_info_json()
        );
        let (base_url, recorded) = serve(vec![Canned::json(200, body)]).await;

        let list = files_client(base_url)
            .list_files(Some(FileListParams::new().with_limit(2).with_page("p2")))
            .await
            .unwrap();
        assert_eq!(list.data.len(), 1);
        assert_eq!(list.next_page.as_deref(), Some("p3"));
        assert_eq!(list.has_more, Some(true));
        assert_eq!(
            request_line(&recorded.lock().unwrap()[0]),
            "GET /v1/files?limit=2&page=p2 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn get_file_uses_file_endpoint() {
        let (base_url, recorded) = serve(vec![Canned::json(200, file_info_json())]).await;

        let info = files_client(base_url).get_file("file_01").await.unwrap();
        assert_eq!(info.filename, "notes.txt");
        assert_eq!(
            request_line(&recorded.lock().unwrap()[0]),
            "GET /v1/files/file_01 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn ids_stay_one_path_segment() {
        let (base_url, recorded) = serve(vec![Canned::json(200, file_info_json())]).await;

        files_client(base_url).get_file("../models").await.unwrap();
        assert_eq!(
            request_line(&recorded.lock().unwrap()[0]),
            "GET /v1/files/..%2Fmodels HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn dot_and_empty_ids_fail_before_sending() {
        let (base_url, recorded) = serve(Vec::new()).await;
        let client = files_client(base_url);
        let (guard, events) = record_events("carpa::client");

        for id in ["", ".", ".."] {
            let errors = [
                client.get_file(id).await.unwrap_err(),
                client.delete_file(id).await.unwrap_err(),
                client.get_message_batch(id).await.unwrap_err(),
                client.get_model(id).await.unwrap_err(),
            ];
            let params: Vec<_> = errors
                .iter()
                .map(|error| match error.kind() {
                    ErrorKind::Validation { param } => param.clone(),
                    other => panic!("{id:?}: {other:?}"),
                })
                .collect();
            assert_eq!(
                params,
                ["file_id", "file_id", "message_batch_id", "model_id"].map(|p| Some(p.to_string())),
                "{id:?}"
            );
        }
        drop(guard);
        assert!(recorded.lock().unwrap().is_empty());
        let events = events.lock().unwrap();
        let count = |message: &str| events.iter().filter(|e| e.message == message).count();
        assert_eq!(
            count("request_completed"),
            12,
            "a bad ID is reported like any other failure: {events:?}"
        );
        assert_eq!(count("API request failed"), 12, "{events:?}");
    }

    #[tokio::test]
    async fn default_betas_reach_the_file_endpoints() {
        let (base_url, recorded) = serve(vec![Canned::json(200, file_info_json())]).await;

        files_client(base_url)
            .with_default_betas(["files-api-2025-04-14"])
            .get_file("file_01")
            .await
            .unwrap();
        assert_eq!(
            header(&recorded.lock().unwrap()[0], "anthropic-beta"),
            Some("files-api-2025-04-14")
        );
    }

    #[tokio::test]
    async fn delete_file_uses_delete_endpoint() {
        let (base_url, recorded) = serve(vec![Canned::json(
            200,
            r#"{"id":"file_01","type":"file_deleted"}"#,
        )])
        .await;

        let deleted = files_client(base_url).delete_file("file_01").await.unwrap();
        assert_eq!(deleted.id, "file_01");
        assert_eq!(
            serde_json::to_value(&deleted).unwrap()["type"],
            "file_deleted"
        );
        assert_eq!(
            request_line(&recorded.lock().unwrap()[0]),
            "DELETE /v1/files/file_01 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn download_file_returns_raw_bytes() {
        let payload = "\u{0}\u{9f}raw\u{ff}";
        let (base_url, recorded) = serve(vec![
            Canned::json(200, payload).with_content_type("application/octet-stream"),
        ])
        .await;

        let bytes = files_client(base_url)
            .download_file("file_01")
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), payload.as_bytes());
        assert_eq!(
            request_line(&recorded.lock().unwrap()[0]),
            "GET /v1/files/file_01/content HTTP/1.1"
        );
    }

    fn send_params() -> MessageCreateParams {
        MessageCreateParams::new(
            16,
            vec![MessageParam::user("hi")],
            Model::Known(KnownModel::ClaudeHaiku55),
        )
    }

    fn reply_json() -> String {
        serde_json::json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-haiku-5-5",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn", "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })
        .to_string()
    }

    #[tokio::test]
    async fn send_with_stream_enabled_sends_a_buffered_request() {
        let (base_url, recorded) = serve(vec![Canned::json(200, reply_json())]).await;
        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();

        let message = client.send(&send_params().with_stream(true)).await.unwrap();

        assert_eq!(message.id, "msg_1");
        let requests = recorded.lock().unwrap();
        let body: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(body["stream"], Value::Bool(false));
    }

    #[test]
    fn base_url_drops_only_a_final_v1_segment() {
        for (input, expected) in [
            ("https://example.com/v1", "https://example.com"),
            ("https://example.com/v1/", "https://example.com"),
            ("https://example.com//v1", "https://example.com"),
            ("https://example.com/proxy/v1", "https://example.com/proxy"),
            (
                "https://example.com/proxy/v1//",
                "https://example.com/proxy",
            ),
            ("https://example.com/v1beta", "https://example.com/v1beta"),
            ("https://example.com/api/v10", "https://example.com/api/v10"),
            ("https://example.com/v1/v1", "https://example.com/v1"),
            ("https://v1", "https://v1"),
            ("https://v1/", "https://v1"),
            ("https://v1/v1", "https://v1"),
            ("http://localhost:8080/v1", "http://localhost:8080"),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("http://[::1]:8080/proxy/", "http://[::1]:8080/proxy"),
        ] {
            let client = Anthropic::try_new(Some("test_key".to_string()))
                .unwrap()
                .with_base_url(input.to_string())
                .unwrap();
            assert_eq!(client.base_url, expected, "{input}");
        }
    }

    #[test]
    fn base_url_needs_https_or_loopback_http() {
        let client = || Anthropic::try_new(Some("test_key".to_string())).unwrap();
        for url in [
            "http://api.anthropic.com",
            "http://10.0.0.1",
            "http://v1",
            "ftp://127.0.0.1",
            "not a url",
            "https://user:pass@api.anthropic.com",
            "https://api.anthropic.com?x=1",
            "https://api.anthropic.com/#v1",
        ] {
            let error = client().with_base_url(url.to_string()).unwrap_err();
            assert!(
                matches!(error.kind(), ErrorKind::Validation { param, .. } if param.as_deref() == Some("base_url")),
                "{url}: {error}"
            );
        }
    }

    #[test]
    fn try_new_checks_the_base_url_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                Ok(pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string()))
            }
        };
        let key = || Some("test_key".to_string());

        let client = Anthropic::try_new_with(key(), env(&[])).unwrap();
        assert_eq!(client.base_url, DEFAULT_API_URL);
        let client = Anthropic::try_new_with(
            key(),
            env(&[("ANTHROPIC_BASE_URL", " https://gw.example/v1/ ")]),
        )
        .unwrap();
        assert_eq!(client.base_url, "https://gw.example");
        let client = Anthropic::try_new_with(
            key(),
            env(&[
                ("CARPA_BASE_URL", "http://localhost:9000"),
                ("ANTHROPIC_BASE_URL", "http://api.anthropic.com"),
            ]),
        )
        .unwrap();
        assert_eq!(client.base_url, "http://localhost:9000");

        let error = Anthropic::try_new_with(
            key(),
            env(&[("ANTHROPIC_BASE_URL", "http://api.anthropic.com")]),
        )
        .unwrap_err();
        assert!(
            matches!(error.kind(), ErrorKind::Validation { param, .. } if param.as_deref() == Some("ANTHROPIC_BASE_URL")),
            "{error}"
        );
        let error = Anthropic::try_new_with(key(), env(&[("CARPA_BASE_URL", "http://v1")]))
            .unwrap_err();
        assert!(
            matches!(error.kind(), ErrorKind::Validation { param, .. } if param.as_deref() == Some("CARPA_BASE_URL")),
            "{error}"
        );
    }

    #[test]
    fn loopback_base_url_makes_both_clients_skip_proxies() {
        let both_use_proxies = |client: &Anthropic| {
            let (buffered, streaming) = (
                uses_proxies(&client.client),
                uses_proxies(&client.stream_client),
            );
            assert_eq!(buffered, streaming, "the two clients disagree on proxies");
            buffered
        };
        let no_env = |_: &str| Ok(None);
        let key = || Some("test_key".to_string());

        let client = Anthropic::try_new_with(key(), no_env).unwrap();
        assert!(both_use_proxies(&client));
        let client = client
            .with_base_url("http://127.0.0.1:1".to_string())
            .unwrap();
        assert!(!both_use_proxies(&client));
        let client = client.with_timeout(Duration::from_secs(30)).unwrap();
        assert!(!both_use_proxies(&client));
        let client = client
            .with_base_url("https://api.anthropic.com".to_string())
            .unwrap();
        assert!(both_use_proxies(&client));

        for name in ["CARPA_BASE_URL", "ANTHROPIC_BASE_URL"] {
            let get = |var: &str| Ok((var == name).then(|| "http://127.0.0.1:1".to_string()));
            let client = Anthropic::try_new_with(key(), get).unwrap();
            assert!(!both_use_proxies(&client), "{name}");
        }
    }

    #[test]
    fn error_body_type_becomes_a_typed_error_type() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let bodies = [
            r#"{"type":"error","error":{"type":"request_too_large","message":"big"}}"#,
            r#"{"type":"error","error":{"type":"saucepan_error","message":"hot"}}"#,
            r#"{"type":"error","error":"text"}"#,
            r#"{"type":"error","error":{"type":5,"message":"number"}}"#,
        ];
        let canned = bodies.iter().map(|body| Canned::json(400, *body)).collect();
        let (base_url, _recorded) = runtime.block_on(serve(canned));
        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let send = || {
            crate::test_support::unknown_warnings(|| {
                let error = runtime.block_on(client.send(&send_params())).unwrap_err();
                match error.kind() {
                    ErrorKind::Api { error_type, .. } => *error_type,
                    other => panic!("expected an Api error, got {other:?}"),
                }
            })
        };

        let (code, warnings) = send();
        assert_eq!(code, Some(ErrorType::RequestTooLarge));
        assert_eq!(warnings, Vec::<String>::new());
        let (code, warnings) = send();
        assert_eq!(code, Some(ErrorType::Unknown));
        assert_eq!(warnings, ["rust_type=ErrorType tag=saucepan_error"]);
        for _ in 0..2 {
            let (code, warnings) = send();
            assert_eq!(code, None);
            assert_eq!(warnings, Vec::<String>::new());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn requests_trace_on_the_client_target() {
        let (base_url, _recorded) = serve(vec![Canned::json(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#,
        )])
        .await;
        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let (guard, targets) = record_targets();

        let error = client.send(&send_params()).await.unwrap_err();
        drop(guard);

        assert_eq!(error.status(), Some(400));
        let targets = targets.lock().unwrap();
        assert!(
            targets.contains(&"span carpa.request carpa::client".to_string()),
            "{targets:?}"
        );
        assert!(
            targets.contains(&"event carpa::client".to_string()),
            "{targets:?}"
        );
    }

    #[tokio::test]
    async fn count_tokens_and_list_models_reject_unknown_values_without_a_request() {
        let (base_url, recorded) = serve(Vec::new()).await;
        let client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap();
        let validation_param = |error: Error| match error.kind() {
            ErrorKind::Validation { param, .. } => param.clone(),
            _ => panic!("expected a validation error, got {error}"),
        };

        let mut count = MessageCountTokensParams::new(
            vec![MessageParam::user("hi")],
            Model::Known(KnownModel::ClaudeHaiku55),
        );
        count.messages[0].role = crate::MessageRole::Unknown;
        let error = client.count_tokens(count.clone()).await.unwrap_err();
        assert_eq!(validation_param(error).as_deref(), Some("messages[0].role"));
        count.messages[0].role = crate::MessageRole::User;
        let error = client
            .count_tokens(count.with_speed(crate::InferenceSpeed::Unknown))
            .await
            .unwrap_err();
        assert_eq!(validation_param(error).as_deref(), Some("speed"));

        let list = ModelListParams::new()
            .with_lifecycle(crate::ModelLifecycle::Active)
            .with_lifecycle(crate::ModelLifecycle::Unknown);
        let error = client.list_models(Some(list)).await.unwrap_err();
        assert_eq!(validation_param(error).as_deref(), Some("lifecycle[1]"));

        assert!(recorded.lock().unwrap().is_empty());
    }

    fn quiet_client(base_url: String) -> Anthropic {
        files_client(base_url).with_max_retries(0)
    }

    fn streaming_params() -> MessageCreateParams {
        MessageCreateParams::new_streaming(
            16,
            vec![MessageParam::user("hi")],
            Model::Known(KnownModel::ClaudeHaiku55),
        )
    }

    #[tokio::test]
    async fn stream_items_keep_their_kind_with_or_without_a_request_id() {
        let bad_event = "event: message_start\ndata: nope\n\n";
        let mut kinds = Vec::new();
        for request_id in [Some("req_stream"), None] {
            let mut canned = Canned::json(200, bad_event).with_content_type("text/event-stream");
            if let Some(id) = request_id {
                canned = canned.with_header("request-id", id);
            }
            let (base_url, _) = serve(vec![canned]).await;
            let client = quiet_client(base_url);
            let mut stream = std::pin::pin!(client.stream(&streaming_params()).await.unwrap());
            let error = stream.next().await.unwrap().unwrap_err();
            assert_eq!(error.request_id(), request_id);
            assert_eq!(error.events_delivered(), Some(0));
            kinds.push(error.kind().clone());
        }
        assert_eq!(kinds[0], ErrorKind::Decode);
        assert_eq!(kinds[0], kinds[1]);
    }

    #[tokio::test]
    async fn stream_passes_events_through_without_accumulating() {
        let start = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-haiku-5-5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}

"#;
        let stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let events = |body: String| async move {
            let canned = Canned::json(200, body).with_content_type("text/event-stream");
            let (base_url, _) = serve(vec![canned]).await;
            let client = quiet_client(base_url);
            let stream = client.stream(&streaming_params()).await.unwrap();
            stream.collect::<Vec<_>>().await
        };

        let complete = events(format!("{start}{stop}")).await;
        assert_eq!(complete.len(), 2, "{complete:?}");
        assert!(
            matches!(&complete[0], Ok(MessageStreamEvent::MessageStart(_))),
            "{complete:?}"
        );
        assert!(
            matches!(&complete[1], Ok(MessageStreamEvent::MessageStop(_))),
            "{complete:?}"
        );

        let cut = events(start.to_string()).await;
        assert!(
            matches!(&cut[0], Ok(MessageStreamEvent::MessageStart(_))),
            "{cut:?}"
        );
        let last = cut.last().unwrap().as_ref().unwrap_err();
        assert_eq!(last.kind(), &ErrorKind::Interrupted, "{cut:?}");
    }

    #[tokio::test]
    async fn batch_result_items_keep_their_kind_with_or_without_a_request_id() {
        let mut kinds = Vec::new();
        for request_id in [Some("req_batch"), None] {
            let mut canned =
                Canned::json(200, "SECRET-LINE\n").with_content_type("application/x-jsonl");
            if let Some(id) = request_id {
                canned = canned.with_header("request-id", id);
            }
            let (base_url, _) = serve(vec![canned]).await;
            let client = quiet_client(base_url);
            let mut results = std::pin::pin!(
                client
                    .stream_message_batch_results("msgbatch_1")
                    .await
                    .unwrap()
            );
            let error = results.next().await.unwrap().unwrap_err();
            assert_eq!(error.request_id(), request_id);
            assert!(!error.to_string().contains("SECRET-LINE"), "{error}");
            assert!(std::error::Error::source(&error).is_none());
            kinds.push(error.kind().clone());
        }
        assert_eq!(kinds[0], ErrorKind::Decode);
        assert_eq!(kinds[0], kinds[1]);
    }

    /// The base URL must be https to pass the base URL rule and never answer, so the client is
    /// swapped for one that skips proxies. A retry waits out a backoff of at least half a second
    /// on the paused clock; one attempt alone takes at most the 100 ms connect timeout.
    #[tokio::test(start_paused = true)]
    async fn connect_timeout_is_retried() {
        let mut client = Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url("https://192.0.2.1".to_string())
            .unwrap()
            .with_max_retries(1);
        client.client = ReqwestClient::builder()
            .connect_timeout(Duration::from_millis(100))
            .no_proxy()
            .build()
            .unwrap();
        let start = tokio::time::Instant::now();

        let error = client.get_model("m").await.unwrap_err();

        assert_eq!(error.kind(), &ErrorKind::Connection, "{error}");
        assert!(error.is_retryable());
        assert!(start.elapsed() >= Duration::from_millis(500));
    }

    #[tokio::test]
    async fn api_error_display_names_service_status_type_and_request() {
        let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let (base_url, _) = serve(vec![
            Canned::json(529, body).with_header("request-id", "req_123"),
        ])
        .await;
        let error = quiet_client(base_url)
            .send(&send_params())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Anthropic API error: Overloaded (HTTP 529 overloaded_error, request req_123)"
        );
        assert_eq!(error.status(), Some(529));
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn error_body_without_a_message_says_so() {
        let (base_url, _) = serve(vec![Canned::json(503, "<html>bad gateway</html>")]).await;
        let error = quiet_client(base_url)
            .send(&send_params())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Anthropic API error: HTTP 503 with no error message (HTTP 503)"
        );
    }

    #[tokio::test]
    async fn invalid_beta_value_is_rejected_before_sending() {
        let (base_url, recorded) = serve(Vec::new()).await;
        let client = quiet_client(base_url);
        let params = send_params().with_beta("bad\nbeta");

        let error = client.send(&params).await.unwrap_err();

        match error.kind() {
            ErrorKind::Validation { param } => assert_eq!(param.as_deref(), Some("betas")),
            other => panic!("expected a validation error, got {other:?}"),
        }
        assert!(std::error::Error::source(&error).is_some());
        let error = client
            .with_default_betas(["bad\nbeta"])
            .list_models(None)
            .await
            .unwrap_err();
        assert!(
            matches!(error.kind(), ErrorKind::Validation { .. }),
            "{error}"
        );
        assert!(recorded.lock().unwrap().is_empty());
    }

    #[test]
    #[should_panic(expected = "rate-limit lock poisoned")]
    fn poisoned_rate_limit_lock_panics() {
        let client = test_client();
        let slot = client.last_rate_limits.clone();
        let _ = std::thread::spawn(move || {
            let _guard = slot.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        let _ = client.last_rate_limits();
    }

    #[tokio::test]
    #[should_panic(expected = "rate-limit lock poisoned")]
    async fn recording_rate_limits_under_a_poisoned_lock_panics() {
        let canned =
            Canned::json(200, "{}").with_header("anthropic-ratelimit-requests-remaining", "49");
        let (base_url, _) = serve(vec![canned]).await;
        let response = ReqwestClient::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(base_url)
            .send()
            .await
            .unwrap();
        let client = test_client();
        let slot = client.last_rate_limits.clone();
        let _ = std::thread::spawn(move || {
            let _guard = slot.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        client.record_rate_limits(&response);
    }

    #[tokio::test]
    async fn success_body_that_is_not_json_is_decode_with_the_request_id() {
        for request_id in [Some("req_ok"), None] {
            let mut canned = Canned::json(200, r#"{"id": "SECRET-BODY", "type": 7"#);
            if let Some(id) = request_id {
                canned = canned.with_header("request-id", id);
            }
            let (base_url, _) = serve(vec![canned]).await;
            let error = quiet_client(base_url)
                .send(&send_params())
                .await
                .unwrap_err();
            assert_eq!(error.kind(), &ErrorKind::Decode, "{error}");
            assert_eq!(error.request_id(), request_id);
            assert!(!error.is_retryable());
            assert!(!error.to_string().contains("SECRET-BODY"), "{error}");
            assert!(std::error::Error::source(&error).is_none(), "{error:?}");
            assert!(error.to_string().contains("31 bytes"), "{error}");
        }
    }

    #[tokio::test]
    async fn download_body_failure_carries_the_request_id() {
        let (base_url, _) = serve(vec![
            Canned::json(200, "partial")
                .with_header("request-id", "req_dl")
                .with_cut_body(),
        ])
        .await;
        let error = quiet_client(base_url)
            .download_file("file_01")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Interrupted, "{error}");
        assert_eq!(error.request_id(), Some("req_dl"));
    }

    /// A client for tests on the paused clock. Its HTTP clients have no timeouts, which the
    /// clock would otherwise jump to while the loopback server is still answering.
    fn untimed_client(base_url: String) -> Anthropic {
        let mut client = files_client(base_url);
        let untimed = || ReqwestClient::builder().no_proxy().build().unwrap();
        client.client = untimed();
        client.stream_client = untimed();
        client
    }

    #[tokio::test(start_paused = true)]
    async fn a_get_whose_body_read_fails_is_resent() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, "partial").with_cut_body(),
            Canned::json(200, file_info_json()),
        ])
        .await;
        let info = untimed_client(base_url).get_file("file_01").await.unwrap();
        assert_eq!(info.id, "file_01");
        assert_eq!(recorded.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_post_closed_before_any_reply_is_resent() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, "").closing_unanswered(),
            Canned::json(200, reply_json()),
        ])
        .await;
        let message = untimed_client(base_url).send(&send_params()).await.unwrap();
        assert_eq!(message.id, "msg_1");
        assert_eq!(recorded.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_post_whose_body_read_fails_is_not_resent() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, "partial").with_cut_body(),
            Canned::json(200, "{}"),
        ])
        .await;
        let error = files_client(base_url)
            .cancel_message_batch("msgbatch_1")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Interrupted, "{error}");
        assert!(error.is_retryable());
        assert_eq!(recorded.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn spend_limit_429_is_not_resent_but_a_plain_429_is() {
        let spend_cap = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Spend limit reached","details":{"error_code":"enforced_spend_limit_reached"}}}"#;
        let (base_url, recorded) = serve(vec![
            Canned::json(429, spend_cap),
            Canned::json(200, reply_json()),
        ])
        .await;
        let error = untimed_client(base_url)
            .send(&send_params())
            .await
            .unwrap_err();
        assert_eq!(error.status(), Some(429));
        assert!(!error.is_retryable());
        match error.kind() {
            ErrorKind::Api { error_code, .. } => {
                assert_eq!(
                    error_code,
                    &Some(ErrorCode::Anthropic(
                        "enforced_spend_limit_reached".to_string()
                    ))
                );
            }
            other => panic!("expected an Api error, got {other:?}"),
        }
        assert_eq!(recorded.lock().unwrap().len(), 1);

        let plain = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Slow down"}}"#;
        let (base_url, recorded) = serve(vec![
            Canned::json(429, plain).with_header("retry-after", "0"),
            Canned::json(200, reply_json()),
        ])
        .await;
        let message = untimed_client(base_url).send(&send_params()).await.unwrap();
        assert_eq!(message.id, "msg_1");
        assert_eq!(recorded.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_retry_logs_one_event_with_its_cause() {
        let overloaded =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let (base_url, _) = serve(vec![
            Canned::json(529, overloaded),
            Canned::json(200, reply_json()),
        ])
        .await;
        let client = untimed_client(base_url);
        let (guard, events) = record_events("carpa::client");
        client.send(&send_params()).await.unwrap();
        drop(guard);

        let events = events.lock().unwrap();
        let retries: Vec<_> = events.iter().filter(|e| e.message == "retry").collect();
        assert_eq!(retries.len(), 1, "{events:?}");
        assert_eq!(retries[0].level, tracing::Level::WARN);
        assert!(
            retries[0]
                .fields
                .starts_with("error=Anthropic API error: Overloaded (HTTP 529 overloaded_error) attempt=1 backoff_seconds="),
            "{retries:?}"
        );
        let completed = events
            .iter()
            .filter(|e| e.message == "request_completed")
            .count();
        assert_eq!(completed, 1, "{events:?}");
    }
}
