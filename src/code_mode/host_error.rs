//! Errors a host function returns to the sandbox.

use monty_types::ExcType;

use crate::error::Error;

/// A Python exception a host function can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PyError {
    /// `ValueError`.
    ValueError,
    /// `TypeError`.
    TypeError,
    /// `KeyError`.
    KeyError,
    /// `LookupError`.
    LookupError,
    /// `RuntimeError`.
    RuntimeError,
}

impl PyError {
    pub(super) fn exc_type(self) -> ExcType {
        match self {
            Self::ValueError => ExcType::ValueError,
            Self::TypeError => ExcType::TypeError,
            Self::KeyError => ExcType::KeyError,
            Self::LookupError => ExcType::LookupError,
            Self::RuntimeError => ExcType::RuntimeError,
        }
    }
}

/// Why a host function failed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum HostError {
    /// The arguments don't fit the function: Python `TypeError`, not charged to the budget.
    BadArgs(String),
    /// A problem Claude or the script can handle: becomes this Python exception.
    Raise {
        /// The exception type.
        kind: PyError,
        /// The exception message.
        message: String,
    },
    /// A problem nobody in the sandbox can fix: ends the run.
    Fatal(Error),
}
