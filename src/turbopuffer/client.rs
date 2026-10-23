//! The turbopuffer HTTP client: auth, base URL, retries and error mapping.

use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use reqwest::{Client as ReqwestClient, ClientBuilder, Method};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::namespace::Namespace;
use crate::backoff::{self, RetryPolicy};
use crate::error::{Error, ErrorKind, Result, Service, describe_json_error};
use crate::http;
use crate::types::retry_after_from_headers;

const API_KEY_ENV: &str = "TURBOPUFFER_API_KEY";
const REGION_ENV: &str = "TURBOPUFFER_REGION";
const BASE_URL_ENV: &str = "TURBOPUFFER_BASE_URL";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_MAX_RETRIES: usize = 3;
// Waiting out a longer Retry-After would stall the conversation, so fail right away instead.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
// Server text can echo the caller's data, so only this much of it reaches an error.
const MAX_ERROR_CHARS: usize = 200;
const MAX_NAMESPACE_BYTES: usize = 128;

/// Client for turbopuffer's v2 API.
///
/// The API key is only kept as a sensitive header value and is redacted from `Debug`.
/// Connection failures and responses 408, 409, 429 and 5xx are retried. A timeout, or a
/// response that stopped partway, is resent only for a query: an upsert or delete may already
/// have been applied. Each wait is the backoff delay or `Retry-After`, whichever is longer; a
/// `Retry-After` over 30 seconds fails at once.
#[derive(Clone)]
pub struct TurbopufferClient {
    http: ReqwestClient,
    authorization: HeaderValue,
    base_url: String,
    max_retries: usize,
    timeout: Duration,
}

impl fmt::Debug for TurbopufferClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TurbopufferClient")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl TurbopufferClient {
    /// Create a client for one region, such as `gcp-us-central1`, sending to
    /// `https://{region}.turbopuffer.com`.
    pub fn try_new(api_key: impl Into<String>, region: &str) -> Result<Self> {
        let region = region.trim();
        let valid = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
        if region.is_empty() || !region.chars().all(valid) {
            return Err(Error::validation(
                "turbopuffer region must be lowercase letters, digits and dashes, such as \
                 gcp-us-central1",
                Some("region".to_string()),
            ));
        }
        let base_url = format!("https://{region}.turbopuffer.com");
        let http = http::client(&base_url, builder())?;
        Self::build(&api_key.into(), base_url, http)
    }

    /// Create a client from `TURBOPUFFER_API_KEY` plus `TURBOPUFFER_REGION` or
    /// `TURBOPUFFER_BASE_URL`. When both are set the base URL wins. A blank variable counts as
    /// unset.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(|name| std::env::var(name).ok())
    }

    pub(super) fn from_env_with(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let var = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        let api_key = var(API_KEY_ENV).ok_or_else(|| {
            Error::validation(
                format!("{API_KEY_ENV} is not set or empty"),
                Some(API_KEY_ENV.to_string()),
            )
        })?;
        match (var(BASE_URL_ENV), var(REGION_ENV)) {
            (Some(base_url), _) => {
                let (base_url, http) =
                    http::rebase(Service::Turbopuffer, BASE_URL_ENV, &base_url, builder())?;
                Self::build(&api_key, base_url, http)
            }
            (None, Some(region)) => Self::try_new(api_key, &region),
            (None, None) => Err(Error::validation(
                format!("set {REGION_ENV} or {BASE_URL_ENV}"),
                Some(REGION_ENV.to_string()),
            )),
        }
    }

    fn build(api_key: &str, base_url: String, http: ReqwestClient) -> Result<Self> {
        Ok(Self {
            http,
            authorization: http::bearer(Service::Turbopuffer, api_key)?,
            base_url,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Send requests to another base URL, such as a BYOC cluster or a local test server.
    ///
    /// The URL must use https; plain http is accepted only for loopback hosts, so the key never
    /// travels in clear. Credentials, a query or a fragment in the URL are rejected. A trailing
    /// slash is trimmed.
    pub fn with_base_url(mut self, base_url: &str) -> Result<Self> {
        (self.base_url, self.http) =
            http::rebase(Service::Turbopuffer, "base_url", base_url, builder())?;
        Ok(self)
    }

    /// Set how many times a retryable failure is retried. Defaults to 3.
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the time limit for one attempt. Defaults to 60 seconds.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// A namespace and the embedding model its `text` attribute is embedded with.
    ///
    /// The name is 1 to 128 of `A-Z`, `a-z`, `0-9`, `-`, `_` and `.`, except `.` and `..`, which
    /// URL path resolution would turn into another endpoint. There is no default model:
    /// the first write fixes it for the namespace, and changing it later means re-ingesting
    /// every document. The model must be available in the client's region.
    pub fn namespace(
        &self,
        name: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Namespace> {
        let name: String = name.into();
        let valid = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        // "." and ".." would be resolved away as path segments and hit another endpoint.
        if name.is_empty()
            || name.len() > MAX_NAMESPACE_BYTES
            || !name.chars().all(valid)
            || name == "."
            || name == ".."
        {
            return Err(Error::validation(
                "turbopuffer namespace name must be 1 to 128 of A-Z, a-z, 0-9, -, _ and .",
                Some("namespace".to_string()),
            ));
        }
        let model: String = model.into();
        let model = model.trim().to_string();
        if model.is_empty() {
            return Err(Error::validation(
                "turbopuffer embedding model is empty",
                Some("model".to_string()),
            ));
        }
        Ok(Namespace::new(self.clone(), name, model))
    }

    /// Posts `body` as JSON to `path`, retrying transient failures, and parses the response.
    pub(super) async fn post<B, T>(&self, path: &str, body: &B, request: Request) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let body = serde_json::to_vec(body).map_err(|e| {
            Error::validation("Failed to serialize the turbopuffer request", None).with_source(e)
        })?;
        let bytes = self
            .send(
                Method::POST,
                path,
                Some(Bytes::from(body)),
                request == Request::Read,
            )
            .await?;
        // The serde error quotes the bad value, which is response content, so only its position
        // is kept.
        serde_json::from_slice(&bytes).map_err(|e| {
            Error::new(
                ErrorKind::Decode,
                format!(
                    "Failed to parse turbopuffer response ({} bytes): {}",
                    bytes.len(),
                    describe_json_error(&e)
                ),
            )
        })
    }

    /// Sends `DELETE` to `path`, retrying transient failures.
    pub(super) async fn delete(&self, path: &str) -> Result<()> {
        self.send(Method::DELETE, path, None, false).await.map(drop)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Bytes>,
        is_read: bool,
    ) -> Result<Bytes> {
        let url = format!("{}{path}", self.base_url);
        let policy = RetryPolicy {
            max_retries: self.max_retries,
            max_retry_after: MAX_RETRY_AFTER,
            budget: None,
            // A write that timed out or was cut off may already be applied, so only reads resend.
            resend_timeouts: is_read,
            resend_interrupted: is_read,
        };
        let retried = backoff::retry(
            &policy,
            |_, _| self.send_once(method.clone(), &url, body.as_ref()),
            |error, attempt, delay| {
                tracing::warn!(
                    target: "topcoat_si::turbopuffer",
                    error = error as &dyn std::error::Error,
                    path,
                    attempt = attempt + 1,
                    sleep_seconds = delay.as_secs_f64(),
                    "Retrying turbopuffer request"
                );
            },
        )
        .await;
        retried.result
    }

    async fn send_once(&self, method: Method, url: &str, body: Option<&Bytes>) -> Result<Bytes> {
        let mut request = self
            .http
            .request(method, url)
            .timeout(self.timeout)
            .header(AUTHORIZATION, self.authorization.clone());
        if let Some(body) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
        }
        let reply = http::send(request, Service::Turbopuffer, |_| None).await?;
        if !reply.is_success() {
            return Err(http_error(
                reply.status,
                &reply.body,
                retry_after_from_headers(&reply.headers),
            ));
        }
        Ok(reply.body)
    }
}

/// What a request does, which decides whether a timeout or a response cut off partway is sent again.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Request {
    /// A query: resending it only repeats the read.
    Read,
    /// An upsert or delete: the server may have applied it, so resending would report a delete
    /// as `rows_deleted: 0` or bill the embeddings again.
    Write,
}

/// The turbopuffer settings for [`http::client`].
fn builder() -> ClientBuilder {
    ReqwestClient::builder().connect_timeout(Duration::from_secs(30))
}

// Display already shows the service and status, so the message is only the JSON `error` field,
// cut to MAX_ERROR_CHARS.
fn http_error(status: u16, body: &[u8], retry_after: Option<Duration>) -> Error {
    let detail = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(String::from))
        .map(|detail| http::truncate_chars(&detail, MAX_ERROR_CHARS, ""));
    Error::http_status(
        Service::Turbopuffer,
        status,
        detail,
        None,
        None,
        None,
        retry_after,
    )
}
