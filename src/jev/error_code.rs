//! Jev's error codes.

use crate::types::warn_unknown;

/// The code of a Jev error.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JevErrorCode {
    /// `max_tokens_exceeded`: the request is longer than the model's context length (400).
    MaxTokensExceeded,
    /// A 403 whose body is not JSON: an edge proxy in front of Jev blocked the request. Jev
    /// never sends this code; this client sets it.
    EdgeBlocked,
    /// A code this client does not know.
    Unknown,
}

impl JevErrorCode {
    /// The code as Jev sends it; `None` for a code Jev never sends or this client does not know.
    pub(crate) fn wire_name(self) -> Option<&'static str> {
        match self {
            Self::MaxTokensExceeded => Some("max_tokens_exceeded"),
            Self::EdgeBlocked | Self::Unknown => None,
        }
    }

    /// Reads a `detail.error_type`, logging a warning for a code this client does not know.
    pub(super) fn from_detail(code: &str) -> Self {
        match code {
            "max_tokens_exceeded" => Self::MaxTokensExceeded,
            other => {
                warn_unknown("JevErrorCode", other);
                Self::Unknown
            }
        }
    }
}
