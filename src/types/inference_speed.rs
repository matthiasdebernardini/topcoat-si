use serde::{Deserialize, Deserializer, Serialize};

use crate::types::open_union;

/// Inference speed of a request, a fallback attempt or a report row.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InferenceSpeed {
    /// Standard inference speed and pricing.
    Standard,
    /// Fast inference mode, where supported.
    Fast,
    /// A speed this crate does not know.
    ///
    /// Never send it; it only models values this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for InferenceSpeed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "InferenceSpeed",
            |value| match value {
                "standard" => Some(Self::Standard),
                "fast" => Some(Self::Fast),
                _ => None,
            },
            Self::Unknown,
        )
    }
}
