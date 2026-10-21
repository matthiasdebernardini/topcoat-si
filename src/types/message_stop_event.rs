use serde::{Deserialize, Serialize};

/// An event that represents the end of a message in a streaming response.
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct MessageStopEvent {}

impl MessageStopEvent {
    /// Create a new `MessageStopEvent`.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn message_stop_event_serialization() {
        let event = MessageStopEvent::new();

        let json = to_value(event).unwrap();
        assert_eq!(json, json!({}));
    }

    #[test]
    fn message_stop_event_deserialization() {
        let json = json!({});

        let event: MessageStopEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event, MessageStopEvent::new());
    }
}
