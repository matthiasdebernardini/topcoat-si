//! Typed judgments from TypeSafe AI's Jev decision model.
//!
//! [`JevClient::ask`] sends one state and a set of named [`Questions`] to `POST /v1/systemone`
//! and returns every answer in a [`JevResponse`]. A question is a noul (yes or no), a choice
//! (pick one label) or a score (rate on an ordered scale). [`JevTool`] exposes the client to
//! Claude as a client-side `jev` tool.
//!
//! Probabilities come back rounded to hundredths, so they need not sum to 1 and the reported
//! choice can trail another label by 0.01. This client never rejects an answer over that.
//! Confidence measures how concentrated an answer is, not whether it is correct.
//!
//! ```no_run
//! # async fn run() -> topcoat_si::Result<()> {
//! use topcoat_si::jev::{JevClient, Outcome, Questions};
//!
//! let jev = JevClient::new("ts-key", "jev-1.13.0")?;
//! let questions = Questions::new()
//!     .noul("refund", "Does the customer ask for a refund?")
//!     .choice(
//!         "route",
//!         "Which team should handle the message?",
//!         [("billing", Some("Charges and refunds")), ("other", None)],
//!     );
//! let response = jev
//!     .ask("I was charged twice, please refund one.", &questions)
//!     .await?;
//! if let Outcome::Answered(true) = response.yes_no("refund", 0.8)? {
//!     println!("route to {}", response.choice("route")?.choice);
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::HashSet;
use std::time::{Duration, Instant};

use bytes::Bytes;
use reqwest::Client as ReqwestClient;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use tokio::time::sleep;

use crate::backoff::ExponentialBackoff;
use crate::error::{Error, HttpMetadata, RequestPhase, Result, map_reqwest_error};
use crate::types::retry_after_from_headers;

mod tool;

#[cfg(test)]
mod tests;

pub use tool::JevTool;

const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
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
const MAX_CHOICE_OPTIONS: usize = 255;
const MAX_SCORE_LEVELS: usize = 10;
// The question name the one-question shortcuts send. Jev never sees names.
const SHORTCUT_QUESTION: &str = "answer";

/// Client for Jev's `POST /v1/systemone`.
///
/// The API key is only kept as a sensitive header value and is redacted from `Debug`. There is
/// no default model: answer thresholds are tuned per model and aliases such as `jev-latest`
/// move, so pin a versioned id such as `jev-1.13.0`.
#[derive(Clone)]
pub struct JevClient {
    http: ReqwestClient,
    authorization: HeaderValue,
    base_url: String,
    model: String,
    timeout: Duration,
    max_retries: usize,
    retry_budget: Duration,
}

impl std::fmt::Debug for JevClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
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
        let model = model.into().trim().to_string();
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
        let http = ReqwestClient::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| {
                Error::http_client(
                    format!("Failed to build HTTP client: {e}"),
                    Some(Box::new(e)),
                )
            })?;
        Ok(Self {
            http,
            authorization,
            base_url: DEFAULT_BASE_URL.to_string(),
            model,
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_budget: DEFAULT_RETRY_BUDGET,
        })
    }

    /// Create a client from `TYPESAFE_API_KEY` and `TYPESAFE_DEFAULT_MODEL`, both required, and
    /// the optional `TYPESAFE_BASE_URL`. A blank variable counts as unset.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(|name| std::env::var(name).ok())
    }

    fn from_env_with(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let var = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        let missing = |name: &str| {
            Error::validation(
                format!("{name} is not set or empty"),
                Some(name.to_string()),
            )
        };
        let api_key = var(API_KEY_ENV).ok_or_else(|| missing(API_KEY_ENV))?;
        let model = var(MODEL_ENV).ok_or_else(|| missing(MODEL_ENV))?;
        let client = Self::new(api_key, model)?;
        match var(BASE_URL_ENV) {
            Some(base_url) => client.with_base_url(base_url),
            None => Ok(client),
        }
    }

    /// Send requests to another base URL, such as a staging host or a local test server.
    ///
    /// The URL must use https; plain http is accepted only for loopback hosts. Credentials, a
    /// query or a fragment in the URL are rejected. A trailing slash is trimmed.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into();
        let base_url = base_url.trim().trim_end_matches('/');
        let invalid = |message: &str| Error::validation(message, Some("base_url".to_string()));
        let parsed =
            url::Url::parse(base_url).map_err(|_| invalid("Jev base URL is not a valid URL"))?;
        let loopback = match parsed.host() {
            Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        match parsed.scheme() {
            "https" if parsed.host().is_some() => {}
            "http" if loopback => {}
            _ => {
                return Err(invalid(
                    "Jev base URL must use https, or http only for a loopback host",
                ));
            }
        }
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(invalid(
                "Jev base URL must not carry credentials, a query or a fragment",
            ));
        }
        self.base_url = base_url.to_string();
        Ok(self)
    }

    /// Set the time limit for one attempt, connecting through reading the whole body.
    /// Defaults to 10 seconds. Each attempt is also cut short by the remaining retry budget.
    pub fn with_timeout(mut self, per_attempt: Duration) -> Self {
        self.timeout = per_attempt;
        self
    }

    /// Set how many times a retryable failure is retried. Defaults to 2; 0 sends exactly once.
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the total time [`JevClient::ask`] may spend across attempts and the waits between
    /// them. Defaults to 30 seconds. A retry whose wait would pass the budget is not made.
    pub fn with_retry_budget(mut self, total: Duration) -> Self {
        self.retry_budget = total;
        self
    }

    /// The model id sent with every request. The served model in [`JevResponse::model`] can
    /// differ when this is an alias.
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
        let state = state.into();
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
            let delay = retry_after.unwrap_or_else(|| ExponentialBackoff::next(attempt));
            let give_up = attempt >= self.max_retries
                || !is_retryable(&error)
                || retry_after.is_some_and(|d| d > MAX_RETRY_AFTER)
                || started.elapsed() + delay >= self.retry_budget;
            if give_up {
                tracing::debug!(
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
                error_type: None,
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
    let mut error_type = None;
    let detail = match serde_json::from_slice::<Value>(body).ok() {
        None if status == 403 => {
            error_type = Some("edge_blocked".to_string());
            Some(match header_str(headers, "cf-ray") {
                Some(ray) => format!("blocked by an edge proxy (cf-ray {ray})"),
                None => "blocked by an edge proxy".to_string(),
            })
        }
        None => None,
        Some(value) => match value.get("detail") {
            Some(Value::String(detail)) => Some(detail.clone()),
            Some(Value::Object(detail)) => {
                error_type = detail
                    .get("error_type")
                    .and_then(Value::as_str)
                    .map(String::from);
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
    let detail = if error_type.as_deref() == Some("max_tokens_exceeded") {
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
        error_type,
        request_id,
        retry_after: retry_after_from_headers(headers),
    };
    Error::from_http(truncate(&message, MAX_ERROR_CHARS), metadata, None)
}

fn validation_detail(items: &[Value]) -> String {
    items
        .iter()
        .filter_map(|item| {
            let msg = item.get("msg")?.as_str()?;
            let loc = item
                .get("loc")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .map(|part| match part {
                            Value::String(part) => part.clone(),
                            other => other.to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(".")
                })
                .unwrap_or_default();
            Some(if loc.is_empty() {
                msg.to_string()
            } else {
                format!("{loc}: {msg}")
            })
        })
        .collect::<Vec<_>>()
        .join("; ")
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

/// An enum Jev can pick with a choice question. Jev slightly favors options listed first.
pub trait Choices: Copy + PartialEq + 'static {
    /// Every option in the order Jev sees them: value, label, optional description. Labels unique.
    const OPTIONS: &'static [(Self, &'static str, Option<&'static str>)];
}

/// An enum for a score question's ordered levels.
pub trait Levels: Copy + PartialEq + 'static {
    /// Every level, lowest first: value and the text Jev sees.
    const LEVELS: &'static [(Self, &'static str)];
}

/// A gated answer: confident enough, or the best guess with its confidence.
///
/// Confidence measures how concentrated the answer is, not whether it is correct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome<T> {
    /// The answer met the threshold.
    Answered(T),
    /// The answer fell below the threshold.
    Unsure {
        /// The most likely answer.
        best: T,
        /// The confidence (or, for yes/no, the certainty) that fell short.
        confidence: f64,
    },
}

impl<T> Outcome<T> {
    /// The answer when it met the threshold, `None` when unsure.
    pub fn answered(self) -> Option<T> {
        match self {
            Self::Answered(value) => Some(value),
            Self::Unsure { .. } => None,
        }
    }
}

fn gate<T>(value: T, confidence: f64, min_confidence: f64) -> Outcome<T> {
    if confidence >= min_confidence {
        Outcome::Answered(value)
    } else {
        Outcome::Unsure {
            best: value,
            confidence,
        }
    }
}

/// The named questions for one [`JevClient::ask`], kept in the order they were added.
///
/// The chain never fails; [`JevClient::ask`] checks it before sending: at least one question,
/// names non-empty and unique, a choice has 2 to 255 options with non-empty unique labels, and a
/// score has 2 to 10 non-empty levels. Jev never sees the names, only the instructions and
/// options.
#[derive(Debug, Clone, Default)]
pub struct Questions {
    items: Vec<(String, Question)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Question {
    Noul {
        instructions: String,
    },
    Choice {
        instructions: String,
        #[serde(serialize_with = "criteria_map")]
        criteria: Vec<(String, Option<String>)>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

fn criteria_map<S: Serializer>(
    options: &[(String, Option<String>)],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(options.len()))?;
    for (label, description) in options {
        map.serialize_entry(label, description)?;
    }
    map.end()
}

impl Questions {
    /// An empty set of questions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a yes/no question. Its answer is the probability of yes.
    pub fn noul(mut self, name: impl Into<String>, instructions: impl Into<String>) -> Self {
        self.items.push((
            name.into(),
            Question::Noul {
                instructions: instructions.into(),
            },
        ));
        self
    }

    /// Add a pick-one question over `(label, description)` options, in the order given.
    ///
    /// Jev slightly favors options listed first. Labels and descriptions may be `&str` or
    /// `String`; a list where every description is `None` needs a type, as in
    /// `None::<&str>`.
    pub fn choice<L, D>(
        mut self,
        name: impl Into<String>,
        instructions: impl Into<String>,
        options: impl IntoIterator<Item = (L, Option<D>)>,
    ) -> Self
    where
        L: Into<String>,
        D: Into<String>,
    {
        let criteria = options
            .into_iter()
            .map(|(label, description)| (label.into(), description.map(Into::into)))
            .collect();
        self.items.push((
            name.into(),
            Question::Choice {
                instructions: instructions.into(),
                criteria,
            },
        ));
        self
    }

    /// Add a question rating the state on `levels`, lowest first.
    pub fn score(
        mut self,
        name: impl Into<String>,
        instructions: impl Into<String>,
        levels: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.items.push((
            name.into(),
            Question::Score {
                instructions: instructions.into(),
                criteria: levels.into_iter().map(Into::into).collect(),
            },
        ));
        self
    }

    /// Add a choice question over the options of `T`, read back with [`JevResponse::choose`].
    pub fn choice_of<T: Choices>(
        self,
        name: impl Into<String>,
        instructions: impl Into<String>,
    ) -> Self {
        self.choice(
            name,
            instructions,
            T::OPTIONS
                .iter()
                .map(|(_, label, description)| (*label, *description)),
        )
    }

    /// Add a score question over the levels of `T`, read back with [`JevResponse::rate`].
    pub fn score_of<T: Levels>(
        self,
        name: impl Into<String>,
        instructions: impl Into<String>,
    ) -> Self {
        self.score(name, instructions, T::LEVELS.iter().map(|(_, text)| *text))
    }

    fn validate(&self) -> Result<()> {
        let invalid =
            |message: String, param: &str| Err(Error::validation(message, Some(param.to_string())));
        if self.items.is_empty() {
            return invalid("Jev needs at least one question".to_string(), "questions");
        }
        let mut names = HashSet::new();
        for (name, question) in &self.items {
            if name.trim().is_empty() {
                return invalid(
                    "Jev question names must not be empty".to_string(),
                    "questions",
                );
            }
            if !names.insert(name.as_str()) {
                return invalid(format!("Jev question name {name:?} is used twice"), name);
            }
            match question {
                Question::Noul { .. } => {}
                Question::Choice { criteria, .. } => {
                    if !(2..=MAX_CHOICE_OPTIONS).contains(&criteria.len()) {
                        return invalid(
                            format!(
                                "choice question {name:?} needs 2 to {MAX_CHOICE_OPTIONS} options, \
                                 got {}",
                                criteria.len()
                            ),
                            name,
                        );
                    }
                    let mut labels = HashSet::new();
                    for (label, _) in criteria {
                        if label.trim().is_empty() {
                            return invalid(
                                format!("choice question {name:?} has an empty label"),
                                name,
                            );
                        }
                        if !labels.insert(label.as_str()) {
                            return invalid(
                                format!("choice question {name:?} lists label {label:?} twice"),
                                name,
                            );
                        }
                    }
                }
                Question::Score { criteria, .. } => {
                    if !(2..=MAX_SCORE_LEVELS).contains(&criteria.len()) {
                        return invalid(
                            format!(
                                "score question {name:?} needs 2 to {MAX_SCORE_LEVELS} levels, \
                                 got {}",
                                criteria.len()
                            ),
                            name,
                        );
                    }
                    if criteria.iter().any(|level| level.trim().is_empty()) {
                        return invalid(
                            format!("score question {name:?} has an empty level"),
                            name,
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    state: &'a Value,
    questions: WireQuestions<'a>,
}

struct WireQuestions<'a>(&'a [(String, Question)]);

impl Serialize for WireQuestions<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, question) in self.0 {
            map.serialize_entry(name, question)?;
        }
        map.end()
    }
}

/// Tokens one request consumed. Either count may be missing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct JevUsage {
    /// Input tokens.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Output tokens.
    #[serde(default)]
    pub output_tokens: Option<u64>,
}

/// One answer, by question kind.
///
/// `Debug` prints the byte length of an unknown or malformed answer, not its JSON.
#[derive(Clone, PartialEq)]
pub enum Answer {
    /// A yes/no answer: the probability of yes, rounded to hundredths.
    Noul(f64),
    /// A pick-one answer.
    Choice(ChoiceAnswer),
    /// A rating on an ordered scale.
    Score(ScoreAnswer),
    /// An answer type this client does not know. It is kept so the response is never lost; the
    /// typed accessors report it as an error.
    Unknown {
        /// The answer's `type` field, empty when missing.
        kind: String,
        /// The answer as Jev sent it.
        raw: Value,
    },
    /// A noul, choice or score answer whose fields could not be decoded. It is kept so the
    /// other answers stay usable; the typed accessors report it as an error.
    Malformed {
        /// The answer's `type` field: `noul`, `choice` or `score`.
        kind: String,
        /// The answer as Jev sent it.
        raw: Value,
    },
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Noul(p) => f.debug_tuple("Noul").field(p).finish(),
            Self::Choice(choice) => f.debug_tuple("Choice").field(choice).finish(),
            Self::Score(score) => f.debug_tuple("Score").field(score).finish(),
            Self::Unknown { kind, raw } => f
                .debug_struct("Unknown")
                .field("kind", kind)
                .field("raw_bytes", &raw.to_string().len())
                .finish(),
            Self::Malformed { kind, raw } => f
                .debug_struct("Malformed")
                .field("kind", kind)
                .field("raw_bytes", &raw.to_string().len())
                .finish(),
        }
    }
}

/// A choice answer. Probabilities are rounded to hundredths, so `choice` can trail another
/// label by 0.01 and the probabilities need not sum to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceAnswer {
    /// The picked label.
    pub choice: String,
    /// How concentrated the probabilities are, 0 to 1. Not a measure of correctness.
    pub confidence: f64,
    /// Probability per label, in the order Jev sent them.
    pub probabilities: Vec<(String, f64)>,
}

/// A score answer. Probabilities are rounded to hundredths and need not sum to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreAnswer {
    /// The probability-weighted mean level, fractional, counting the lowest level as 0.
    pub score: f64,
    /// How concentrated the probabilities are, 0 to 1. Not a measure of correctness.
    pub confidence: f64,
    /// Level index (`"0"`, `"1"`, ...) to the level as Jev echoed it.
    pub legend: serde_json::Map<String, Value>,
    /// Probability per level, lowest first.
    pub probabilities: Vec<f64>,
}

impl ScoreAnswer {
    /// The index of the most likely level, ties going to the lower level. `None` without
    /// probabilities.
    pub fn mode(&self) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (index, p) in self.probabilities.iter().copied().enumerate() {
            if best.is_none_or(|(_, top)| p > top) {
                best = Some((index, p));
            }
        }
        best.map(|(index, _)| index)
    }
}

/// Every answer from one [`JevClient::ask`], with the exact bytes sent and received.
///
/// `Debug` omits the bodies, which carry the state.
#[derive(Clone)]
pub struct JevResponse {
    /// The model that answered. It can differ from the alias that was sent.
    pub model: String,
    /// Answers by question name, in the order Jev sent them.
    pub answers: Vec<(String, Answer)>,
    /// Tokens consumed.
    pub usage: JevUsage,
    /// `x-typesafe-request-id`, or `cf-ray` when that is missing.
    pub request_id: Option<String>,
    /// Attempts made, 1 when the first succeeded.
    pub attempts: usize,
    /// The request body as sent, for replay and audit.
    pub request_body: Bytes,
    /// The response body as received.
    pub response_body: Bytes,
}

impl std::fmt::Debug for JevResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JevResponse")
            .field("model", &self.model)
            .field("answers", &self.answers)
            .field("usage", &self.usage)
            .field("request_id", &self.request_id)
            .field("attempts", &self.attempts)
            .field("request_body_bytes", &self.request_body.len())
            .field("response_body_bytes", &self.response_body.len())
            .finish()
    }
}

#[derive(Deserialize)]
struct WireResponse {
    model: String,
    answers: serde_json::Map<String, Value>,
    #[serde(default)]
    usage: Option<JevUsage>,
}

#[derive(Deserialize)]
struct WireNoul {
    noul: f64,
}

#[derive(Deserialize)]
struct WireChoice {
    choice: String,
    confidence: f64,
    probabilities: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct WireScore {
    score: f64,
    confidence: f64,
    // A missing or null legend is empty.
    #[serde(default)]
    legend: Option<serde_json::Map<String, Value>>,
    probabilities: serde_json::Map<String, Value>,
}

fn parse_answer(value: Value) -> Answer {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !matches!(kind.as_str(), "noul" | "choice" | "score") {
        return Answer::Unknown { kind, raw: value };
    }
    match decode_answer(&kind, &value) {
        Some(answer) => answer,
        None => Answer::Malformed { kind, raw: value },
    }
}

fn decode_answer(kind: &str, value: &Value) -> Option<Answer> {
    match kind {
        "noul" => {
            let wire = WireNoul::deserialize(value).ok()?;
            Some(Answer::Noul(wire.noul))
        }
        "choice" => {
            let wire = WireChoice::deserialize(value).ok()?;
            let probabilities = wire
                .probabilities
                .into_iter()
                .map(|(label, p)| Some((label, p.as_f64()?)))
                .collect::<Option<Vec<_>>>()?;
            Some(Answer::Choice(ChoiceAnswer {
                choice: wire.choice,
                confidence: wire.confidence,
                probabilities,
            }))
        }
        "score" => {
            let wire = WireScore::deserialize(value).ok()?;
            // Keys must be the level indexes 0..n in any order.
            let mut probabilities = vec![None; wire.probabilities.len()];
            for (key, p) in &wire.probabilities {
                let slot = probabilities.get_mut(key.parse::<usize>().ok()?)?;
                if slot.is_some() {
                    return None;
                }
                *slot = Some(p.as_f64()?);
            }
            Some(Answer::Score(ScoreAnswer {
                score: wire.score,
                confidence: wire.confidence,
                legend: wire.legend.unwrap_or_default(),
                probabilities: probabilities.into_iter().collect::<Option<Vec<_>>>()?,
            }))
        }
        _ => None,
    }
}

impl JevResponse {
    fn parse(
        request_body: Bytes,
        response_body: Bytes,
        request_id: Option<String>,
        attempts: usize,
    ) -> Result<Self> {
        let parsed = serde_json::from_slice::<WireResponse>(&response_body)
            .ok()
            .map(|wire| {
                let answers = wire
                    .answers
                    .into_iter()
                    .map(|(name, value)| (name, parse_answer(value)))
                    .collect::<Vec<_>>();
                (wire.model, answers, wire.usage.unwrap_or_default())
            });
        let Some((model, answers, usage)) = parsed else {
            let id = request_id
                .as_deref()
                .map(|id| format!(", request id {id}"))
                .unwrap_or_default();
            return Err(Error::serialization(
                format!(
                    "Failed to parse Jev response ({} bytes{id})",
                    response_body.len()
                ),
                None,
            ));
        };
        Ok(Self {
            model,
            answers,
            usage,
            request_id,
            attempts,
            request_body,
            response_body,
        })
    }

    fn answer(&self, name: &str) -> Result<&Answer> {
        let answer = self
            .answers
            .iter()
            .find(|(answer_name, _)| answer_name == name)
            .map(|(_, answer)| answer)
            .ok_or_else(|| {
                Error::validation(
                    format!("Jev returned no answer for question {name:?}"),
                    Some(name.to_string()),
                )
            })?;
        match answer {
            Answer::Unknown { kind, .. } => Err(Error::serialization(
                format!("Jev answer {name:?} has type {kind:?}, which this client does not know"),
                None,
            )),
            Answer::Malformed { kind, .. } => Err(Error::serialization(
                format!("the {kind} answer for {name:?} could not be read"),
                None,
            )),
            answer => Ok(answer),
        }
    }

    /// The probability of yes for noul question `name`.
    pub fn noul(&self, name: &str) -> Result<f64> {
        match self.answer(name)? {
            Answer::Noul(p) => finite(name, *p),
            other => Err(wrong_type(name, other, "noul")),
        }
    }

    /// The yes/no answer to noul question `name`, gated on certainty `|2p - 1|`.
    ///
    /// The answer is yes when `p >= 0.5`. Certainty measures how far `p` is from a coin flip,
    /// not whether the answer is correct.
    pub fn yes_no(&self, name: &str, min_certainty: f64) -> Result<Outcome<bool>> {
        let p = self.noul(name)?;
        Ok(gate(p >= 0.5, (2.0 * p - 1.0).abs(), min_certainty))
    }

    /// The answer to choice question `name`.
    pub fn choice(&self, name: &str) -> Result<&ChoiceAnswer> {
        match self.answer(name)? {
            Answer::Choice(choice) => {
                finite(name, choice.confidence)?;
                for (_, p) in &choice.probabilities {
                    finite(name, *p)?;
                }
                Ok(choice)
            }
            other => Err(wrong_type(name, other, "choice")),
        }
    }

    /// The typed pick for choice question `name`, gated on Jev's `confidence`.
    ///
    /// Confidence measures how concentrated the answer is, not whether it is correct.
    pub fn choose<T: Choices>(&self, name: &str, min_confidence: f64) -> Result<Outcome<T>> {
        let answer = self.choice(name)?;
        let value = T::OPTIONS
            .iter()
            .find(|(_, label, _)| *label == answer.choice)
            .map(|(value, _, _)| *value)
            .ok_or_else(|| {
                Error::serialization(
                    format!(
                        "Jev's choice for {name:?} is not a label of {}",
                        std::any::type_name::<T>()
                    ),
                    None,
                )
            })?;
        Ok(gate(value, answer.confidence, min_confidence))
    }

    /// The answer to score question `name`.
    pub fn score(&self, name: &str) -> Result<&ScoreAnswer> {
        match self.answer(name)? {
            Answer::Score(score) => {
                finite(name, score.score)?;
                finite(name, score.confidence)?;
                for p in &score.probabilities {
                    finite(name, *p)?;
                }
                Ok(score)
            }
            other => Err(wrong_type(name, other, "score")),
        }
    }

    /// The typed most likely level ([`ScoreAnswer::mode`]) for score question `name`, gated on
    /// Jev's `confidence`.
    ///
    /// Confidence measures how concentrated the answer is, not whether it is correct.
    pub fn rate<T: Levels>(&self, name: &str, min_confidence: f64) -> Result<Outcome<T>> {
        let answer = self.score(name)?;
        let value = answer
            .mode()
            .and_then(|index| T::LEVELS.get(index))
            .map(|(value, _)| *value)
            .ok_or_else(|| {
                Error::serialization(
                    format!(
                        "Jev's level for {name:?} is not a level of {}",
                        std::any::type_name::<T>()
                    ),
                    None,
                )
            })?;
        Ok(gate(value, answer.confidence, min_confidence))
    }
}

fn finite(name: &str, value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::serialization(
            format!("Jev answer {name:?} holds a non-finite number"),
            None,
        ))
    }
}

fn wrong_type(name: &str, answer: &Answer, expected: &str) -> Error {
    let actual = match answer {
        Answer::Noul(_) => "noul",
        Answer::Choice(_) => "choice",
        Answer::Score(_) => "score",
        Answer::Unknown { .. } => "unknown",
        Answer::Malformed { .. } => "malformed answer",
    };
    Error::validation(
        format!("Jev answer {name:?} is a {actual}, not a {expected}"),
        Some(name.to_string()),
    )
}
