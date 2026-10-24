//! Helpers shared by the open (forward-compatible) tagged unions and string enums.
//!
//! A value the API sends that this crate does not know becomes the type's catch-all variant
//! and is logged as a warning on the `carpa::unknown` target the first time it appears in
//! the process, so a new API value shows up in the logs instead of failing the whole reply.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{LazyLock, Mutex};

use serde::de::Visitor;
use serde::{Deserializer, Serialize, Serializer, de, ser};
use serde_json::{Map, Value};

/// How many distinct unknown values are remembered; past this, every one is logged.
const MAX_SEEN_UNKNOWN: usize = 1024;

/// How many chars of an unknown value are remembered and logged; the server sends it, so it
/// can be arbitrarily long.
const MAX_UNKNOWN_TAG_CHARS: usize = 128;

/// Marks a tag cut at [`MAX_UNKNOWN_TAG_CHARS`].
const TRUNCATED_MARKER: &str = "...[truncated]";

/// The unknown values already logged, by Rust type.
#[derive(Default)]
struct SeenUnknown {
    tags: HashMap<&'static str, HashSet<String>>,
    count: usize,
}

static SEEN_UNKNOWN: LazyLock<Mutex<SeenUnknown>> = LazyLock::new(Mutex::default);

/// Records `tag` for `type_name` and returns whether it was not seen before.
///
/// Past [`MAX_SEEN_UNKNOWN`] values, or when the lock is poisoned, nothing is recorded and every
/// value counts as new, so a warning is never lost.
fn first_sighting(type_name: &'static str, tag: &str) -> bool {
    let Ok(mut seen) = SEEN_UNKNOWN.lock() else {
        return true;
    };
    if seen
        .tags
        .get(type_name)
        .is_some_and(|tags| tags.contains(tag))
    {
        return false;
    }
    if seen.count < MAX_SEEN_UNKNOWN {
        seen.count += 1;
        seen.tags
            .entry(type_name)
            .or_default()
            .insert(tag.to_string());
    }
    true
}

/// Logs a warning for an API value `type_name` does not know, the first time each distinct
/// value appears in the process.
///
/// Only the type name and the unknown tag or string are logged, never the payload, which can
/// hold user content.
pub(crate) fn warn_unknown(type_name: &'static str, tag: &str) {
    let tag = crate::http::truncate_chars(tag, MAX_UNKNOWN_TAG_CHARS, TRUNCATED_MARKER);
    if !first_sighting(type_name, &tag) {
        return;
    }
    tracing::warn!(
        target: "carpa::unknown",
        rust_type = type_name,
        tag = tag.as_str(),
        "unknown API value; update carpa to read it"
    );
}

/// The validation error for an `Unknown` value in a request. It stands for an API value this
/// crate could not read, so it has nothing to send and would go out as the string `"unknown"`.
pub(crate) fn unknown_in_request(field: String) -> crate::Error {
    crate::Error::validation(
        format!("{field} is an unknown value read from a reply and cannot be sent"),
        Some(field),
    )
}

/// Deserializes a string enum: `known` maps the strings it knows, and any other string
/// becomes `unknown` with a warning (see [`warn_unknown`]).
pub(crate) fn deserialize_str<'de, D, T>(
    deserializer: D,
    type_name: &'static str,
    known: fn(&str) -> Option<T>,
    unknown: T,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    struct OpenStr<T> {
        type_name: &'static str,
        known: fn(&str) -> Option<T>,
        unknown: T,
    }

    impl<T> Visitor<'_> for OpenStr<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "a {} string", self.type_name)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
            Ok((self.known)(value).unwrap_or_else(|| {
                warn_unknown(self.type_name, value);
                self.unknown
            }))
        }
    }

    deserializer.deserialize_str(OpenStr {
        type_name,
        known,
        unknown,
    })
}

/// Logs a warning for a union member whose `type` tag `type_name` does not know (see
/// [`warn_unknown`]).
pub(crate) fn warn_unknown_member(type_name: &'static str, value: &Value) {
    let tag = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    warn_unknown(type_name, tag);
}

/// Reads the `type` tag of a union member.
pub(crate) fn type_tag<E: de::Error>(value: &Value) -> Result<String, E> {
    value
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| E::missing_field("type"))
}

/// Deserializes a known union member, reporting failures as the deserializer's error.
pub(crate) fn from_value<T, E>(value: Value) -> Result<T, E>
where
    T: de::DeserializeOwned,
    E: de::Error,
{
    serde_json::from_value(value).map_err(E::custom)
}

/// Serializes `value` as a JSON object with `"type": tag` placed first.
///
/// Used for nested items whose struct does not carry its own `type` field.
pub(crate) fn with_type<T, S>(
    tag: &'static str,
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    let inner = serde_json::to_value(value).map_err(ser::Error::custom)?;
    let Value::Object(fields) = inner else {
        return Err(ser::Error::custom("expected a JSON object"));
    };
    let mut tagged = Map::with_capacity(fields.len() + 1);
    tagged.insert("type".to_string(), Value::String(tag.to_string()));
    for (key, field) in fields {
        tagged.insert(key, field);
    }
    Value::Object(tagged).serialize(serializer)
}

/// Like [`with_type`], for a slice of items sharing one tag.
pub(crate) fn with_type_seq<T, S>(
    tag: &'static str,
    values: &[T],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    use serde::ser::SerializeSeq;

    struct Tagged<'value, T>(&'static str, &'value T);
    impl<T: Serialize> Serialize for Tagged<'_, T> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            with_type(self.0, self.1, serializer)
        }
    }

    let mut seq = serializer.serialize_seq(Some(values.len()))?;
    for value in values {
        seq.serialize_element(&Tagged(tag, value))?;
    }
    seq.end()
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    use crate::sse::{SseEvent, parse_message_stream_event};
    use crate::test_support::unknown_warnings;
    use crate::types::{
        AppliedEditType, CacheMissType, CodeExecutionToolResultContent, ContentBlock, CostType,
        DocumentSource, ErrorType, FallbackCreditReason, FallbackCreditStatus, InferenceSpeed,
        MessageBatchProcessingStatus, MessageBatchResult, MessageBatchResultVariant, MessageRole,
        MessageStreamEvent, ModelLifecycle, ModelType, ServiceTier, SkillType, StopDetailsType,
        StopReason, TokenType, ToolSearchToolResultContent, UsageIteration, UsageReportServiceTier,
        WebFetchToolResultContent,
    };

    /// Deserializes `value`, asserting that it logs no warning.
    fn known<T: DeserializeOwned>(value: Value) -> T {
        let (parsed, warnings) = unknown_warnings(|| serde_json::from_value::<T>(value).unwrap());
        assert_eq!(warnings, Vec::<String>::new());
        parsed
    }

    /// Deserializes `value`, returning it with the warnings it logged.
    fn unknown<T: DeserializeOwned>(value: Value) -> (T, Vec<String>) {
        unknown_warnings(|| serde_json::from_value::<T>(value).unwrap())
    }

    fn round_trips<T: Serialize + DeserializeOwned + PartialEq + Debug>(values: &[T]) {
        for value in values {
            let wire = serde_json::to_value(value).unwrap();
            assert_eq!(&known::<T>(wire), value);
        }
    }

    #[test]
    fn known_strings_deserialize_unchanged_and_without_warnings() {
        round_trips(&[
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::StopSequence,
            StopReason::ToolUse,
            StopReason::PauseTurn,
            StopReason::Refusal,
            StopReason::ModelContextWindowExceeded,
            StopReason::Compaction,
        ]);
        round_trips(&[
            ServiceTier::Standard,
            ServiceTier::Priority,
            ServiceTier::Batch,
        ]);
        round_trips(&[InferenceSpeed::Standard, InferenceSpeed::Fast]);
        round_trips(&[
            FallbackCreditReason::BodyMismatch,
            FallbackCreditReason::ContinuationExcluded,
            FallbackCreditReason::ContinuationOnly,
            FallbackCreditReason::Expired,
            FallbackCreditReason::InvalidTargetModel,
            FallbackCreditReason::NotEnabled,
            FallbackCreditReason::RepriceUnavailable,
            FallbackCreditReason::TemporarilyUnavailable,
            FallbackCreditReason::VariantFieldsPresent,
            FallbackCreditReason::WrongOrganization,
            FallbackCreditReason::WrongPlatform,
            FallbackCreditReason::WrongWorkspace,
        ]);
        round_trips(&[
            MessageBatchProcessingStatus::InProgress,
            MessageBatchProcessingStatus::Canceling,
            MessageBatchProcessingStatus::Ended,
        ]);
        round_trips(&[
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::System,
        ]);
        round_trips(&[
            ModelLifecycle::Active,
            ModelLifecycle::Deprecated,
            ModelLifecycle::Retired,
        ]);
        round_trips(&[SkillType::Anthropic, SkillType::Custom]);
        round_trips(&[ModelType::Model]);
        assert_eq!(
            known::<MessageBatchProcessingStatus>(json!("in_progress")),
            MessageBatchProcessingStatus::InProgress
        );
    }

    #[test]
    fn unknown_string_becomes_unknown_with_one_warning() {
        let (reason, warnings) = unknown::<StopReason>(json!("brand_new_reason"));
        assert_eq!(reason, StopReason::Unknown);
        assert_eq!(warnings, ["rust_type=StopReason tag=brand_new_reason"]);

        let (role, warnings) = unknown::<MessageRole>(json!("narrator"));
        assert_eq!(role, MessageRole::Unknown);
        assert_eq!(warnings, ["rust_type=MessageRole tag=narrator"]);

        let (status, _) = unknown::<MessageBatchProcessingStatus>(json!("paused"));
        assert_eq!(status, MessageBatchProcessingStatus::Unknown);
        let (lifecycle, _) = unknown::<ModelLifecycle>(json!("sunset"));
        assert_eq!(lifecycle, ModelLifecycle::Unknown);
        let (skill, warnings) = unknown::<SkillType>(json!("shared"));
        assert_eq!(skill, SkillType::Unknown);
        assert_eq!(warnings, ["rust_type=SkillType tag=shared"]);
        let (object, _) = unknown::<ModelType>(json!("model_alias"));
        assert_eq!(object, ModelType::Unknown);

        assert!(serde_json::from_value::<StopReason>(json!(7)).is_err());
    }

    #[test]
    fn wire_value_enums_round_trip_their_documented_values() {
        round_trips(&[
            ErrorType::InvalidRequestError,
            ErrorType::AuthenticationError,
            ErrorType::BillingError,
            ErrorType::PermissionError,
            ErrorType::NotFoundError,
            ErrorType::ConflictError,
            ErrorType::RequestTooLarge,
            ErrorType::RateLimitError,
            ErrorType::ApiError,
            ErrorType::TimeoutError,
            ErrorType::OverloadedError,
        ]);
        round_trips(&[StopDetailsType::Refusal]);
        round_trips(&[
            CacheMissType::ModelChanged,
            CacheMissType::SystemChanged,
            CacheMissType::ToolsChanged,
            CacheMissType::MessagesChanged,
            CacheMissType::PreviousMessageNotFound,
            CacheMissType::Unavailable,
        ]);
        round_trips(&[
            AppliedEditType::ClearToolUses20250919,
            AppliedEditType::ClearThinking20251015,
        ]);
        round_trips(&[
            CostType::CodeExecution,
            CostType::SessionUsage,
            CostType::Tokens,
            CostType::WebSearch,
        ]);
        round_trips(&[
            TokenType::UncachedInputTokens,
            TokenType::OutputTokens,
            TokenType::CacheReadInputTokens,
            TokenType::CacheCreationEphemeral5mInputTokens,
            TokenType::CacheCreationEphemeral1hInputTokens,
        ]);
        round_trips(&[
            UsageReportServiceTier::Standard,
            UsageReportServiceTier::Priority,
            UsageReportServiceTier::PriorityOnDemand,
            UsageReportServiceTier::Batch,
            UsageReportServiceTier::Flex,
            UsageReportServiceTier::FlexDiscount,
        ]);
        assert_eq!(
            serde_json::to_value(TokenType::CacheCreationEphemeral5mInputTokens).unwrap(),
            json!("cache_creation.ephemeral_5m_input_tokens")
        );
        assert_eq!(
            serde_json::to_value(AppliedEditType::ClearThinking20251015).unwrap(),
            json!("clear_thinking_20251015")
        );
        assert_eq!(
            serde_json::to_value(UsageReportServiceTier::PriorityOnDemand).unwrap(),
            json!("priority_on_demand")
        );
    }

    #[test]
    fn wire_value_enums_read_an_unknown_value_with_one_warning() {
        let (value, warnings) = unknown::<ErrorType>(json!("teapot_error"));
        assert_eq!(value, ErrorType::Unknown);
        assert_eq!(warnings, ["rust_type=ErrorType tag=teapot_error"]);
        let (value, warnings) = unknown::<StopDetailsType>(json!("pause"));
        assert_eq!(value, StopDetailsType::Unknown);
        assert_eq!(warnings, ["rust_type=StopDetailsType tag=pause"]);
        let (value, warnings) = unknown::<CacheMissType>(json!("betas_changed"));
        assert_eq!(value, CacheMissType::Unknown);
        assert_eq!(warnings, ["rust_type=CacheMissType tag=betas_changed"]);
        let (value, warnings) = unknown::<AppliedEditType>(json!("compact_20260112"));
        assert_eq!(value, AppliedEditType::Unknown);
        assert_eq!(warnings, ["rust_type=AppliedEditType tag=compact_20260112"]);
        let (value, warnings) = unknown::<CostType>(json!("storage"));
        assert_eq!(value, CostType::Unknown);
        assert_eq!(warnings, ["rust_type=CostType tag=storage"]);
        let (value, warnings) = unknown::<TokenType>(json!("thinking_tokens"));
        assert_eq!(value, TokenType::Unknown);
        assert_eq!(warnings, ["rust_type=TokenType tag=thinking_tokens"]);
        let (value, warnings) = unknown::<UsageReportServiceTier>(json!("spot"));
        assert_eq!(value, UsageReportServiceTier::Unknown);
        assert_eq!(warnings, ["rust_type=UsageReportServiceTier tag=spot"]);
        let (value, warnings) = unknown::<ServiceTier>(json!("overflow"));
        assert_eq!(value, ServiceTier::Unknown);
        assert_eq!(warnings, ["rust_type=ServiceTier tag=overflow"]);
    }

    #[test]
    fn long_unknown_string_is_truncated_before_logging_and_dedupe() {
        let short = "é".repeat(super::MAX_UNKNOWN_TAG_CHARS);
        let (reason, warnings) =
            unknown::<StopReason>(json!(format!("{short}{}", "a".repeat(10_000))));
        assert_eq!(reason, StopReason::Unknown);
        assert_eq!(
            warnings,
            [format!(
                "rust_type=StopReason tag={short}{}",
                super::TRUNCATED_MARKER
            )]
        );
        // Another long tag with the same first chars is the same value once truncated.
        let (_, warnings) = unknown::<StopReason>(json!(format!("{short}{}", "b".repeat(10_000))));
        assert_eq!(warnings, Vec::<String>::new());
        // A tag exactly at the limit is kept whole.
        let (_, warnings) = unknown::<MessageRole>(json!(short.clone()));
        assert_eq!(warnings, [format!("rust_type=MessageRole tag={short}")]);
    }

    #[test]
    fn known_tagged_members_deserialize_unchanged_and_without_warnings() {
        let content: CodeExecutionToolResultContent = known(json!({
            "type": "code_execution_result",
            "stdout": "4\n",
            "stderr": "",
            "return_code": 0,
            "content": [{"type": "code_execution_output", "file_id": "file_1"}]
        }));
        assert!(matches!(
            &content,
            CodeExecutionToolResultContent::Result { stdout, return_code: 0, content, .. }
                if stdout == "4\n" && content[0].file_id == "file_1"
        ));
        let encrypted = json!({"type": "encrypted_code_execution_result", "blob": "x"});
        let content: CodeExecutionToolResultContent = known(encrypted.clone());
        assert_eq!(serde_json::to_value(&content).unwrap(), encrypted);

        let error = json!({"type": "web_fetch_tool_result_error", "error_code": "url_not_allowed"});
        let content: WebFetchToolResultContent = known(error.clone());
        assert_eq!(serde_json::to_value(&content).unwrap(), error);

        let result = json!({
            "type": "tool_search_tool_search_result",
            "tool_references": [{"type": "tool_reference", "tool_name": "get_weather"}]
        });
        let content: ToolSearchToolResultContent = known(result.clone());
        assert_eq!(serde_json::to_value(&content).unwrap(), result);

        let source = json!({"type": "url", "url": "https://example.com/a.pdf"});
        let document: DocumentSource = known(source.clone());
        assert!(matches!(document, DocumentSource::UrlPdf(_)));

        let status: FallbackCreditStatus = known(json!({
            "type": "not_applied",
            "reason": "expired"
        }));
        assert_eq!(
            status,
            FallbackCreditStatus::NotApplied {
                reason: FallbackCreditReason::Expired,
                remove_to_redeem: None
            }
        );
        let iteration: UsageIteration =
            known(json!({"type": "advisor_message", "input_tokens": 1, "output_tokens": 2}));
        assert!(matches!(iteration, UsageIteration::AdvisorMessage(_)));
        let event: MessageStreamEvent = known(json!({"type": "ping"}));
        assert_eq!(event, MessageStreamEvent::Ping);

        // A known tag with a malformed body is still an error, not an unknown member.
        assert!(
            serde_json::from_value::<CodeExecutionToolResultContent>(
                json!({"type": "code_execution_tool_result_error"})
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_tagged_member_is_kept_and_logged_without_its_payload() {
        let block = json!({"type": "hologram", "secret": "user text"});
        let (parsed, warnings) = unknown::<ContentBlock>(block.clone());
        assert_eq!(parsed, ContentBlock::Unknown(block.clone()));
        assert_eq!(warnings, ["rust_type=ContentBlock tag=hologram"]);

        let (parsed, warnings) = unknown::<WebFetchToolResultContent>(block.clone());
        assert_eq!(parsed, WebFetchToolResultContent::Unknown(block.clone()));
        assert_eq!(
            warnings,
            ["rust_type=WebFetchToolResultContent tag=hologram"]
        );
        assert_eq!(serde_json::to_value(&parsed).unwrap(), block);

        let (parsed, _) = unknown::<CodeExecutionToolResultContent>(block.clone());
        assert_eq!(
            parsed,
            CodeExecutionToolResultContent::Unknown(block.clone())
        );
        let (parsed, _) = unknown::<ToolSearchToolResultContent>(block.clone());
        assert_eq!(parsed, ToolSearchToolResultContent::Unknown(block.clone()));
        let (parsed, _) = unknown::<DocumentSource>(block.clone());
        assert_eq!(parsed, DocumentSource::Unknown(block.clone()));
        let (parsed, warnings) = unknown::<UsageIteration>(block);
        assert_eq!(parsed, UsageIteration::Unknown);
        assert!(
            warnings
                .iter()
                .all(|warning| !warning.contains("user text"))
        );
    }

    #[test]
    fn batch_result_of_unknown_type_is_kept_and_logged_without_its_payload() {
        let line = json!({
            "custom_id": "request-1",
            "result": {"type": "deferred", "secret": "user text"}
        });
        let (parsed, warnings) = unknown::<MessageBatchResult>(line.clone());
        assert_eq!(
            parsed.result,
            MessageBatchResultVariant::Unknown(line["result"].clone())
        );
        assert_eq!(
            warnings,
            ["rust_type=MessageBatchResultVariant tag=deferred"]
        );
        assert_eq!(serde_json::to_value(&parsed).unwrap(), line);

        let canceled: MessageBatchResultVariant = known(json!({"type": "canceled"}));
        assert_eq!(canceled, MessageBatchResultVariant::Canceled);
        // A known tag with a malformed body is still an error, not an unknown result.
        assert!(
            serde_json::from_value::<MessageBatchResultVariant>(json!({"type": "succeeded"}))
                .is_err()
        );
    }

    #[test]
    fn unknown_stream_event_is_logged_once() {
        let event = SseEvent::new("hologram_delta", r#"{"type":"hologram_delta","x":1}"#);
        let (parsed, warnings) = unknown_warnings(|| parse_message_stream_event(&event).unwrap());
        assert_eq!(parsed, MessageStreamEvent::Unknown);
        assert_eq!(
            warnings,
            ["rust_type=MessageStreamEvent tag=hologram_delta"]
        );

        let (parsed, warnings) = unknown::<MessageStreamEvent>(json!({"type": "hologram_burst"}));
        assert_eq!(parsed, MessageStreamEvent::Unknown);
        assert_eq!(
            warnings,
            ["rust_type=MessageStreamEvent tag=hologram_burst"]
        );
    }

    #[test]
    fn repeated_unknown_value_is_logged_once_per_process() {
        let ((), warnings) = unknown_warnings(|| {
            for _ in 0..3 {
                let reason: StopReason = serde_json::from_value(json!("repeat_reason")).unwrap();
                assert_eq!(reason, StopReason::Unknown);
            }
            // The same tag on another type is a distinct value.
            let role: MessageRole = serde_json::from_value(json!("repeat_reason")).unwrap();
            assert_eq!(role, MessageRole::Unknown);
        });
        assert_eq!(
            warnings,
            [
                "rust_type=StopReason tag=repeat_reason",
                "rust_type=MessageRole tag=repeat_reason"
            ]
        );
    }
}
