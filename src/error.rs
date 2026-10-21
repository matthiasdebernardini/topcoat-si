//! Errors preserve transport sources and HTTP diagnostics without response bodies.
use std::{error, fmt, io, str::Utf8Error, sync::Arc, time::Duration};

/// Diagnostics shared by every HTTP response error.
#[derive(Clone, Debug, Default)]
pub struct HttpMetadata {
    /// Status.
    pub status: u16,
    /// Error type.
    pub error_type: Option<String>,
    /// Request id.
    pub request_id: Option<String>,
    /// Retry after.
    pub retry_after: Option<Duration>,
}

/// SDK failure, with original sources retained when available.
#[derive(Clone, Debug)]
pub enum Error {
    /// Api failure.
    Api {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// Authentication failure.
    Authentication {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// Permission failure.
    Permission {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// NotFound failure.
    NotFound {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// RateLimit failure.
    RateLimit {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// BadRequest failure.
    BadRequest {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// InternalServer failure.
    InternalServer {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// ServiceUnavailable failure.
    ServiceUnavailable {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// Overloaded failure.
    Overloaded {
        /// message associated with this failure.
        message: String,
        /// metadata associated with this failure.
        metadata: HttpMetadata,
        /// source associated with this failure.
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    },
    /// Timeout failure.
    Timeout {
        /// Response diagnostics, when headers were received.
        metadata: Option<Box<HttpMetadata>>,
        /// message associated with this failure.
        message: String,
        /// duration associated with this failure.
        duration: Option<f64>,
        /// source associated with this failure.
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
    /// From http.
    pub fn from_http(
        message: impl Into<String>,
        metadata: HttpMetadata,
        source: Option<Arc<dyn error::Error + Send + Sync>>,
    ) -> Self {
        let message = message.into();
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
    /// Api.
    pub fn api(
        status: u16,
        error_type: Option<String>,
        message: String,
        request_id: Option<String>,
    ) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status,
                error_type,
                request_id,
                retry_after: None,
            },
            None,
        )
    }
    /// Timeout.
    pub fn timeout(message: impl Into<String>, duration: Option<f64>) -> Self {
        Self::Timeout {
            message: message.into(),
            duration,
            source: None,
            metadata: None,
        }
    }
    /// Authentication.
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
    /// Permission.
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
    /// Not found.
    pub fn not_found(
        message: impl Into<String>,
        _resource_type: Option<String>,
        _resource_id: Option<String>,
    ) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 404,
                ..Default::default()
            },
            None,
        )
    }
    /// Rate limit.
    pub fn rate_limit(message: impl Into<String>, retry_after: Option<u64>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 429,
                retry_after: retry_after.map(Duration::from_secs),
                ..Default::default()
            },
            None,
        )
    }
    /// Bad request.
    pub fn bad_request(message: impl Into<String>, _param: Option<String>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 400,
                ..Default::default()
            },
            None,
        )
    }
    /// Internal server.
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
    /// Service unavailable.
    pub fn service_unavailable(message: impl Into<String>, retry_after: Option<u64>) -> Self {
        Self::from_http(
            message,
            HttpMetadata {
                status: 503,
                retry_after: retry_after.map(Duration::from_secs),
                ..Default::default()
            },
            None,
        )
    }
    /// Creates a new abort error.
    pub fn abort(message: impl Into<String>) -> Self {
        Error::Abort {
            message: message.into(),
        }
    }

    /// Creates a new connection error.
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
    pub fn io(message: impl Into<String>, source: io::Error) -> Self {
        Error::Io {
            message: message.into(),
            source: Arc::new(source),
        }
    }

    /// Creates a new HTTP client error.
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
    pub fn validation(message: impl Into<String>, param: Option<String>) -> Self {
        Error::Validation {
            message: message.into(),
            param,
        }
    }

    /// Creates a new URL error.
    pub fn url(message: impl Into<String>, source: Option<url::ParseError>) -> Self {
        Error::Url {
            message: message.into(),
            source,
        }
    }

    /// Creates a new streaming error.
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
    pub fn sandbox(message: impl Into<String>) -> Self {
        Error::Sandbox {
            message: message.into(),
        }
    }

    /// Creates a new unknown error.
    pub fn unknown(message: impl Into<String>) -> Self {
        Error::Unknown {
            message: message.into(),
        }
    }

    /// Creates a new ToDo error for unimplemented functionality.
    pub fn todo(message: impl Into<String>) -> Self {
        Error::ToDo {
            message: message.into(),
        }
    }

    /// Returns true if this error is related to authentication.
    pub fn is_authentication(&self) -> bool {
        matches!(self, Error::Authentication { .. })
    }

    /// Returns true if this error is related to permissions.
    pub fn is_permission(&self) -> bool {
        matches!(self, Error::Permission { .. })
    }

    /// Returns true if this error is a "not found" error.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::NotFound { .. })
    }

    /// Returns true if this error is related to rate limiting.
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, Error::RateLimit { .. })
    }

    /// Returns true if this error is a bad request.
    pub fn is_bad_request(&self) -> bool {
        matches!(self, Error::BadRequest { .. })
    }

    /// Returns true if this error is a timeout.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Error::Timeout { .. })
    }

    /// Returns true if this error is an abort.
    pub fn is_abort(&self) -> bool {
        matches!(self, Error::Abort { .. })
    }

    /// Returns true if this error is a connection error.
    pub fn is_connection(&self) -> bool {
        matches!(self, Error::Connection { .. })
    }

    /// Returns true if this error is a server error.
    pub fn is_server_error(&self) -> bool {
        matches!(
            self,
            Error::InternalServer { .. }
                | Error::ServiceUnavailable { .. }
                | Error::Overloaded { .. }
        )
    }

    /// Strict policy: only pre-header connection failures and explicit retryable HTTP statuses.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Connection { .. } => true,
            error if error.metadata().is_some() => {
                matches!(error.status_code(), Some(408 | 409 | 429 | 500..=599))
            }
            _ => false,
        }
    }
    /// Is todo.
    pub fn is_todo(&self) -> bool {
        matches!(self, Self::ToDo { .. })
    }
    /// Is validation.
    pub fn is_validation(&self) -> bool {
        matches!(self, Self::Validation { .. })
    }
    /// Metadata.
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
    /// Request id.
    pub fn request_id(&self) -> Option<&str> {
        self.metadata().and_then(|m| m.request_id.as_deref())
    }
    /// Status code.
    pub fn status_code(&self) -> Option<u16> {
        self.metadata().map(|m| m.status)
    }
    /// Retry after.
    pub fn retry_after(&self) -> Option<Duration> {
        self.metadata().and_then(|m| m.retry_after)
    }
    pub(crate) fn with_http_metadata(self, metadata: HttpMetadata) -> Self {
        match self {
            Self::Timeout {
                message,
                duration,
                source,
                ..
            } => Self::Timeout {
                message,
                duration,
                source,
                metadata: Some(Box::new(metadata)),
            },
            error => Self::from_http(error.to_string(), metadata, Some(Arc::new(error))),
        }
    }

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
    let mut source = std::error::Error::source(&err);
    while let Some(inner) = source {
        message.push_str(": ");
        message.push_str(&inner.to_string());
        source = inner.source();
    }
    if err.is_timeout() {
        Error::Timeout {
            message,
            duration: None,
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
