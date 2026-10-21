use serde::{Deserialize, Deserializer, Serialize};

use crate::types::open_union;

/// Opt-in request for prompt-cache diagnostics.
///
/// Send it with `previous_message_id: None` on the first turn to opt in, and
/// pass the previous response `id` on later turns so the API can compare the two
/// requests and report where they diverged. Generally available; no beta header.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiagnosticsRequest {
    /// The `id` of the previous response, or `None` on the first turn.
    ///
    /// The key is always serialized; `null` on the first turn is what opts in.
    #[serde(default)]
    pub previous_message_id: Option<String>,
}

impl DiagnosticsRequest {
    /// Opt in on the first turn of a conversation.
    #[must_use]
    pub fn first_turn() -> Self {
        Self {
            previous_message_id: None,
        }
    }

    /// Opt in on a later turn, comparing against the previous response.
    #[must_use]
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
#[non_exhaustive]
pub struct CacheDiagnostics {
    /// Why the cache missed, or `None` while the comparison is still running.
    #[serde(default)]
    pub cache_miss_reason: Option<CacheMissReason>,
}

/// The earliest divergence between this request and the previous one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct CacheMissReason {
    /// Where the request diverged.
    pub r#type: CacheMissType,

    /// Input tokens that missed the cache because of the divergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_missed_input_tokens: Option<u64>,
}

impl CacheMissReason {
    /// Whether the miss is attributable to one of the four `*_changed` divergences.
    #[must_use]
    pub fn is_changed(&self) -> bool {
        matches!(
            self.r#type,
            CacheMissType::ModelChanged
                | CacheMissType::SystemChanged
                | CacheMissType::ToolsChanged
                | CacheMissType::MessagesChanged
        )
    }
}

/// The kind of [`CacheMissReason`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CacheMissType {
    /// The model or speed differs from the previous request.
    ModelChanged,
    /// The system prompt differs from the previous request.
    SystemChanged,
    /// The tools differ from the previous request.
    ToolsChanged,
    /// An earlier message differs from the previous request.
    MessagesChanged,
    /// The API has no record of the previous message.
    PreviousMessageNotFound,
    /// The comparison could not be made.
    Unavailable,
    /// A kind this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for CacheMissType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "CacheMissType",
            |value| match value {
                "model_changed" => Some(Self::ModelChanged),
                "system_changed" => Some(Self::SystemChanged),
                "tools_changed" => Some(Self::ToolsChanged),
                "messages_changed" => Some(Self::MessagesChanged),
                "previous_message_not_found" => Some(Self::PreviousMessageNotFound),
                "unavailable" => Some(Self::Unavailable),
                _ => None,
            },
            Self::Unknown,
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
        assert_eq!(reason.r#type, CacheMissType::SystemChanged);
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
        let make = |t: CacheMissType| CacheMissReason {
            r#type: t,
            cache_missed_input_tokens: None,
        };
        for t in [
            CacheMissType::ModelChanged,
            CacheMissType::SystemChanged,
            CacheMissType::ToolsChanged,
            CacheMissType::MessagesChanged,
        ] {
            assert!(make(t).is_changed(), "{t:?}");
        }
        assert!(!make(CacheMissType::PreviousMessageNotFound).is_changed());
        assert!(!make(CacheMissType::Unavailable).is_changed());
    }
}
