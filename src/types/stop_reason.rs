use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::types::open_union;

/// Reasons why the model stopped generating a response.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopReason {
    /// The model reached the end of a generated turn
    EndTurn,

    /// The response reached the maximum token limit for the response
    MaxTokens,

    /// The model reached a specified stop sequence
    StopSequence,

    /// The model indicated it wants to use a tool
    ToolUse,

    /// The model paused in the middle of a turn
    PauseTurn,

    /// The model refused to respond due to safety or other considerations
    Refusal,

    /// The response stopped because it exceeded the model's context window
    ModelContextWindowExceeded,

    /// The response stopped after producing a compaction block.
    ///
    /// The caller re-sends the conversation with the `compaction` block first in
    /// `messages` and the summarized messages removed.
    Compaction,

    /// A value introduced by a newer API version.
    Unknown,
}

impl<'de> Deserialize<'de> for StopReason {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "StopReason",
            |value| match value {
                "end_turn" => Some(Self::EndTurn),
                "max_tokens" => Some(Self::MaxTokens),
                "stop_sequence" => Some(Self::StopSequence),
                "tool_use" => Some(Self::ToolUse),
                "pause_turn" => Some(Self::PauseTurn),
                "refusal" => Some(Self::Refusal),
                "model_context_window_exceeded" => Some(Self::ModelContextWindowExceeded),
                "compaction" => Some(Self::Compaction),
                _ => None,
            },
            Self::Unknown,
        )
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::Unknown => write!(f, "unknown"),
            StopReason::EndTurn => write!(f, "end_turn"),
            StopReason::MaxTokens => write!(f, "max_tokens"),
            StopReason::StopSequence => write!(f, "stop_sequence"),
            StopReason::ToolUse => write!(f, "tool_use"),
            StopReason::PauseTurn => write!(f, "pause_turn"),
            StopReason::Refusal => write!(f, "refusal"),
            StopReason::ModelContextWindowExceeded => write!(f, "model_context_window_exceeded"),
            StopReason::Compaction => write!(f, "compaction"),
        }
    }
}

/// Error returned when parsing an invalid stop reason string.
///
/// This error contains the invalid string value that could not be parsed
/// into a valid `StopReason` variant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StopReasonParseError {
    /// The invalid string value that could not be parsed.
    pub invalid_value: String,
}

impl fmt::Display for StopReasonParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unknown stop reason: {}", self.invalid_value)
    }
}

impl std::error::Error for StopReasonParseError {}

impl FromStr for StopReason {
    type Err = StopReasonParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "end_turn" => Ok(StopReason::EndTurn),
            "max_tokens" => Ok(StopReason::MaxTokens),
            "stop_sequence" => Ok(StopReason::StopSequence),
            "tool_use" => Ok(StopReason::ToolUse),
            "pause_turn" => Ok(StopReason::PauseTurn),
            "refusal" => Ok(StopReason::Refusal),
            "model_context_window_exceeded" => Ok(StopReason::ModelContextWindowExceeded),
            "compaction" => Ok(StopReason::Compaction),
            _ => Err(StopReasonParseError {
                invalid_value: s.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization() {
        let reason = StopReason::EndTurn;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, r#""end_turn""#);

        let reason = StopReason::MaxTokens;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, r#""max_tokens""#);
    }

    #[test]
    fn deserialization() {
        let json = r#""end_turn""#;
        let reason: StopReason = serde_json::from_str(json).unwrap();
        assert_eq!(reason, StopReason::EndTurn);

        let json = r#""stop_sequence""#;
        let reason: StopReason = serde_json::from_str(json).unwrap();
        assert_eq!(reason, StopReason::StopSequence);
    }

    #[test]
    fn display() {
        let reason = StopReason::EndTurn;
        assert_eq!(reason.to_string(), "end_turn");

        let reason = StopReason::MaxTokens;
        assert_eq!(reason.to_string(), "max_tokens");
    }

    #[test]
    fn serialization_model_context_window_exceeded() {
        let reason = StopReason::ModelContextWindowExceeded;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, r#""model_context_window_exceeded""#);
    }

    #[test]
    fn deserialization_model_context_window_exceeded() {
        let json = r#""model_context_window_exceeded""#;
        let reason: StopReason = serde_json::from_str(json).unwrap();
        assert_eq!(reason, StopReason::ModelContextWindowExceeded);
    }

    #[test]
    fn display_model_context_window_exceeded() {
        let reason = StopReason::ModelContextWindowExceeded;
        assert_eq!(reason.to_string(), "model_context_window_exceeded");
    }

    #[test]
    fn compaction_round_trip() {
        let reason: StopReason = serde_json::from_str(r#""compaction""#).unwrap();
        assert_eq!(reason, StopReason::Compaction);
        assert_eq!(serde_json::to_string(&reason).unwrap(), r#""compaction""#);
        assert_eq!(reason.to_string(), "compaction");
        assert_eq!(
            "compaction".parse::<StopReason>().unwrap(),
            StopReason::Compaction
        );
    }
}
