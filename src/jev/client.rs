//! The Jev HTTP client.

use std::fmt;
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client as ReqwestClient, ClientBuilder};
use serde::Serialize;
use serde_json::Value;

use super::questions::WireQuestions;
use super::{Choices, JevErrorCode, JevResponse, Levels, Outcome, Questions};
use crate::backoff::{self, RetryPolicy};
use crate::error::{Error, ErrorCode, Result, Service};
use crate::http::{self, header_str};
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

/// A request body that passed every check [`JevClient::ask`] makes before sending.
pub(crate) struct Prepared(Bytes);

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
        let authorization = http::bearer(Service::Jev, api_key)?;
        let model = model.trim().to_string();
        if model.is_empty() {
            return Err(Error::validation(
                "Jev model is empty",
                Some("model".to_string()),
            ));
        }
        Ok(Self {
            http: http::client(DEFAULT_BASE_URL, builder())?,
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
        (self.base_url, self.http) = http::rebase(Service::Jev, setting, base_url, builder())?;
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
    /// sends nothing. Whatever [`Error::is_retryable`] accepts (408, 409, 429, 5xx, connection
    /// failures, timeouts) is retried, except `Interrupted` (the response started, then stopped),
    /// because Jev has already answered and may have billed. Each wait is the backoff delay or
    /// `Retry-After`, whichever is longer; a `Retry-After` over 60 seconds, or a wait that would
    /// pass the retry budget, returns the error at once.
    pub async fn ask(&self, state: impl Into<Value>, questions: &Questions) -> Result<JevResponse> {
        let request = self.prepare(state.into(), questions)?;
        self.send(request).await
    }

    /// Checks the state and questions and serializes the request body, as [`JevClient::ask`]
    /// does before sending.
    pub(crate) fn prepare(&self, state: Value, questions: &Questions) -> Result<Prepared> {
        questions.validate()?;
        validate_state(&state)?;
        let request_body = serialize_body(&WireRequest {
            model: &self.model,
            state: &state,
            questions: WireQuestions(&questions.items),
        })?;
        Ok(Prepared(Bytes::from(request_body)))
    }

    /// Sends a checked request, retrying as [`JevClient::ask`] describes.
    pub(crate) async fn send(&self, request: Prepared) -> Result<JevResponse> {
        let Prepared(request_body) = request;
        let url = format!("{}{PATH}", self.base_url);
        let started = Instant::now();
        let policy = RetryPolicy {
            max_retries: self.max_retries,
            max_retry_after: MAX_RETRY_AFTER,
            budget: Some(self.retry_budget),
            resend_timeouts: true,
            resend_interrupted: false,
        };
        let retried = backoff::retry(
            &policy,
            |attempt, remaining| {
                let timeout = self.timeout.min(remaining.unwrap_or(self.retry_budget));
                self.send_once(&url, &request_body, timeout, attempt)
            },
            |error, attempt, delay| {
                tracing::debug!(
                    target: "topcoat_si::jev",
                    status = error.status(),
                    attempt = attempt + 1,
                    request_id = error.request_id(),
                    model = %self.model,
                    sleep_seconds = delay.as_secs_f64(),
                    "Retrying Jev request"
                );
            },
        )
        .await;
        match retried.result {
            Ok((status, request_id, response_body)) => {
                tracing::debug!(
                    target: "topcoat_si::jev",
                    status,
                    attempt = retried.attempts,
                    request_id = request_id.as_deref(),
                    model = %self.model,
                    latency_seconds = started.elapsed().as_secs_f64(),
                    "Jev answered"
                );
                JevResponse::parse(request_body, response_body, request_id, retried.attempts)
            }
            Err(error) => {
                tracing::debug!(
                    target: "topcoat_si::jev",
                    status = error.status(),
                    attempt = retried.attempts,
                    request_id = error.request_id(),
                    model = %self.model,
                    latency_seconds = started.elapsed().as_secs_f64(),
                    "Jev request failed"
                );
                Err(error)
            }
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
        let reply = http::send(request, Service::Jev, request_id).await?;
        if !reply.is_success() {
            return Err(http_error(
                reply.status,
                &reply.headers,
                &reply.body,
                reply.request_id,
            ));
        }
        Ok((reply.status, reply.request_id, reply.body))
    }
}

/// The Jev settings for [`http::client`].
fn builder() -> ClientBuilder {
    ReqwestClient::builder().user_agent(USER_AGENT)
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
    // Display already shows the service and status, so the message is Jev's own words.
    Error::http_status(
        Service::Jev,
        status,
        detail.map(|detail| http::truncate_chars(&detail, MAX_ERROR_CHARS, "")),
        None,
        error_code.map(ErrorCode::Jev),
        request_id,
        retry_after_from_headers(headers),
    )
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

/// Serializes the request, keeping the serde error as the source, and enforces the body cap.
pub(super) fn serialize_body(request: &impl Serialize) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(request).map_err(|e| {
        Error::validation("Failed to serialize the Jev request", None).with_source(e)
    })?;
    if body.len() > MAX_BODY_BYTES {
        return Err(Error::request_too_large(
            format!(
                "Jev request body is {} bytes, over the 32 MiB limit",
                body.len()
            ),
            body.len(),
            MAX_BODY_BYTES,
        ));
    }
    Ok(body)
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
