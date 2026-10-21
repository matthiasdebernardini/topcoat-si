//! Errors preserve transport sources and HTTP diagnostics without response bodies.
use std::str::Utf8Error;
use std::sync::Arc;
use std::time::Duration;
use std::{error, fmt, io};

use crate::jev::JevErrorCode;
use crate::types::ErrorType;

/// Diagnostics shared by every HTTP response error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HttpMetadata {
    /// HTTP status code.
    pub status: u16,
    /// The service's error code, when the body had one.
    pub error_code: Option<ErrorCode>,
    /// The `request-id` header or body field.
    pub request_id: Option<String>,
    /// The delay from the `retry-after` header.
    pub retry_after: Option<Duration>,
}

/// The error code an HTTP error body carried, by service.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// An Anthropic API error: the body's `error.type`.
    Anthropic(ErrorType),
    /// A Jev error: the body's `detail.error_type`, or a code this client sets itself.
    Jev(JevErrorCode),
    /// A Firecrawl error: the body's `code` as sent. Firecrawl documents codes only for some
    /// statuses, so the set is open.
    Firecrawl(String),
}

/// SDK failure, with original sources retained when available.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Error {
    /// Any other HTTP status.
    Api {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 401.
    Authentication {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 403.
    Permission {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 404.
    NotFound {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 429.
    RateLimit {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 400.
    BadRequest {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 500.
    InternalServer {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 502, 503 or 504.
    ServiceUnavailable {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// HTTP 529.
    Overloaded {
        /// The API's error text.
        message: String,
        /// Status, error type, request id and `retry-after` of the response.
        metadata: HttpMetadata,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// The request timed out.
    Timeout {
        /// Response diagnostics, when headers were received.
        metadata: Option<Box<HttpMetadata>>,
        /// Human-readable error message.
        message: String,
        /// The underlying failure, when there was one.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// Request was aborted by the client.
    Abort {
        /// Human-readable error message.
        message: String,
    },

    /// Connection error.
    Connection {
        /// Human-readable error message.
        message: String,
        /// Underlying cause.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },

    /// Error during JSON serialization or deserialization.
    Serialization {
        /// Human-readable error message.
        message: String,
        /// The underlying error.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },

    /// I/O error.
    Io {
        /// Human-readable error message.
        message: String,
        /// The underlying error.
        source: Arc<io::Error>,
    },

    /// HTTP client error.
    HttpClient {
        /// Human-readable error message.
        message: String,
        /// The underlying error.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },

    /// Error during validation of request parameters.
    Validation {
        /// Human-readable error message.
        message: String,
        /// Parameter that failed validation.
        param: Option<String>,
    },

    /// A URL parsing or manipulation error.
    Url {
        /// Human-readable error message.
        message: String,
        /// The underlying error.
        source: Option<url::ParseError>,
    },

    /// A streaming error occurred.
    Streaming {
        /// Human-readable error message.
        message: String,
        /// The underlying error.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },

    /// Encoding/decoding error.
    Encoding {
        /// Human-readable error message.
        message: String,
        /// The underlying error.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },

    /// The Python sandbox failed in a way the model cannot fix: a pool that would not start, a
    /// worker that would not spawn, a pack whose source fails to run, or a session that would
    /// not finish. Never retryable.
    Sandbox {
        /// Human-readable error message.
        message: String,
    },

    /// Unknown error.
    Unknown {
        /// Human-readable error message.
        message: String,
    },

    /// Unimplemented functionality.
    ToDo {
        /// Human-readable error message.
        message: String,
    },
}

impl Error {
    /// Builds the variant that matches `metadata.status`:
    ///
    /// | Status | Variant |
    /// |---|---|
    /// | 400 | [`Error::BadRequest`] |
    /// | 401 | [`Error::Authentication`] |
    /// | 403 | [`Error::Permission`] |
    /// | 404 | [`Error::NotFound`] |
    /// | 429 | [`Error::RateLimit`] |
    /// | 500 | [`Error::InternalServer`] |
    /// | 502, 503, 504 | [`Error::ServiceUnavailable`] |
    /// | 529 | [`Error::Overloaded`] |
    /// | any other | [`Error::Api`] |
    #[must_use]
    pub fn from_http(
        message: impl Into<String>,
        metadata: HttpMetadata,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        let message: String = message.into();
        let source: Option<Arc<dyn error::Error + Send + Sync>> = source.map(Arc::from);
        match metadata.status {
            400 => Self::BadRequest {
                message,
                metadata,
                source,
            },
            401 => Self::Authentication {
                message,
                metadata,
                source,
            },
            403 => Self::Permission {
                message,
                metadata,
                source,
            },
            404 => Self::NotFound {
                message,
                metadata,
                source,
            },
            429 => Self::RateLimit {
                message,
                metadata,
                source,
            },
            500 => Self::InternalServer {
                message,
                metadata,
                source,
            },
            502..=504 => Self::ServiceUnavailable {
                message,
                metadata,
                source,
            },
            529 => Self::Overloaded {
                message,
                metadata,
                source,
            },
            _ => Self::Api {
                message,
                metadata,
                source,
            },
        }
    }
    /// Builds the [`Error::from_http`] variant for `status`, with no source or `retry-after`.
    #[must_use]
    pub fn api(
        status: u16,
        error_code: Option<ErrorCode>,
        message: impl Into<String>,
        request_id: Option<String>,
    ) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status,
                error_code,
                request_id,
                retry_after: None,
            },
            None,
        )
    }
    /// Creates a new timeout error.
    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout {
            message: message.into(),
            source: None,
            metadata: None,
        }
    }
    /// Creates a new HTTP 401 authentication error.
    #[must_use]
    pub fn authentication(message: impl Into<String>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 401,
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new HTTP 403 permission error.
    #[must_use]
    pub fn permission(message: impl Into<String>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 403,
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new HTTP 404 not-found error.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 404,
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new HTTP 429 rate-limit error.
    #[must_use]
    pub fn rate_limit(message: impl Into<String>, retry_after: Option<Duration>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 429,
                retry_after,
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new HTTP 400 bad-request error.
    #[must_use]
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 400,
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new HTTP 500 internal-server error.
    #[must_use]
    pub fn internal_server(message: impl Into<String>, request_id: Option<String>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 500,
                request_id,
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new HTTP 503 service-unavailable error.
    #[must_use]
    pub fn service_unavailable(message: impl Into<String>, retry_after: Option<Duration>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 503,
                retry_after,
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new abort error.
    #[must_use]
    pub fn abort(message: impl Into<String>) -> Self {
        Error::Abort {
            message: message.into(),
        }
    }

    /// Creates a new connection error.
    #[must_use]
    pub fn connection(
        message: impl Into<String>,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Error::Connection {
            message: message.into(),
            source: source.map(Arc::from),
        }
    }

    /// Creates a new serialization error.
    #[must_use]
    pub fn serialization(
        message: impl Into<String>,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Error::Serialization {
            message: message.into(),
            source: source.map(Arc::from),
        }
    }

    /// Creates a new I/O error.
    #[must_use]
    pub fn io(message: impl Into<String>, source: io::Error) -> Self {
        Error::Io {
            message: message.into(),
            source: Arc::new(source),
        }
    }

    /// Creates a new HTTP client error.
    #[must_use]
    pub fn http_client(
        message: impl Into<String>,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Error::HttpClient {
            message: message.into(),
            source: source.map(Arc::from),
        }
    }

    /// Creates a new validation error.
    #[must_use]
    pub fn validation(message: impl Into<String>, param: Option<String>) -> Self {
        Error::Validation {
            message: message.into(),
            param,
        }
    }

    /// Creates a new URL error.
    #[must_use]
    pub fn url(message: impl Into<String>, source: Option<url::ParseError>) -> Self {
        Error::Url {
            message: message.into(),
            source,
        }
    }

    /// Creates a new streaming error.
    #[must_use]
    pub fn streaming(
        message: impl Into<String>,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Error::Streaming {
            message: message.into(),
            source: source.map(Arc::from),
        }
    }

    /// Creates a new encoding error.
    #[must_use]
    pub fn encoding(
        message: impl Into<String>,
        source: Option<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Error::Encoding {
            message: message.into(),
            source: source.map(Arc::from),
        }
    }

    /// Creates a new sandbox error.
    #[must_use]
    pub fn sandbox(message: impl Into<String>) -> Self {
        Error::Sandbox {
            message: message.into(),
        }
    }

    /// Creates a new unknown error.
    #[must_use]
    pub fn unknown(message: impl Into<String>) -> Self {
        Error::Unknown {
            message: message.into(),
        }
    }

    /// Creates a new ToDo error for unimplemented functionality.
    #[must_use]
    pub fn todo(message: impl Into<String>) -> Self {
        Error::ToDo {
            message: message.into(),
        }
    }

    /// Returns true if this error is related to authentication.
    #[must_use]
    pub fn is_authentication(&self) -> bool {
        matches!(self, Error::Authentication { .. })
    }

    /// Returns true if this error is related to permissions.
    #[must_use]
    pub fn is_permission(&self) -> bool {
        matches!(self, Error::Permission { .. })
    }

    /// Returns true if this error is a "not found" error.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::NotFound { .. })
    }

    /// Returns true if this error is related to rate limiting.
    #[must_use]
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, Error::RateLimit { .. })
    }

    /// Returns true if this error is a bad request.
    #[must_use]
    pub fn is_bad_request(&self) -> bool {
        matches!(self, Error::BadRequest { .. })
    }

    /// Returns true if this error is a timeout.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Error::Timeout { .. })
    }

    /// Returns true if this error is an abort.
    #[must_use]
    pub fn is_abort(&self) -> bool {
        matches!(self, Error::Abort { .. })
    }

    /// Returns true if this error is a connection error.
    #[must_use]
    pub fn is_connection(&self) -> bool {
        matches!(self, Error::Connection { .. })
    }

    /// Returns true if this error is a server error: any HTTP 5xx status.
    #[must_use]
    pub fn is_server_error(&self) -> bool {
        matches!(
            self,
            Error::InternalServer { .. }
                | Error::ServiceUnavailable { .. }
                | Error::Overloaded { .. }
        ) || matches!(self.status_code(), Some(500..=599))
    }

    /// Strict policy: only pre-header connection failures and explicit retryable HTTP statuses.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Connection { .. } => true,
            error if error.metadata().is_some() => {
                matches!(error.status_code(), Some(408 | 409 | 429 | 500..=599))
            }
            _ => false,
        }
    }
    /// Returns true if this error is a [`Error::ToDo`].
    #[must_use]
    pub fn is_todo(&self) -> bool {
        matches!(self, Self::ToDo { .. })
    }
    /// Returns true if this error is a validation error.
    #[must_use]
    pub fn is_validation(&self) -> bool {
        matches!(self, Self::Validation { .. })
    }
    /// HTTP diagnostics, when the error came from a response.
    #[must_use]
    pub fn metadata(&self) -> Option<&HttpMetadata> {
        match self {
            Self::Api { metadata, .. }
            | Self::Authentication { metadata, .. }
            | Self::Permission { metadata, .. }
            | Self::NotFound { metadata, .. }
            | Self::RateLimit { metadata, .. }
            | Self::BadRequest { metadata, .. }
            | Self::InternalServer { metadata, .. }
            | Self::ServiceUnavailable { metadata, .. }
            | Self::Overloaded { metadata, .. } => Some(metadata),
            Self::Timeout { metadata, .. } => metadata.as_deref(),
            _ => None,
        }
    }
    /// The response's `request-id`, when known.
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.metadata().and_then(|m| m.request_id.as_deref())
    }
    /// The response's HTTP status, when the error came from a response.
    #[must_use]
    pub fn status_code(&self) -> Option<u16> {
        self.metadata().map(|m| m.status)
    }
    /// The delay the server asked for in `retry-after`, when it sent one.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        self.metadata().and_then(|m| m.retry_after)
    }
    #[must_use]
    pub(crate) fn with_http_metadata(self, metadata: HttpMetadata) -> Self {
        match self {
            Self::Timeout {
                message, source, ..
            } => Self::Timeout {
                message,
                source,
                metadata: Some(Box::new(metadata)),
            },
            error => Self::from_http(error.to_string(), metadata, Some(Box::new(error))),
        }
    }

    #[must_use]
    pub(crate) fn with_request_id(mut self, request_id: Option<String>) -> Self {
        if request_id.is_none() {
            return self;
        }
        match &mut self {
            Self::Api { metadata, .. }
            | Self::Authentication { metadata, .. }
            | Self::Permission { metadata, .. }
            | Self::NotFound { metadata, .. }
            | Self::RateLimit { metadata, .. }
            | Self::BadRequest { metadata, .. }
            | Self::InternalServer { metadata, .. }
            | Self::ServiceUnavailable { metadata, .. }
            | Self::Overloaded { metadata, .. } => metadata.request_id = request_id,
            _ => {
                return self.with_http_metadata(HttpMetadata {
                    status: 200,
                    request_id,
                    ..Default::default()
                });
            }
        }
        self
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api { message, .. } => write!(f, "Api: {message}")?,
            Self::Authentication { message, .. } => write!(f, "Authentication: {message}")?,
            Self::Permission { message, .. } => write!(f, "Permission: {message}")?,
            Self::NotFound { message, .. } => write!(f, "NotFound: {message}")?,
            Self::RateLimit { message, .. } => write!(f, "RateLimit: {message}")?,
            Self::BadRequest { message, .. } => write!(f, "BadRequest: {message}")?,
            Self::InternalServer { message, .. } => write!(f, "InternalServer: {message}")?,
            Self::ServiceUnavailable { message, .. } => write!(f, "ServiceUnavailable: {message}")?,
            Self::Overloaded { message, .. } => write!(f, "Overloaded: {message}")?,
            Self::Timeout { message, .. } => write!(f, "Timeout: {message}")?,
            Self::Abort { message, .. } => write!(f, "Abort: {message}")?,
            Self::Connection { message, .. } => write!(f, "Connection: {message}")?,
            Self::Serialization { message, .. } => write!(f, "Serialization: {message}")?,
            Self::Io { message, .. } => write!(f, "Io: {message}")?,
            Self::HttpClient { message, .. } => write!(f, "HttpClient: {message}")?,
            Self::Validation { message, .. } => write!(f, "Validation: {message}")?,
            Self::Url { message, .. } => write!(f, "Url: {message}")?,
            Self::Streaming { message, .. } => write!(f, "Streaming: {message}")?,
            Self::Encoding { message, .. } => write!(f, "Encoding: {message}")?,
            Self::Sandbox { message, .. } => write!(f, "Sandbox: {message}")?,
            Self::Unknown { message, .. } => write!(f, "Unknown: {message}")?,
            Self::ToDo { message, .. } => write!(f, "ToDo: {message}")?,
        }
        if let Some(id) = self.request_id() {
            write!(f, " (Request ID: {id})")?;
        }
        Ok(())
    }
}
impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Error::Api { source, .. }
            | Error::Authentication { source, .. }
            | Error::Permission { source, .. }
            | Error::NotFound { source, .. }
            | Error::RateLimit { source, .. }
            | Error::BadRequest { source, .. }
            | Error::InternalServer { source, .. }
            | Error::ServiceUnavailable { source, .. }
            | Error::Overloaded { source, .. }
            | Error::Timeout { source, .. }
            | Error::Connection { source, .. } => source
                .as_ref()
                .map(|e| e.as_ref() as &(dyn error::Error + 'static)),
            Error::Serialization { source, .. } => source
                .as_ref()
                .map(|e| e.as_ref() as &(dyn error::Error + 'static)),
            Error::Io { source, .. } => Some(source),
            Error::HttpClient { source, .. } => source
                .as_ref()
                .map(|e| e.as_ref() as &(dyn error::Error + 'static)),
            Error::Url { source, .. } => {
                source.as_ref().map(|e| e as &(dyn error::Error + 'static))
            }
            Error::Streaming { source, .. } => source
                .as_ref()
                .map(|e| e.as_ref() as &(dyn error::Error + 'static)),
            Error::Encoding { source, .. } => source
                .as_ref()
                .map(|e| e.as_ref() as &(dyn error::Error + 'static)),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::io(err.to_string(), err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::serialization(format!("JSON error: {err}"), Some(Box::new(err)))
    }
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Self {
        Error::url(format!("URL parse error: {err}"), Some(err))
    }
}

impl From<Utf8Error> for Error {
    fn from(err: Utf8Error) -> Self {
        Error::encoding(format!("UTF-8 error: {err}"), Some(Box::new(err)))
    }
}

/// A specialized Result type for Topcoat SI operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Phase determines whether a transport failure is safe to retry.
#[derive(Clone, Copy)]
pub(crate) enum RequestPhase {
    Headers,
    Body,
}

pub(crate) fn map_reqwest_error(err: reqwest::Error, phase: RequestPhase) -> Error {
    let mut message = err.to_string();
    let mut source = error::Error::source(&err);
    while let Some(inner) = source {
        message.push_str(": ");
        message.push_str(&inner.to_string());
        source = inner.source();
    }
    if err.is_timeout() {
        Error::Timeout {
            message,
            source: Some(Arc::new(err)),
            metadata: None,
        }
    } else if err.is_builder() {
        Error::http_client(message, Some(Box::new(err)))
    } else if matches!(phase, RequestPhase::Headers) && (err.is_connect() || err.is_request()) {
        Error::connection(message, Some(Box::new(err)))
    } else {
        Error::http_client(message, Some(Box::new(err)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_5xx_status_is_a_server_error() {
        for status in [500, 501, 503, 529, 599] {
            let error = Error::api(status, None, "failed", None);
            assert!(error.is_server_error(), "{status}");
        }
        for status in [400, 429, 600] {
            let error = Error::api(status, None, "failed", None);
            assert!(!error.is_server_error(), "{status}");
        }
    }
}
