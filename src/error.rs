//! One error type for every client: what failed, which service failed, and whether sending the
//! same request again might work.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{error, fmt};

use serde_json::error::Category;

use crate::jev::JevErrorCode;
use crate::types::ErrorType;

/// The error code an error body carried, by service.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// An Anthropic API error's `error.details.error_code` as sent, such as
    /// `enforced_spend_limit_reached`. Anthropic documents codes only for some errors, so the
    /// set is open.
    Anthropic(String),
    /// A Jev error: the body's `detail.error_type`, or a code this client sets itself.
    Jev(JevErrorCode),
    /// A Firecrawl error: the body's `code` as sent. Firecrawl documents codes only for some
    /// statuses, so the set is open.
    Firecrawl(String),
}

impl ErrorCode {
    /// The code as the service sends it; `None` for a code with no wire form.
    fn wire_name(&self) -> Option<&str> {
        match self {
            Self::Anthropic(code) | Self::Firecrawl(code) => Some(code),
            Self::Jev(code) => code.wire_name(),
        }
    }
}

/// The service that refused a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Service {
    /// The Anthropic API.
    Anthropic,
    /// The Firecrawl API.
    Firecrawl,
    /// TypeSafe's Jev API.
    Jev,
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Anthropic => "Anthropic",
            Self::Firecrawl => "Firecrawl",
            Self::Jev => "Jev",
        })
    }
}

/// What failed. [`Error::is_retryable`] says whether sending the same request again might work.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Rejected before sending: bad parameters or configuration, or a request that fails to
    /// serialize.
    #[non_exhaustive]
    Validation {
        /// The parameter, setting or environment variable at fault, when there is one.
        param: Option<String>,
    },
    /// A local size cap was exceeded: Jev's request body or the message batch size.
    #[non_exhaustive]
    RequestTooLarge {
        /// The size of the request in bytes.
        size: usize,
        /// The cap in bytes.
        limit: usize,
    },
    /// Structured outputs cannot use this type, for example a recursive schema.
    #[non_exhaustive]
    UnsupportedSchema {
        /// The Rust type name.
        type_name: String,
    },
    /// The connection failed before any response arrived: DNS failure, refused, TLS failure, a
    /// connect timeout, or a reset before the reply. Usually nothing reached the server; when the
    /// request was already written it may have, which Anthropic's SDKs also accept when they
    /// resend.
    Connection,
    /// The request was sent, but no complete response arrived in time.
    Timeout,
    /// The service refused: an HTTP error status, an `error` event inside a stream, or a
    /// Firecrawl 2xx body with `success: false`.
    #[non_exhaustive]
    Api {
        /// The service that refused.
        service: Service,
        /// The HTTP status; `None` for an `error` event inside a stream, an errored message
        /// batch result, or a Firecrawl 2xx body with `success: false`.
        status: Option<u16>,
        /// Anthropic's `error.type`, such as `overloaded_error`.
        error_type: Option<ErrorType>,
        /// The service's error code: Anthropic's `error.details.error_code`, Jev's
        /// `detail.error_type` or Firecrawl's `code`.
        error_code: Option<ErrorCode>,
        /// The delay the service asked for in `retry-after`.
        retry_after: Option<Duration>,
        /// The stream payload's own `retryable` flag, when it sent one.
        retryable: Option<bool>,
    },
    /// The response started, then stopped: the connection dropped mid-body, or a stream ended
    /// before `message_stop`. Only GET requests are resent automatically; the server has already
    /// answered a POST or DELETE.
    Interrupted,
    /// A success response arrived but is not what was expected: bad JSON, bad SSE framing or
    /// UTF-8, or a bad batch result line.
    Decode,
    /// Structured output was cut off at `max_tokens`; retry with more tokens.
    MaxTokens,
    /// The model refused, so the reply does not follow the schema.
    Refusal,
    /// Reading a local file failed.
    #[non_exhaustive]
    Io {
        /// The file, when known.
        path: Option<PathBuf>,
    },
    /// Code mode: a sandbox pool, worker or pack failure.
    Sandbox,
    /// The `surreal` feature: an embedded SurrealDB query, schema or storage failure.
    Store,
    /// A [`ClientTool`](crate::ClientTool) author's own fatal failure.
    Tool,
}

/// A failure from any client in this crate.
///
/// The message says what failed and where; the cause is only in
/// [`source`](error::Error::source).
#[derive(Clone, Debug)]
#[must_use]
pub struct Error {
    // Boxed so `Result<T, Error>` stays small.
    kind: Box<ErrorKind>,
    message: String,
    request_id: Option<String>,
    events_delivered: Option<u64>,
    source: Option<Arc<dyn error::Error + Send + Sync>>,
}

impl Error {
    /// Builds a [`ErrorKind::Validation`] error for a request rejected before sending.
    pub fn validation(message: impl Into<String>, param: Option<String>) -> Self {
        Self::new(ErrorKind::Validation { param }, message)
    }

    /// Builds a [`ErrorKind::Store`] error: a SurrealDB failure, with the database's own error as
    /// the source.
    #[cfg(feature = "surreal")]
    pub fn store(
        message: impl Into<String>,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            source: source.map(Arc::from),
            ..Self::new(ErrorKind::Store, message)
        }
    }

    /// Builds a [`ErrorKind::Tool`] error: a [`ClientTool`](crate::ClientTool) failure the model
    /// cannot fix, with the tool's own error as the source.
    pub fn tool(
        message: impl Into<String>,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            source: source.map(Arc::from),
            ..Self::new(ErrorKind::Tool, message)
        }
    }

    /// Builds a [`ErrorKind::Api`] error, for example to simulate a 429 in a test. A `status` of
    /// `None` is an `error` event inside a stream or an errored message batch result.
    pub fn api(
        service: Service,
        status: Option<u16>,
        error_type: Option<ErrorType>,
        message: impl Into<String>,
        retry_after: Option<Duration>,
    ) -> Self {
        let kind = ErrorKind::Api {
            service,
            status,
            error_type,
            error_code: None,
            retry_after,
            retryable: None,
        };
        Self::new(kind, message)
    }

    /// Builds an error of `kind` with no source.
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind: Box::new(kind),
            message: message.into(),
            request_id: None,
            events_delivered: None,
            source: None,
        }
    }

    /// Builds the [`ErrorKind::Api`] error for an HTTP error status. Every client parses its own
    /// body shape, then calls this. A body without a message, or with an empty or blank one,
    /// gets `HTTP {status} with no error message`.
    pub(crate) fn http_status(
        service: Service,
        status: u16,
        message: Option<String>,
        error_type: Option<ErrorType>,
        error_code: Option<ErrorCode>,
        request_id: Option<String>,
        retry_after: Option<Duration>,
    ) -> Self {
        let kind = ErrorKind::Api {
            service,
            status: Some(status),
            error_type,
            error_code,
            retry_after,
            retryable: None,
        };
        let message = message
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("HTTP {status} with no error message"));
        Self::new(kind, message).with_request_id(request_id)
    }

    /// A local size cap was exceeded: the request is `size` bytes in total, against a cap of
    /// `limit`.
    pub(crate) fn request_too_large(message: impl Into<String>, size: usize, limit: usize) -> Self {
        Self::new(ErrorKind::RequestTooLarge { size, limit }, message)
    }

    /// Keeps `source` as the cause. It never reaches the message.
    pub(crate) fn with_source(mut self, source: impl error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    /// Attaches the response's request id, when there is one. The kind never changes.
    pub(crate) fn with_request_id(mut self, request_id: Option<String>) -> Self {
        if request_id.is_some() {
            self.request_id = request_id;
        }
        self
    }

    /// Records how many events a stream yielded before this error. The kind never changes.
    pub(crate) fn with_events_delivered(mut self, events_delivered: u64) -> Self {
        self.events_delivered = Some(events_delivered);
        self
    }

    /// Rewrites the parameter of a [`ErrorKind::Validation`] error with `rename`, keeping the
    /// message and source. Other kinds, and a validation error without a parameter, are
    /// returned as they are.
    pub(crate) fn rename_param(mut self, rename: impl FnOnce(&str) -> String) -> Self {
        if let ErrorKind::Validation { param: Some(param) } = self.kind.as_mut() {
            *param = rename(param);
        }
        self
    }

    /// This layer's words, without the request id.
    #[cfg(test)]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// What failed.
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// The response's request id, when one came back.
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// The HTTP status of an [`ErrorKind::Api`] error; `None` for every other kind, for an
    /// `error` event inside a stream and for an errored message batch result.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self.kind.as_ref() {
            ErrorKind::Api { status, .. } => *status,
            _ => None,
        }
    }

    /// The delay an [`ErrorKind::Api`] error asked for in `retry-after`.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self.kind.as_ref() {
            ErrorKind::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// For an error yielded by a stream, the number of events the stream yielded before it.
    /// `Some(0)` means resending the request is clean.
    #[must_use]
    pub fn events_delivered(&self) -> Option<u64> {
        self.events_delivered
    }

    /// Whether sending the same request again might succeed.
    ///
    /// True for [`ErrorKind::Connection`], [`ErrorKind::Timeout`] and
    /// [`ErrorKind::Interrupted`]; for HTTP 408, 409, 429 and 5xx, except a 429 for an
    /// exhausted spend limit; for stream `error` events that say they are retryable or, when
    /// they don't say, have a transient type; and for errored batch results of a transient
    /// type. False for everything else.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self.kind.as_ref() {
            ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Interrupted => true,
            ErrorKind::Api {
                status: Some(429),
                error_code: Some(ErrorCode::Anthropic(code)),
                ..
            } if code == "enforced_spend_limit_reached" => false,
            ErrorKind::Api {
                status: Some(status),
                ..
            } => matches!(status, 408 | 409 | 429 | 500..=599),
            ErrorKind::Api {
                status: None,
                retryable: Some(retryable),
                ..
            } => *retryable,
            ErrorKind::Api {
                status: None,
                error_type,
                ..
            } => matches!(
                error_type,
                Some(
                    ErrorType::OverloadedError
                        | ErrorType::ApiError
                        | ErrorType::RateLimitError
                        | ErrorType::TimeoutError
                )
            ),
            ErrorKind::Validation { .. }
            | ErrorKind::RequestTooLarge { .. }
            | ErrorKind::UnsupportedSchema { .. }
            | ErrorKind::Decode
            | ErrorKind::MaxTokens
            | ErrorKind::Refusal
            | ErrorKind::Io { .. }
            | ErrorKind::Sandbox
            | ErrorKind::Store
            | ErrorKind::Tool => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ErrorKind::Api {
            service,
            status,
            error_type,
            error_code,
            ..
        } = self.kind.as_ref()
        else {
            f.write_str(&self.message)?;
            if let Some(id) = &self.request_id {
                write!(f, " (request {id})")?;
            }
            return Ok(());
        };
        write!(f, "{service} API error: {} (", self.message)?;
        // Anthropic's error type, else the service's own code, as the service sends it. An
        // unknown type has no wire name, so the code shows instead.
        let error_type = error_type
            .and_then(|error_type| error_type.wire_name())
            .or_else(|| error_code.as_ref().and_then(ErrorCode::wire_name));
        match (status, error_type) {
            (Some(status), Some(error_type)) => write!(f, "HTTP {status} {error_type}")?,
            (Some(status), None) => write!(f, "HTTP {status}")?,
            (None, error_type) => {
                // Firecrawl's only refusal without an error status is a 2xx `success: false`.
                f.write_str(match service {
                    Service::Firecrawl => "success false",
                    _ => "in stream",
                })?;
                if let Some(error_type) = error_type {
                    write!(f, ", {error_type}")?;
                }
            }
        }
        if let Some(id) = &self.request_id {
            write!(f, ", request {id}")?;
        }
        f.write_str(")")
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn error::Error + 'static))
    }
}

/// A specialized Result type for Topcoat SI operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Says where serde_json gave up, as the error category, line and column. It never quotes the
/// text that failed to parse, which is response content and may be sensitive.
pub(crate) fn describe_json_error(error: &serde_json::Error) -> String {
    let category = match error.classify() {
        Category::Io => "I/O error",
        Category::Syntax => "syntax error",
        Category::Data => "type or value mismatch",
        Category::Eof => "unexpected end of input",
    };
    format!(
        "{category} at line {} column {}",
        error.line(),
        error.column()
    )
}

/// Whether response headers had arrived when a transport failure happened.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RequestPhase {
    Headers,
    Body,
}

/// Maps a reqwest failure by what happened. The message names the host; the reqwest error is the
/// source.
pub(crate) fn map_reqwest_error(err: reqwest::Error, phase: RequestPhase) -> Error {
    let host = err
        .url()
        .and_then(|url| url.host_str())
        .unwrap_or("the server")
        .to_string();
    // reqwest reports a connect timeout as both a connect and a timeout failure, so the connect
    // check comes first.
    let (kind, message) = if err.is_builder() {
        (
            ErrorKind::Validation { param: None },
            format!("could not build the request to {host}"),
        )
    } else if err.is_connect() {
        (
            ErrorKind::Connection,
            format!("could not connect to {host}"),
        )
    } else if err.is_timeout() {
        (ErrorKind::Timeout, format!("request to {host} timed out"))
    } else {
        // reqwest reports every failed body read as a decode error, so the phase decides. A
        // failure before the headers is `Connection` even when the request was already written,
        // as in Anthropic's SDKs, which resend it.
        match phase {
            RequestPhase::Headers => (
                ErrorKind::Connection,
                format!("connection to {host} failed before a response arrived"),
            ),
            RequestPhase::Body => (
                ErrorKind::Interrupted,
                format!("connection to {host} failed while reading the response"),
            ),
        }
    };
    Error::new(kind, message).with_source(err)
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    fn http(status: u16) -> Error {
        Error::http_status(Service::Anthropic, status, None, None, None, None, None)
    }

    fn stream_event(error_type: Option<ErrorType>, retryable: Option<bool>) -> Error {
        let kind = ErrorKind::Api {
            service: Service::Anthropic,
            status: None,
            error_type,
            error_code: None,
            retry_after: None,
            retryable,
        };
        Error::new(kind, "stream failed")
    }

    #[test]
    fn retryable_kinds() {
        let cases = [
            (ErrorKind::Validation { param: None }, false),
            (ErrorKind::RequestTooLarge { size: 2, limit: 1 }, false),
            (
                ErrorKind::UnsupportedSchema {
                    type_name: "Node".to_string(),
                },
                false,
            ),
            (ErrorKind::Connection, true),
            (ErrorKind::Timeout, true),
            (ErrorKind::Interrupted, true),
            (ErrorKind::Decode, false),
            (ErrorKind::MaxTokens, false),
            (ErrorKind::Refusal, false),
            (ErrorKind::Io { path: None }, false),
            (ErrorKind::Sandbox, false),
            (ErrorKind::Store, false),
            (ErrorKind::Tool, false),
        ];
        for (kind, retryable) in cases {
            let error = Error::new(kind.clone(), "failed");
            assert_eq!(error.is_retryable(), retryable, "{kind:?}");
        }
    }

    #[test]
    fn empty_or_blank_message_gets_the_fallback() {
        for message in ["", "  \n\t"] {
            let error = Error::http_status(
                Service::Jev,
                503,
                Some(message.to_string()),
                None,
                None,
                None,
                None,
            );
            assert_eq!(error.message(), "HTTP 503 with no error message");
        }
        let error = Error::http_status(
            Service::Jev,
            503,
            Some("down".to_string()),
            None,
            None,
            None,
            None,
        );
        assert_eq!(error.message(), "down");
    }

    #[test]
    fn unknown_type_shows_the_error_code() {
        let error = Error::http_status(
            Service::Anthropic,
            429,
            Some("Spend limit reached".to_string()),
            Some(ErrorType::Unknown),
            Some(ErrorCode::Anthropic(
                "enforced_spend_limit_reached".to_string(),
            )),
            Some("req_9".to_string()),
            None,
        );
        assert_eq!(
            error.to_string(),
            "Anthropic API error: Spend limit reached (HTTP 429 enforced_spend_limit_reached, request req_9)"
        );
    }

    #[test]
    fn retryable_statuses() {
        for status in [408, 409, 429, 500, 501, 502, 503, 504, 529, 599] {
            assert!(http(status).is_retryable(), "{status}");
        }
        for status in [200, 400, 401, 402, 403, 404, 413, 422, 600] {
            assert!(!http(status).is_retryable(), "{status}");
        }
    }

    #[test]
    fn spend_limit_429_is_not_retryable() {
        let spend_cap = Error::http_status(
            Service::Anthropic,
            429,
            Some("Spend limit reached".to_string()),
            Some(ErrorType::RateLimitError),
            Some(ErrorCode::Anthropic(
                "enforced_spend_limit_reached".to_string(),
            )),
            None,
            None,
        );
        assert!(!spend_cap.is_retryable());
        let other_code = Error::http_status(
            Service::Anthropic,
            429,
            None,
            None,
            Some(ErrorCode::Anthropic("something_else".to_string())),
            None,
            None,
        );
        assert!(other_code.is_retryable());
    }

    #[test]
    fn stream_events_follow_their_flag_then_their_type() {
        for error_type in [
            ErrorType::OverloadedError,
            ErrorType::ApiError,
            ErrorType::RateLimitError,
            ErrorType::TimeoutError,
        ] {
            assert!(
                stream_event(Some(error_type), None).is_retryable(),
                "{error_type:?}"
            );
            assert!(
                !stream_event(Some(error_type), Some(false)).is_retryable(),
                "{error_type:?}"
            );
        }
        for error_type in [
            ErrorType::InvalidRequestError,
            ErrorType::RequestTooLarge,
            ErrorType::Unknown,
        ] {
            assert!(
                !stream_event(Some(error_type), None).is_retryable(),
                "{error_type:?}"
            );
        }
        assert!(!stream_event(None, None).is_retryable());
        assert!(stream_event(Some(ErrorType::InvalidRequestError), Some(true)).is_retryable());
        assert!(stream_event(None, Some(true)).is_retryable());
    }

    #[test]
    fn api_display_names_service_status_type_and_request() {
        let error = Error::http_status(
            Service::Anthropic,
            529,
            Some("Overloaded".to_string()),
            Some(ErrorType::OverloadedError),
            None,
            Some("req_123".to_string()),
            None,
        );
        assert_eq!(
            error.to_string(),
            "Anthropic API error: Overloaded (HTTP 529 overloaded_error, request req_123)"
        );
        let error = Error::api(Service::Firecrawl, Some(404), None, "Not found", None);
        assert_eq!(
            error.to_string(),
            "Firecrawl API error: Not found (HTTP 404)"
        );
        let error = Error::http_status(Service::Jev, 503, None, None, None, None, None);
        assert_eq!(
            error.to_string(),
            "Jev API error: HTTP 503 with no error message (HTTP 503)"
        );
        let error = Error::http_status(
            Service::Firecrawl,
            429,
            Some("Slow down".to_string()),
            None,
            Some(ErrorCode::Firecrawl("RATE_LIMIT".to_string())),
            None,
            None,
        );
        assert_eq!(
            error.to_string(),
            "Firecrawl API error: Slow down (HTTP 429 RATE_LIMIT)"
        );
        let error = Error::http_status(
            Service::Anthropic,
            500,
            Some("Odd".to_string()),
            Some(ErrorType::Unknown),
            None,
            None,
            None,
        );
        assert_eq!(error.to_string(), "Anthropic API error: Odd (HTTP 500)");
    }

    #[test]
    fn stream_event_display_says_in_stream() {
        let error = stream_event(Some(ErrorType::OverloadedError), None)
            .with_request_id(Some("req_1".to_string()));
        assert_eq!(
            error.to_string(),
            "Anthropic API error: stream failed (in stream, overloaded_error, request req_1)"
        );
        assert_eq!(
            stream_event(None, None).to_string(),
            "Anthropic API error: stream failed (in stream)"
        );
    }

    #[test]
    fn other_kinds_display_their_message_and_request() {
        let error = Error::new(ErrorKind::Decode, "could not parse the response");
        assert_eq!(error.to_string(), "could not parse the response");
        let error = error.with_request_id(Some("req_9".to_string()));
        assert_eq!(
            error.to_string(),
            "could not parse the response (request req_9)"
        );
    }

    #[test]
    fn display_never_names_a_variant() {
        let kinds = [
            ErrorKind::Validation { param: None },
            ErrorKind::Connection,
            ErrorKind::Timeout,
            ErrorKind::Interrupted,
            ErrorKind::Decode,
            ErrorKind::Sandbox,
            ErrorKind::Store,
            ErrorKind::Tool,
        ];
        let mut errors: Vec<Error> = Vec::new();
        for kind in kinds {
            errors.push(Error::new(kind, "failed").with_request_id(Some("req".to_string())));
        }
        errors.push(http(503));
        errors.push(stream_event(Some(ErrorType::ApiError), None));
        for error in errors {
            let text = error.to_string();
            for name in [
                "Api:",
                "Timeout:",
                "Connection:",
                "Validation:",
                "Decode",
                "Interrupted",
                "Sandbox",
                "Store",
                "ErrorKind",
            ] {
                assert!(!text.contains(name), "{text}");
            }
        }
    }

    #[test]
    fn attaching_request_id_or_events_never_changes_the_kind() {
        let kinds = [
            ErrorKind::Validation { param: None },
            ErrorKind::Connection,
            ErrorKind::Timeout,
            ErrorKind::Interrupted,
            ErrorKind::Decode,
            ErrorKind::Tool,
            http(529).kind().clone(),
        ];
        for kind in kinds {
            let error = Error::new(kind.clone(), "failed")
                .with_request_id(Some("req_1".to_string()))
                .with_events_delivered(3);
            assert_eq!(error.kind(), &kind);
            assert_eq!(error.request_id(), Some("req_1"));
            assert_eq!(error.events_delivered(), Some(3));
        }
        let kept = Error::new(ErrorKind::Decode, "failed")
            .with_request_id(Some("req_1".to_string()))
            .with_request_id(None);
        assert_eq!(kept.request_id(), Some("req_1"));
    }

    #[test]
    fn accessors_read_the_api_kind_only() {
        let error = Error::api(
            Service::Anthropic,
            Some(429),
            None,
            "slow down",
            Some(Duration::from_secs(5)),
        );
        assert_eq!(error.status(), Some(429));
        assert_eq!(error.retry_after(), Some(Duration::from_secs(5)));
        let error = Error::new(ErrorKind::Timeout, "timed out");
        assert_eq!(error.status(), None);
        assert_eq!(error.retry_after(), None);
    }

    #[test]
    fn source_is_kept_and_not_copied_into_the_message() {
        let cause = std::io::Error::other("disk on fire");
        let error =
            Error::new(ErrorKind::Io { path: None }, "could not read the key").with_source(cause);
        assert!(!error.to_string().contains("disk on fire"));
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("disk on fire")
        );
    }

    #[test]
    fn renaming_a_validation_param_keeps_message_and_source() {
        let cause = std::io::Error::other("bad byte");
        let error = Error::validation("bad role", Some("messages[0].role".to_string()))
            .with_source(cause)
            .rename_param(|param| format!("requests[2].params.{param}"));
        assert_eq!(
            error.kind(),
            &ErrorKind::Validation {
                param: Some("requests[2].params.messages[0].role".to_string())
            }
        );
        assert_eq!(error.to_string(), "bad role");
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("bad byte")
        );
        let unnamed = Error::validation("bad", None).rename_param(|_| "renamed".to_string());
        assert_eq!(unnamed.kind(), &ErrorKind::Validation { param: None });
    }

    async fn reqwest_error(client: reqwest::Client, url: &str, phase: RequestPhase) -> Error {
        let error = match phase {
            RequestPhase::Headers => client
                .get(url)
                .send()
                .await
                .expect_err("the request should fail"),
            RequestPhase::Body => client
                .get(url)
                .send()
                .await
                .expect("headers should arrive")
                .bytes()
                .await
                .expect_err("the body should fail"),
        };
        map_reqwest_error(error, phase)
    }

    fn local_client(timeout: Option<Duration>) -> reqwest::Client {
        let builder = reqwest::Client::builder().no_proxy();
        let builder = match timeout {
            Some(timeout) => builder.timeout(timeout),
            None => builder,
        };
        builder.build().expect("client")
    }

    #[tokio::test]
    async fn refused_connection_is_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        drop(listener);
        let url = format!("http://{address}/");
        let error = reqwest_error(local_client(None), &url, RequestPhase::Headers).await;
        assert_eq!(error.kind(), &ErrorKind::Connection, "{error}");
        assert!(error.is_retryable());
        assert_eq!(error.to_string(), "could not connect to 127.0.0.1");
        assert!(error.source().is_some());
    }

    #[tokio::test]
    async fn connect_timeout_is_connection_and_retryable() {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(100))
            .no_proxy()
            .build()
            .expect("client");
        // TEST-NET-1 is never routed, so the connection never completes.
        let error = reqwest_error(client, "http://192.0.2.1/", RequestPhase::Headers).await;
        assert_eq!(error.kind(), &ErrorKind::Connection, "{error}");
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn response_timeout_after_connect_is_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0; 1024];
            let _ = socket.read(&mut buffer).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let client = local_client(Some(Duration::from_millis(200)));
        let url = format!("http://{address}/");
        let error = reqwest_error(client, &url, RequestPhase::Headers).await;
        assert_eq!(error.kind(), &ErrorKind::Timeout, "{error}");
        assert!(error.is_retryable());
        server.abort();
    }

    #[tokio::test]
    async fn reset_before_any_reply_is_connection_and_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0; 1024];
            let _ = socket.read(&mut buffer).await;
            // Dropping the socket closes it without a reply.
        });
        let url = format!("http://{address}/");
        let error = reqwest_error(local_client(None), &url, RequestPhase::Headers).await;
        assert_eq!(error.kind(), &ErrorKind::Connection, "{error}");
        assert!(error.is_retryable());
        assert_eq!(
            error.to_string(),
            "connection to 127.0.0.1 failed before a response arrived"
        );
    }

    #[tokio::test]
    async fn body_dropped_mid_way_is_interrupted() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0; 1024];
            let _ = socket.read(&mut buffer).await;
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial";
            socket.write_all(response.as_bytes()).await.expect("write");
        });
        let url = format!("http://{address}/");
        let error = reqwest_error(local_client(None), &url, RequestPhase::Body).await;
        assert_eq!(error.kind(), &ErrorKind::Interrupted, "{error}");
        assert_eq!(
            error.to_string(),
            "connection to 127.0.0.1 failed while reading the response"
        );
    }

    #[tokio::test]
    async fn builder_error_is_validation() {
        let error = reqwest_error(local_client(None), "not a url", RequestPhase::Headers).await;
        assert_eq!(
            error.kind(),
            &ErrorKind::Validation { param: None },
            "{error}"
        );
        assert!(!error.is_retryable());
    }
}
