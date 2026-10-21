use serde::{Deserialize, Serialize};

/// Whether a server tool's results are included in the model's context.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResponseInclusion {
    /// Results are included in full.
    Full,
    /// Results are excluded from the model's context.
    Excluded,
}
