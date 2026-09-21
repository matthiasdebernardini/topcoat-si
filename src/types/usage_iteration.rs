use crate::types::Model;
use serde::{Deserialize, Serialize};

/// Token counts for an individual inference or compaction iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationUsage {
    /// Model for this iteration, when supplied by the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// Input tokens consumed.
    pub input_tokens: i32,
    /// Output tokens consumed.
    pub output_tokens: i32,
    /// Tokens written to the cache.
    pub cache_creation_input_tokens: i32,
    /// Tokens read from the cache.
    pub cache_read_input_tokens: i32,
}

/// Each known API iteration kind, accepting future kinds without losing the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UsageIteration {
    /// Primary inference; model may be absent or null.
    Message(IterationUsage),
    /// Fallback inference.
    FallbackMessage(IterationUsage),
    /// Context compaction.
    Compaction(IterationUsage),
    /// Advisor sub-inference.
    AdvisorMessage(IterationUsage),
    /// An iteration introduced by a newer API.
    #[serde(other)]
    Unknown,
}
