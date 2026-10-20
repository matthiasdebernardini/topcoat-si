use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A request to compact the conversation on demand.
///
/// The response contains a `compaction` content block and
/// `stop_reason: "compaction"`. The caller must re-send the conversation with
/// that block first in `messages` and the summarized messages removed.
/// Mutually exclusive with `context_management`. Requires the
/// `compact-2026-09-04` beta, which is added automatically.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactionRequest {
    /// Custom summarization instructions.
    pub instructions: Option<String>,
}

impl CompactionRequest {
    /// Create a compaction request with default instructions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set custom summarization instructions.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

impl Serialize for CompactionRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map =
            serializer.serialize_map(Some(1 + usize::from(self.instructions.is_some())))?;
        map.serialize_entry("type", "summarize")?;
        if let Some(instructions) = &self.instructions {
            map.serialize_entry("instructions", instructions)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for CompactionRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            instructions: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            instructions: raw.instructions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialization_always_emits_type() {
        assert_eq!(
            serde_json::to_value(CompactionRequest::new()).unwrap(),
            json!({"type": "summarize"})
        );
        assert_eq!(
            serde_json::to_value(CompactionRequest::new().with_instructions("Keep names")).unwrap(),
            json!({"type": "summarize", "instructions": "Keep names"})
        );
    }

    #[test]
    fn deserialization() {
        let request: CompactionRequest =
            serde_json::from_value(json!({"type": "summarize", "instructions": "x"})).unwrap();
        assert_eq!(request.instructions.as_deref(), Some("x"));
    }
}
