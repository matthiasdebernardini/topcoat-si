use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

/// Server-side context management for a request.
///
/// Requires the `context-management-2025-06-27` beta (plus `compact-2026-01-12`
/// when a [`ContextEdit::Compact20260112`] edit is present); both are added
/// automatically. Mutually exclusive with `compaction`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextManagement {
    /// The edits to apply, in order.
    pub edits: Vec<ContextEdit>,
}

impl ContextManagement {
    /// Create a context-management configuration from edits.
    pub fn new(edits: Vec<ContextEdit>) -> Self {
        Self { edits }
    }

    /// Whether any edit is a [`ContextEdit::Compact20260112`] edit.
    pub fn has_compact_edit(&self) -> bool {
        self.edits
            .iter()
            .any(|edit| matches!(edit, ContextEdit::Compact20260112 { .. }))
    }
}

/// A threshold that starts or bounds a context edit.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextTrigger {
    /// Trigger when the prompt reaches this many input tokens.
    InputTokens {
        /// Token count.
        value: u64,
    },
    /// Trigger after this many tool uses.
    ToolUses {
        /// Tool-use count.
        value: u64,
    },
}

/// How many recent tool uses to keep when clearing tool uses.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct KeepToolUses {
    /// Number of most recent tool uses to keep.
    pub value: u64,
}

impl Serialize for KeepToolUses {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("type", "tool_uses")?;
        map.serialize_entry("value", &self.value)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for KeepToolUses {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(rename = "type")]
            kind: String,
            value: u64,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.kind != "tool_uses" {
            return Err(de::Error::unknown_variant(&raw.kind, &["tool_uses"]));
        }
        Ok(Self { value: raw.value })
    }
}

/// How much thinking to keep when clearing thinking blocks.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ThinkingKeep {
    /// Keep thinking from this many recent assistant turns.
    Turns(u64),
    /// Keep all thinking blocks.
    All,
}

impl Serialize for ThinkingKeep {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ThinkingKeep::Turns(value) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "thinking_turns")?;
                map.serialize_entry("value", value)?;
                map.end()
            }
            ThinkingKeep::All => serializer.serialize_str("all"),
        }
    }
}

impl<'de> Deserialize<'de> for ThinkingKeep {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Turns {
                #[serde(rename = "type")]
                kind: String,
                value: u64,
            },
        }
        match Raw::deserialize(deserializer)? {
            Raw::Text(text) if text == "all" => Ok(ThinkingKeep::All),
            Raw::Text(text) => Err(de::Error::unknown_variant(&text, &["all"])),
            Raw::Turns { kind, value } if kind == "thinking_turns" => {
                Ok(ThinkingKeep::Turns(value))
            }
            Raw::Turns { kind, .. } => Err(de::Error::unknown_variant(&kind, &["thinking_turns"])),
        }
    }
}

/// One context-management edit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ContextEdit {
    /// Clear old tool results (and optionally inputs) once the prompt grows.
    #[serde(rename = "clear_tool_uses_20250919")]
    ClearToolUses20250919 {
        /// When to start clearing. Default: 100,000 input tokens.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger: Option<ContextTrigger>,
        /// Recent tool uses to keep. Default: 3.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keep: Option<KeepToolUses>,
        /// Minimum amount that must be cleared for the edit to apply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clear_at_least: Option<ContextTrigger>,
        /// Tools whose uses are never cleared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclude_tools: Option<Vec<String>>,
        /// Whether to clear tool inputs as well as results.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clear_tool_inputs: Option<bool>,
    },
    /// Clear old thinking blocks.
    #[serde(rename = "clear_thinking_20251015")]
    ClearThinking20251015 {
        /// How much thinking to keep.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keep: Option<ThinkingKeep>,
    },
    /// Summarize the conversation server-side when it grows.
    #[serde(rename = "compact_20260112")]
    Compact20260112 {
        /// When to compact. Default: 150,000 input tokens (minimum 50,000).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger: Option<ContextTrigger>,
        /// Stop after compaction so the caller can inspect the summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pause_after_compaction: Option<bool>,
        /// Custom summarization instructions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
    },
}

impl ContextEdit {
    /// A `clear_tool_uses_20250919` edit with API defaults.
    pub fn clear_tool_uses() -> Self {
        Self::ClearToolUses20250919 {
            trigger: None,
            keep: None,
            clear_at_least: None,
            exclude_tools: None,
            clear_tool_inputs: None,
        }
    }

    /// A `clear_thinking_20251015` edit with API defaults.
    pub fn clear_thinking() -> Self {
        Self::ClearThinking20251015 { keep: None }
    }

    /// A `compact_20260112` edit with API defaults.
    pub fn compact() -> Self {
        Self::Compact20260112 {
            trigger: None,
            pause_after_compaction: None,
            instructions: None,
        }
    }
}

/// Context management applied to a response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextManagementResponse {
    /// The edits that were applied.
    #[serde(default)]
    pub applied_edits: Vec<AppliedEdit>,
}

/// An edit the API applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppliedEdit {
    /// The edit type, such as `clear_tool_uses_20250919`.
    pub r#type: String,

    /// Input tokens removed by the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_input_tokens: Option<u64>,

    /// Tool uses removed by the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_tool_uses: Option<u64>,

    /// Thinking turns removed by the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_thinking_turns: Option<u64>,
}

/// Context-management information in a count-tokens response.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CountTokensContextManagement {
    /// Input tokens before any context-management edits were applied.
    pub original_input_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn clear_tool_uses_example() {
        let expected = json!({"edits":[{"type":"clear_tool_uses_20250919","trigger":{"type":"input_tokens","value":100000},"keep":{"type":"tool_uses","value":3},"clear_at_least":{"type":"input_tokens","value":5000},"exclude_tools":["web_search"],"clear_tool_inputs":true}]});
        let management = ContextManagement::new(vec![ContextEdit::ClearToolUses20250919 {
            trigger: Some(ContextTrigger::InputTokens { value: 100_000 }),
            keep: Some(KeepToolUses { value: 3 }),
            clear_at_least: Some(ContextTrigger::InputTokens { value: 5000 }),
            exclude_tools: Some(vec!["web_search".to_string()]),
            clear_tool_inputs: Some(true),
        }]);
        assert_eq!(to_value(&management).unwrap(), expected);
        let back: ContextManagement = serde_json::from_value(expected).unwrap();
        assert_eq!(back, management);
    }

    #[test]
    fn clear_thinking_keep_all() {
        let edit = ContextEdit::ClearThinking20251015 {
            keep: Some(ThinkingKeep::All),
        };
        let expected = json!({"type": "clear_thinking_20251015", "keep": "all"});
        assert_eq!(to_value(&edit).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<ContextEdit>(expected).unwrap(),
            edit
        );
    }

    #[test]
    fn clear_thinking_keep_turns() {
        let edit = ContextEdit::ClearThinking20251015 {
            keep: Some(ThinkingKeep::Turns(2)),
        };
        let expected = json!({"type": "clear_thinking_20251015", "keep": {"type": "thinking_turns", "value": 2}});
        assert_eq!(to_value(&edit).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<ContextEdit>(expected).unwrap(),
            edit
        );
    }

    #[test]
    fn defaults_omit_everything() {
        assert_eq!(
            to_value(ContextEdit::clear_tool_uses()).unwrap(),
            json!({"type": "clear_tool_uses_20250919"})
        );
        assert_eq!(
            to_value(ContextEdit::clear_thinking()).unwrap(),
            json!({"type": "clear_thinking_20251015"})
        );
        assert_eq!(
            to_value(ContextEdit::compact()).unwrap(),
            json!({"type": "compact_20260112"})
        );
    }

    #[test]
    fn compact_edit_serialization() {
        let edit = ContextEdit::Compact20260112 {
            trigger: Some(ContextTrigger::InputTokens { value: 150_000 }),
            pause_after_compaction: Some(true),
            instructions: Some("Keep file paths".to_string()),
        };
        assert_eq!(
            to_value(&edit).unwrap(),
            json!({
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 150000},
                "pause_after_compaction": true,
                "instructions": "Keep file paths"
            })
        );
    }

    #[test]
    fn tool_uses_trigger() {
        assert_eq!(
            to_value(ContextTrigger::ToolUses { value: 7 }).unwrap(),
            json!({"type": "tool_uses", "value": 7})
        );
    }

    #[test]
    fn has_compact_edit() {
        assert!(!ContextManagement::new(vec![ContextEdit::clear_tool_uses()]).has_compact_edit());
        assert!(
            ContextManagement::new(vec![ContextEdit::clear_tool_uses(), ContextEdit::compact()])
                .has_compact_edit()
        );
    }

    #[test]
    fn invalid_thinking_keep_rejected() {
        assert!(serde_json::from_value::<ThinkingKeep>(json!("some")).is_err());
    }

    #[test]
    fn applied_edits_deserialization() {
        let response: ContextManagementResponse = serde_json::from_value(json!({
            "applied_edits": [
                {"type": "clear_tool_uses_20250919", "cleared_input_tokens": 12000, "cleared_tool_uses": 4}
            ]
        }))
        .unwrap();
        assert_eq!(response.applied_edits.len(), 1);
        let edit = &response.applied_edits[0];
        assert_eq!(edit.r#type, "clear_tool_uses_20250919");
        assert_eq!(edit.cleared_input_tokens, Some(12000));
        assert_eq!(edit.cleared_tool_uses, Some(4));
        assert_eq!(edit.cleared_thinking_turns, None);
    }
}
