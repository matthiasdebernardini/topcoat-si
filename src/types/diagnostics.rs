use serde::{Deserialize, Serialize};

/// Opt-in request for prompt-cache diagnostics.
///
/// Send it with `previous_message_id: None` on the first turn to opt in, and
/// pass the previous response `id` on later turns so the API can compare the two
/// requests and report where they diverged. Generally available; no beta header.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticsRequest {
    /// The `id` of the previous response, or `None` on the first turn.
    ///
    /// The key is always serialized; `null` on the first turn is what opts in.
    #[serde(default)]
    pub previous_message_id: Option<String>,
}

impl DiagnosticsRequest {
    /// Opt in on the first turn of a conversation.
    pub fn first_turn() -> Self {
        Self {
            previous_message_id: None,
        }
    }

    /// Opt in on a later turn, comparing against the previous response.
    pub fn after(previous_message_id: impl Into<String>) -> Self {
        Self {
            previous_message_id: Some(previous_message_id.into()),
        }
    }
}

/// Cache diagnostics reported on a response.
///
/// The `diagnostics` field on a response has three meanings:
/// - absent or `null`: there was no comparison, or no divergence was found;
/// - `{"cache_miss_reason": null}`: the comparison is still running;
/// - a [`CacheMissReason`]: the earliest point at which the request diverged
///   from the previous one (`unavailable` also covers differences in
///   `tool_choice`, `thinking`, `output_config` or beta headers).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheDiagnostics {
    /// Why the cache missed, or `None` while the comparison is still running.
    #[serde(default)]
    pub cache_miss_reason: Option<CacheMissReason>,
}

/// The earliest divergence between this request and the previous one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheMissReason {
    /// One of the `*_CHANGED`, `PREVIOUS_MESSAGE_NOT_FOUND` or `UNAVAILABLE` types.
    pub r#type: String,

    /// Input tokens that missed the cache because of the divergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_missed_input_tokens: Option<u64>,
}

impl CacheMissReason {
    /// The model differs from the previous request.
    pub const MODEL_CHANGED: &'static str = "model_changed";
    /// The system prompt differs from the previous request.
    pub const SYSTEM_CHANGED: &'static str = "system_changed";
    /// The tools differ from the previous request.
    pub const TOOLS_CHANGED: &'static str = "tools_changed";
    /// The messages differ from the previous request.
    pub const MESSAGES_CHANGED: &'static str = "messages_changed";
    /// The previous message could not be found.
    pub const PREVIOUS_MESSAGE_NOT_FOUND: &'static str = "previous_message_not_found";
    /// The comparison could not be made.
    pub const UNAVAILABLE: &'static str = "unavailable";

    /// Whether the miss is attributable to one of the four `*_changed` divergences.
    pub fn is_changed(&self) -> bool {
        matches!(
            self.r#type.as_str(),
            Self::MODEL_CHANGED
                | Self::SYSTEM_CHANGED
                | Self::TOOLS_CHANGED
                | Self::MESSAGES_CHANGED
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_turn_request_emits_null() {
        let value = serde_json::to_value(DiagnosticsRequest::first_turn()).unwrap();
        assert_eq!(value, json!({"previous_message_id": null}));
    }

    #[test]
    fn later_turn_request_carries_id() {
        let value = serde_json::to_value(DiagnosticsRequest::after("msg_01")).unwrap();
        assert_eq!(value, json!({"previous_message_id": "msg_01"}));
    }

    #[test]
    fn miss_reason_deserialization() {
        let diagnostics: CacheDiagnostics = serde_json::from_value(json!({
            "cache_miss_reason": {"type": "system_changed", "cache_missed_input_tokens": 41850}
        }))
        .unwrap();
        let reason = diagnostics.cache_miss_reason.unwrap();
        assert_eq!(reason.r#type, CacheMissReason::SYSTEM_CHANGED);
        assert_eq!(reason.cache_missed_input_tokens, Some(41850));
        assert!(reason.is_changed());
    }

    #[test]
    fn running_comparison_deserialization() {
        let diagnostics: CacheDiagnostics =
            serde_json::from_value(json!({"cache_miss_reason": null})).unwrap();
        assert_eq!(diagnostics.cache_miss_reason, None);
    }

    #[test]
    fn is_changed_only_for_changed_types() {
        let make = |t: &str| CacheMissReason {
            r#type: t.to_string(),
            cache_missed_input_tokens: None,
        };
        for t in [
            CacheMissReason::MODEL_CHANGED,
            CacheMissReason::SYSTEM_CHANGED,
            CacheMissReason::TOOLS_CHANGED,
            CacheMissReason::MESSAGES_CHANGED,
        ] {
            assert!(make(t).is_changed(), "{t}");
        }
        assert!(!make(CacheMissReason::PREVIOUS_MESSAGE_NOT_FOUND).is_changed());
        assert!(!make(CacheMissReason::UNAVAILABLE).is_changed());
    }
}
