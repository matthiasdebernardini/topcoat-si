use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

/// A compaction block holding a summary of earlier conversation.
///
/// Only fields that are `Some` are serialized; the API rejects null fields on this block.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CompactionBlock {
    /// The summary text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Signature authenticating the compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// Encrypted form of the summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,

    /// Tool changes recorded by the compaction, kept verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_changes: Option<serde_json::Value>,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}

impl CompactionBlock {
    /// Creates a compaction block with the given summary text.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            ..Self::default()
        }
    }

    /// Sets the signature.
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }
}
