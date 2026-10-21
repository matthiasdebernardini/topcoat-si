//! Zero-sized types for the constant `type` tags of API objects.
//!
//! Each tag serializes as its one string. Reading any other string still succeeds and logs a
//! warning through [`open_union::warn_unknown`], so an unexpected tag never breaks a reply. Do
//! not use a tag where its value decides which variant or type parses.

use crate::types::open_union;

/// Defines a zero-sized tag type that always serializes as `$value`.
macro_rules! wire_tag {
    ($(#[$doc:meta])* $name:ident = $value:literal) => {
        $(#[$doc])*
        #[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
        pub(crate) struct $name;

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str($value)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                open_union::deserialize_str(
                    deserializer,
                    stringify!($name),
                    |value| (value == $value).then_some(Self),
                    Self,
                )
            }
        }
    };
}

wire_tag!(
    /// `"ephemeral"`, the type of a prompt-cache breakpoint.
    EphemeralTag = "ephemeral"
);
wire_tag!(
    /// `"message"`, the type of a generated message.
    MessageTag = "message"
);
wire_tag!(
    /// `"message_batch"`, the type of a Message Batch.
    MessageBatchTag = "message_batch"
);
wire_tag!(
    /// `"message_batch_deleted"`, the type of a deleted Message Batch.
    MessageBatchDeletedTag = "message_batch_deleted"
);
wire_tag!(
    /// `"error"`, the type of an error response.
    ErrorTag = "error"
);
wire_tag!(
    /// `"file"`, the type of a stored file.
    FileTag = "file"
);
wire_tag!(
    /// `"file_deleted"`, the type of a deleted file.
    FileDeletedTag = "file_deleted"
);
wire_tag!(
    /// `"text"`, the type of a system prompt text block.
    TextTag = "text"
);
wire_tag!(
    /// `"tool_uses"`, the type of a tool-use count to keep.
    ToolUsesTag = "tool_uses"
);
wire_tag!(
    /// `"thinking_turns"`, the type of a thinking-turn count to keep.
    ThinkingTurnsTag = "thinking_turns"
);
wire_tag!(
    /// `"pdf"`, the type of Firecrawl's PDF parser.
    PdfTag = "pdf"
);

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    use super::*;
    use crate::test_support::unknown_warnings;

    #[test]
    fn every_tag_round_trips_its_value() {
        fn round_trips<T>(tag: T, value: &str)
        where
            T: Serialize + DeserializeOwned + PartialEq + Debug,
        {
            assert_eq!(serde_json::to_value(&tag).unwrap(), json!(value));
            let (parsed, warnings) =
                unknown_warnings(|| serde_json::from_value::<T>(json!(value)).unwrap());
            assert_eq!(parsed, tag);
            assert_eq!(warnings, Vec::<String>::new());
        }
        round_trips(EphemeralTag, "ephemeral");
        round_trips(MessageTag, "message");
        round_trips(MessageBatchTag, "message_batch");
        round_trips(MessageBatchDeletedTag, "message_batch_deleted");
        round_trips(ErrorTag, "error");
        round_trips(FileTag, "file");
        round_trips(FileDeletedTag, "file_deleted");
        round_trips(TextTag, "text");
        round_trips(ToolUsesTag, "tool_uses");
        round_trips(ThinkingTurnsTag, "thinking_turns");
        round_trips(PdfTag, "pdf");
    }

    #[test]
    fn other_value_parses_and_logs_one_warning() {
        let (tag, warnings) =
            unknown_warnings(|| serde_json::from_value::<MessageTag>(json!("message_v2")).unwrap());
        assert_eq!(tag, MessageTag);
        assert_eq!(warnings, ["rust_type=MessageTag tag=message_v2"]);
        // It still serializes as the constant.
        assert_eq!(serde_json::to_value(tag).unwrap(), json!("message"));
        assert!(serde_json::from_value::<MessageTag>(json!(1)).is_err());
    }
}
