//! The Jev HTTP client.

use std::fmt;
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::Client as ReqwestClient;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::redirect;
use serde::Serialize;
use serde_json::Value;
use tokio::time::sleep;

use super::questions::WireQuestions;
use super::{Choices, JevErrorCode, JevResponse, Levels, Outcome, Questions};
use crate::backoff;
use crate::base_url;
use crate::error::{Error, ErrorCode, HttpMetadata, RequestPhase, Result, map_reqwest_error};
use crate::types::retry_after_from_headers;

pub(super) const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const PATH: &str = "/v1/systemone";
const API_KEY_ENV: &str = "TYPESAFE_API_KEY";
const MODEL_ENV: &str = "TYPESAFE_DEFAULT_MODEL";
const BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
const USER_AGENT: &str = concat!("topcoat-si/", env!("CARGO_PKG_VERSION"));
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_RETRIES: usize = 2;
const DEFAULT_RETRY_BUDGET: Duration = Duration::from_secs(30);
// A longer Retry-After means the service wants callers gone for a while, so fail right away.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_STATE_DEPTH: usize = 64;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERROR_CHARS: usize = 200;
// The question name the one-question shortcuts send. Jev never sees names.
const SHORTCUT_QUESTION: &str = "answer";

/// Client for Jev's `POST /v1/systemone`.
///
/// The API key is only kept as a sensitive header value and is redacted from `Debug`. There is
/// no default model: answer thresholds are tuned per model and aliases such as `jev-latest`
/// move, so pin a versioned id such as `jev-1.13.0`.
#[derive(Clone)]
pub struct JevClient {
    pub(super) http: ReqwestClient,
    authorization: HeaderValue,
    pub(super) base_url: String,
    model: String,
    timeout: Duration,
    max_retries: usize,
    retry_budget: Duration,
}

impl fmt::Debug for JevClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JevClient")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("max_retries", &self.max_retries)
            .field("retry_budget", &self.retry_budget)
            .finish_non_exhaustive()
    }
}

impl JevClient {
    /// Create a client with a TypeSafe API key and the Jev model to ask.
    ///
    /// Pin a versioned model id such as `jev-1.13.0`; aliases like `jev-latest` move and
    /// confidence thresholds tuned on one model do not carry over to the next.
    ///
    /// Fails when the key is empty after trimming or is not printable ASCII without
    /// whitespace, when the model is empty after trimming, or when the HTTP client cannot be
    /// built.
    pub fn try_new(api_key: &str, model: &str) -> Result<Self> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(Error::validation(
                "Jev API key is empty",
                Some("api_key".to_string()),
            ));
        }
        if !api_key.chars().all(|c| c.is_ascii_graphic()) {
            return Err(Error::validation(
                "Jev API key must be printable ASCII without whitespace",
                Some("api_key".to_string()),
            ));
        }
        let model = model.trim().to_string();
        if model.is_empty() {
            return Err(Error::validation(
                "Jev model is empty",
                Some("model".to_string()),
            ));
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|_| {
                Error::validation(
                    "Jev API key contains characters not allowed in an HTTP header",
                    Some("api_key".to_string()),
                )
            })?;
        authorization.set_sensitive(true);
        Ok(Self {
            http: http_client(DEFAULT_BASE_URL)?,
            authorization,
            base_url: DEFAULT_BASE_URL.to_string(),
            model,
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_budget: DEFAULT_RETRY_BUDGET,
        })
    }

    /// Create a client from `TYPESAFE_API_KEY` and `TYPESAFE_DEFAULT_MODEL`, both required, and
    /// the optional `TYPESAFE_BASE_URL`. A blank variable counts as unset, and a bad base URL is a
    /// validation error naming the variable.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(|name| std::env::var(name).ok())
    }

    pub(super) fn from_env_with(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let var = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        let missing = |name: &str| {
            Error::validation(
                format!("{name} is not set or empty"),
                Some(name.to_string()),
            )
        };
        let api_key = var(API_KEY_ENV).ok_or_else(|| missing(API_KEY_ENV))?;
        let model = var(MODEL_ENV).ok_or_else(|| missing(MODEL_ENV))?;
        let client = Self::try_new(&api_key, &model)?;
        match var(BASE_URL_ENV) {
            Some(base_url) => client.with_base_url_from(BASE_URL_ENV, &base_url),
            None => Ok(client),
        }
    }

    /// Send requests to another base URL, such as a staging host or a local test server.
    ///
    /// The URL must use https; plain http is accepted only for loopback hosts. Credentials, a
    /// query or a fragment in the URL are rejected. A trailing slash is trimmed.
    pub fn with_base_url(self, base_url: &str) -> Result<Self> {
        self.with_base_url_from("base_url", base_url)
    }

    /// [`JevClient::with_base_url`] with `setting` naming where the URL came from in the
    /// validation error.
    fn with_base_url_from(mut self, setting: &str, base_url: &str) -> Result<Self> {
        self.base_url = base_url::checked_base_url("Jev", setting, base_url)?;
        // A loopback base URL bypasses proxies, so the HTTP client depends on it too.
        self.http = http_client(&self.base_url)?;
        Ok(self)
    }

    /// Set the time limit for one attempt, connecting through reading the whole body.
    /// Defaults to 10 seconds. Each attempt is also cut short by the remaining retry budget.
    #[must_use]
    pub fn with_timeout(mut self, per_attempt: Duration) -> Self {
        self.timeout = per_attempt;
        self
    }

    /// Set how many times a retryable failure is retried. Defaults to 2; 0 sends exactly once.
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the total time [`JevClient::ask`] may spend across attempts and the waits between
    /// them. Defaults to 30 seconds. A retry whose wait would pass the budget is not made.
    #[must_use]
    pub fn with_retry_budget(mut self, total: Duration) -> Self {
        self.retry_budget = total;
        self
    }

    /// The model id sent with every request. The served model in [`JevResponse::model`] can
    /// differ when this is an alias.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Ask every question in `questions` about `state` in one billed request.
    ///
    /// `state` must be a string, object or array nested at most 64 deep, and the request body
    /// at most 32 MiB; the questions are checked as described on [`Questions`]. A failed check
    /// sends nothing. Responses 408, 429 and every 5xx, connection failures and timeouts are
    /// retried, honoring `Retry-After`; a `Retry-After` over 60 seconds, or a wait that would
    /// pass the retry budget, returns the error at once.
    pub async fn ask(&self, state: impl Into<Value>, questions: &Questions) -> Result<JevResponse> {
        let state: Value = state.into();
        questions.validate()?;
        validate_state(&state)?;
        let request_body = serde_json::to_vec(&WireRequest {
            model: &self.model,
            state: &state,
            questions: WireQuestions(&questions.items),
        })
        .map_err(|_| Error::serialization("Failed to serialize the Jev request", None))?;
        if request_body.len() > MAX_BODY_BYTES {
            return Err(Error::validation(
                format!(
                    "Jev request body is {} bytes, over the 32 MiB limit",
                    request_body.len()
                ),
                Some("state".to_string()),
            ));
        }
        let request_body = Bytes::from(request_body);
        let url = format!("{}{PATH}", self.base_url);
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            let remaining = self.retry_budget.saturating_sub(started.elapsed());
            let timeout = self.timeout.min(remaining);
            let error = match self.send_once(&url, &request_body, timeout, attempt).await {
                Ok((status, request_id, response_body)) => {
                    tracing::debug!(
                        target: "topcoat_si::jev",
                        status,
                        attempt = attempt + 1,
                        request_id = request_id.as_deref(),
                        model = %self.model,
                        latency_seconds = started.elapsed().as_secs_f64(),
                        "Jev answered"
                    );
                    return JevResponse::parse(
                        request_body,
                        response_body,
                        request_id,
                        attempt + 1,
                    );
                }
                Err(error) => error,
            };
            let retry_after = error.retry_after();
            let delay = retry_after.unwrap_or_else(|| backoff::next(attempt));
            let give_up = attempt >= self.max_retries
                || !is_retryable(&error)
                || retry_after.is_some_and(|d| d > MAX_RETRY_AFTER)
                || started.elapsed() + delay >= self.retry_budget;
            if give_up {
                tracing::debug!(
                    target: "topcoat_si::jev",
                    status = error.status_code(),
                    attempt = attempt + 1,
                    request_id = error.request_id(),
                    model = %self.model,
                    latency_seconds = started.elapsed().as_secs_f64(),
                    "Jev request failed"
                );
                return Err(error);
            }
            tracing::debug!(
                target: "topcoat_si::jev",
                status = error.status_code(),
                attempt = attempt + 1,
                request_id = error.request_id(),
                model = %self.model,
                sleep_seconds = delay.as_secs_f64(),
                "Retrying Jev request"
            );
            sleep(delay).await;
            attempt += 1;
        }
    }

    /// Ask one choice question over the options of `T` and gate the pick on its confidence.
    ///
    /// Confidence measures how concentrated the answer is, not whether it is correct.
    pub async fn choose<T: Choices>(
        &self,
        state: impl Into<Value>,
        instructions: impl Into<String>,
        min_confidence: f64,
    ) -> Result<Outcome<T>> {
        let questions = Questions::new().choice_of::<T>(SHORTCUT_QUESTION, instructions);
        self.ask(state, &questions)
            .await?
            .choose(SHORTCUT_QUESTION, min_confidence)
    }

    /// Ask one yes/no question and gate the answer on its certainty, `|2p - 1|`.
    pub async fn yes_no(
        &self,
        state: impl Into<Value>,
        instructions: impl Into<String>,
        min_certainty: f64,
    ) -> Result<Outcome<bool>> {
        let questions = Questions::new().noul(SHORTCUT_QUESTION, instructions);
        self.ask(state, &questions)
            .await?
            .yes_no(SHORTCUT_QUESTION, min_certainty)
    }

    /// Ask one score question over the levels of `T` and gate the most likely level on the
    /// answer's confidence.
    ///
    /// Confidence measures how concentrated the answer is, not whether it is correct.
    pub async fn rate<T: Levels>(
        &self,
        state: impl Into<Value>,
        instructions: impl Into<String>,
        min_confidence: f64,
    ) -> Result<Outcome<T>> {
        let questions = Questions::new().score_of::<T>(SHORTCUT_QUESTION, instructions);
        self.ask(state, &questions)
            .await?
            .rate(SHORTCUT_QUESTION, min_confidence)
    }

    async fn send_once(
        &self,
        url: &str,
        body: &Bytes,
        timeout: Duration,
        attempt: usize,
    ) -> Result<(u16, Option<String>, Bytes)> {
        let mut request = self
            .http
            .post(url)
            .timeout(timeout)
            .header(AUTHORIZATION, self.authorization.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body(body.clone());
        if attempt > 0 {
            request = request.header("x-typesafe-retry-count", attempt.to_string());
        }
        let response = request
            .send()
            .await
            .map_err(|e| map_reqwest_error(e, RequestPhase::Headers))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let request_id = request_id(&headers);
        // Headers arrived, so a body failure keeps the real status and the request id.
        let bytes = response.bytes().await.map_err(|e| {
            map_reqwest_error(e, RequestPhase::Body).with_http_metadata(HttpMetadata {
                status,
                error_code: None,
                request_id: request_id.clone(),
                retry_after: retry_after_from_headers(&headers),
            })
        })?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &headers, &bytes, request_id));
        }
        Ok((status, request_id, bytes))
    }
}

/// Builds the HTTP client for `base_url`, which decides whether proxies are used (see
/// [`base_url::with_proxy_rule`]).
fn http_client(base_url: &str) -> Result<ReqwestClient> {
    let builder = ReqwestClient::builder()
        .redirect(redirect::Policy::none())
        .user_agent(USER_AGENT);
    base_url::with_proxy_rule(builder, base_url)
        .build()
        .map_err(|e| {
            Error::http_client(
                format!("Failed to build HTTP client: {e}"),
                Some(Box::new(e)),
            )
        })
}

/// Jev's transient statuses (408, 429, every 5xx), connection failures and timeouts.
fn is_retryable(error: &Error) -> bool {
    matches!(error, Error::Connection { .. } | Error::Timeout { .. })
        || matches!(error.status_code(), Some(408 | 429 | 500..=599))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn request_id(headers: &HeaderMap) -> Option<String> {
    header_str(headers, "x-typesafe-request-id")
        .or_else(|| header_str(headers, "cf-ray"))
        .map(String::from)
}

// Only the JSON `detail` reaches the message: a string, an object's `message`, or a 422 list
// as `loc: msg` pairs. A 422 item's `input` echoes the caller's data and is never read.
fn http_error(status: u16, headers: &HeaderMap, body: &[u8], request_id: Option<String>) -> Error {
    let mut error_code = None;
    let detail = match serde_json::from_slice::<Value>(body).ok() {
        None if status == 403 => {
            error_code = Some(JevErrorCode::EdgeBlocked);
            Some(match header_str(headers, "cf-ray") {
                Some(ray) => format!("blocked by an edge proxy (cf-ray {ray})"),
                None => "blocked by an edge proxy".to_string(),
            })
        }
        None => None,
        Some(value) => match value.get("detail") {
            Some(Value::String(detail)) => Some(detail.clone()),
            Some(Value::Object(detail)) => {
                error_code = detail
                    .get("error_type")
                    .and_then(Value::as_str)
                    .map(JevErrorCode::from_detail);
                detail
                    .get("message")
                    .and_then(Value::as_str)
                    .map(String::from)
            }
            Some(Value::Array(items)) => {
                Some(validation_detail(items)).filter(|detail| !detail.is_empty())
            }
            _ => None,
        },
    };
    let detail = if error_code == Some(JevErrorCode::MaxTokensExceeded) {
        Some(match detail {
            Some(detail) => format!("input too long: {detail}"),
            None => "input too long".to_string(),
        })
    } else {
        detail
    };
    let message = match detail {
        Some(detail) => format!("Jev returned {status}: {detail}"),
        None => format!("Jev returned {status}"),
    };
    let metadata = HttpMetadata {
        status,
        error_code: error_code.map(ErrorCode::Jev),
        request_id,
        retry_after: retry_after_from_headers(headers),
    };
    Error::from_http(truncate(&message, MAX_ERROR_CHARS), metadata, None)
}

fn validation_detail(items: &[Value]) -> String {
    let mut details: Vec<String> = Vec::new();
    for item in items {
        let Some(message) = item.get("msg").and_then(Value::as_str) else {
            continue;
        };
        let mut parts: Vec<String> = Vec::new();
        if let Some(path) = item.get("loc").and_then(Value::as_array) {
            for part in path {
                match part {
                    Value::String(part) => parts.push(part.clone()),
                    other => parts.push(other.to_string()),
                }
            }
        }
        // Checked after joining: a location of only empty strings counts as none.
        let location = parts.join(".");
        if location.is_empty() {
            details.push(message.to_string());
        } else {
            details.push(format!("{location}: {message}"));
        }
    }
    details.join("; ")
}

fn truncate(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => text[..end].to_string(),
        None => text.to_string(),
    }
}

fn validate_state(state: &Value) -> Result<()> {
    let invalid = |message: &str| Err(Error::validation(message, Some("state".to_string())));
    if !matches!(state, Value::String(_) | Value::Object(_) | Value::Array(_)) {
        return invalid("Jev state must be a string, object or array");
    }
    // Walked with an explicit stack so a hostile nesting depth cannot overflow ours.
    let mut stack = vec![(state, 1_usize)];
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_STATE_DEPTH {
            return invalid("Jev state is nested more than 64 levels deep");
        }
        let is_container = |v: &&Value| matches!(v, Value::Object(_) | Value::Array(_));
        match value {
            Value::Array(items) => stack.extend(
                items
                    .iter()
                    .filter(is_container)
                    .map(|item| (item, depth + 1)),
            ),
            Value::Object(map) => stack.extend(
                map.values()
                    .filter(is_container)
                    .map(|item| (item, depth + 1)),
            ),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct WireRequest<'request> {
    model: &'request str,
    state: &'request Value,
    questions: WireQuestions<'request>,
}
