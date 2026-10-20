use serde::{Deserialize, Serialize};

use crate::types::{
    ContainerInfo, ContextManagementResponse, MessageDeltaUsage, StopDetails, StopReason,
};

/// The delta information for a message delta event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageDelta {
    /// The reason the model stopped generating text, if it has stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,

    /// If the model stopped because it encountered a stop sequence, this field
    /// contains the specific stop sequence that was encountered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,

    /// Additional refusal information, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<Box<StopDetails>>,

    /// The container used by the request, when it changed or became known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<ContainerInfo>,
}

impl MessageDelta {
    /// Create a new empty `MessageDelta`.
    pub fn new() -> Self {
        Self {
            stop_reason: None,
            stop_sequence: None,
            stop_details: None,
            container: None,
        }
    }

    /// Set the stop reason.
    pub fn with_stop_reason(mut self, stop_reason: StopReason) -> Self {
        self.stop_reason = Some(stop_reason);
        self
    }

    /// Set the stop sequence.
    pub fn with_stop_sequence(mut self, stop_sequence: String) -> Self {
        self.stop_sequence = Some(stop_sequence);
        self
    }
}

impl Default for MessageDelta {
    fn default() -> Self {
        Self::new()
    }
}

/// An event that represents a delta update to a message in a streaming response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageDeltaEvent {
    /// The delta information for the message.
    pub delta: MessageDelta,

    /// The usage information for the message.
    pub usage: MessageDeltaUsage,

    /// Context-management edits applied to the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_management: Option<ContextManagementResponse>,
}

impl MessageDeltaEvent {
    /// Create a new `MessageDeltaEvent` with the given delta and usage.
    pub fn new(delta: MessageDelta, usage: MessageDeltaUsage) -> Self {
        Self {
            delta,
            usage,
            context_management: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn message_delta_empty() {
        let delta = MessageDelta::new();
        let json = to_value(&delta).unwrap();

        assert_eq!(json, json!({}));
    }

    #[test]
    fn message_delta_with_values() {
        let delta = MessageDelta::new()
            .with_stop_reason(StopReason::EndTurn)
            .with_stop_sequence("###".to_string());

        let json = to_value(&delta).unwrap();

        assert_eq!(
            json,
            json!({
                "stop_reason": "end_turn",
                "stop_sequence": "###"
            })
        );
    }

    #[test]
    fn message_delta_event_serialization() {
        let delta = MessageDelta::new().with_stop_reason(StopReason::EndTurn);

        let usage = MessageDeltaUsage::new(100).with_input_tokens(50);

        let event = MessageDeltaEvent::new(delta, usage);
        let json = to_value(&event).unwrap();

        assert_eq!(
            json,
            json!({
                "delta": {
                    "stop_reason": "end_turn"
                },
                "usage": {
                    "input_tokens": 50,
                    "output_tokens": 100
                }
            })
        );
    }

    #[test]
    fn message_delta_event_deserialization() {
        let json = json!({
            "delta": {
                "stop_reason": "end_turn",
                "stop_sequence": "###"
            },
            "usage": {
                "input_tokens": 50,
                "output_tokens": 100
            }
        });

        let event: MessageDeltaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.delta.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(event.delta.stop_sequence, Some("###".to_string()));
        assert_eq!(event.usage.input_tokens, Some(50));
        assert_eq!(event.usage.output_tokens, 100);
    }

    #[test]
    fn message_delta_event_deserializes_container_and_context_management() {
        let event: MessageDeltaEvent = serde_json::from_value(json!({
            "delta": {
                "stop_reason": "compaction",
                "container": {"id": "container_011"}
            },
            "usage": {"output_tokens": 15},
            "context_management": {
                "applied_edits": [{"type": "clear_thinking_20251015", "cleared_thinking_turns": 3}]
            }
        }))
        .unwrap();
        assert_eq!(event.delta.stop_reason, Some(StopReason::Compaction));
        assert_eq!(event.delta.container.unwrap().id, "container_011");
        assert_eq!(
            event.context_management.unwrap().applied_edits[0].cleared_thinking_turns,
            Some(3)
        );
    }
}
