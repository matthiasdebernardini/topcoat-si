use serde::{Deserialize, Deserializer, Serialize};

use crate::types::open_union;

/// The `type` of an API error, as in `{"error": {"type": "invalid_request_error"}}`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorType {
    /// 400: the format or content of the request is wrong.
    InvalidRequestError,
    /// 401: the API key is missing, malformed, revoked or expired.
    AuthenticationError,
    /// 402: a billing or payment problem.
    BillingError,
    /// 403: the API key may not use the resource.
    PermissionError,
    /// 404: the resource was not found.
    NotFoundError,
    /// 409: the request conflicts with the current state of a resource.
    ConflictError,
    /// 413: the request is larger than the endpoint allows.
    RequestTooLarge,
    /// 429: a rate limit or spend cap was hit.
    RateLimitError,
    /// 500: an unexpected error inside the API.
    ApiError,
    /// 504: the request timed out while processing.
    TimeoutError,
    /// 529: the API is temporarily overloaded.
    OverloadedError,
    /// An error type this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for ErrorType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "ErrorType",
            |value| match value {
                "invalid_request_error" => Some(Self::InvalidRequestError),
                "authentication_error" => Some(Self::AuthenticationError),
                "billing_error" => Some(Self::BillingError),
                "permission_error" => Some(Self::PermissionError),
                "not_found_error" => Some(Self::NotFoundError),
                "conflict_error" => Some(Self::ConflictError),
                "request_too_large" => Some(Self::RequestTooLarge),
                "rate_limit_error" => Some(Self::RateLimitError),
                "api_error" => Some(Self::ApiError),
                "timeout_error" => Some(Self::TimeoutError),
                "overloaded_error" => Some(Self::OverloadedError),
                _ => None,
            },
            Self::Unknown,
        )
    }
}

impl ErrorType {
    /// The HTTP status the API documents for this error type; 500 for an unknown type.
    pub(crate) fn http_status(self) -> u16 {
        match self {
            Self::InvalidRequestError => 400,
            Self::AuthenticationError => 401,
            Self::BillingError => 402,
            Self::PermissionError => 403,
            Self::NotFoundError => 404,
            Self::ConflictError => 409,
            Self::RequestTooLarge => 413,
            Self::RateLimitError => 429,
            Self::ApiError | Self::Unknown => 500,
            Self::TimeoutError => 504,
            Self::OverloadedError => 529,
        }
    }
}
