use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

/// Parameters for the Text Editor tool, versions 20250124, 20250429 and 20250728.
///
/// This tool allows the AI to perform text editing operations via the API. The version is
/// chosen by the [`ToolUnionParam`](crate::ToolUnionParam) variant; the matching constructor
/// sets the expected name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct TextEditorTool {
    /// Name of the tool. This is how the tool will be called by the model and in `tool_use` blocks.
    ///
    /// `str_replace_editor` for version 20250124, `str_replace_based_edit_tool` for 20250429
    /// and 20250728.
    pub name: String,

    /// Create a cache control breakpoint at this content block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,

    /// Maximum characters to display when viewing a file; version 20250728 only. If not
    /// specified, the full file is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_characters: Option<u32>,
}

impl TextEditorTool {
    /// Creates the version 20250124 tool (`str_replace_editor`).
    #[must_use]
    pub fn new_20250124() -> Self {
        Self {
            name: "str_replace_editor".to_string(),
            cache_control: None,
            max_characters: None,
        }
    }

    /// Creates the version 20250429 tool (`str_replace_based_edit_tool`).
    #[must_use]
    pub fn new_20250429() -> Self {
        Self {
            name: "str_replace_based_edit_tool".to_string(),
            cache_control: None,
            max_characters: None,
        }
    }

    /// Creates the version 20250728 tool (`str_replace_based_edit_tool`), which accepts
    /// [`TextEditorTool::with_max_characters`].
    #[must_use]
    pub fn new_20250728() -> Self {
        Self::new_20250429()
    }

    /// Sets the cache control to ephemeral for this tool.
    #[must_use]
    pub fn with_ephemeral_cache_control(mut self) -> Self {
        self.cache_control = Some(CacheControlEphemeral::new());
        self
    }

    /// Sets the maximum characters to display when viewing a file. Only version 20250728
    /// accepts it.
    #[must_use]
    pub fn with_max_characters(mut self, max_characters: u32) -> Self {
        self.max_characters = Some(max_characters);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn tool_text_editor_param_minimal() {
        assert_eq!(
            to_value(TextEditorTool::new_20250124()).unwrap(),
            json!({
                "name": "str_replace_editor"
            })
        );
        assert_eq!(
            to_value(TextEditorTool::new_20250429()).unwrap(),
            json!({
                "name": "str_replace_based_edit_tool"
            })
        );
    }

    #[test]
    fn tool_text_editor_param_with_cache_control() {
        let param = TextEditorTool::new_20250124().with_ephemeral_cache_control();

        let json = to_value(&param).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "str_replace_editor",
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }

    #[test]
    fn tool_text_editor_param_deserialization() {
        let json = json!({
            "name": "str_replace_editor",
            "cache_control": {
                "type": "ephemeral"
            }
        });

        let param: TextEditorTool = serde_json::from_value(json).unwrap();
        assert_eq!(param.name, "str_replace_editor");
        assert!(param.cache_control.is_some());
    }

    #[test]
    fn tool_text_editor_20250728_param_minimal() {
        let param = TextEditorTool::new_20250728();
        let json = to_value(&param).unwrap();

        assert_eq!(
            json,
            json!({
                "name": "str_replace_based_edit_tool"
            })
        );
    }

    #[test]
    fn tool_text_editor_20250728_param_with_cache_control() {
        let param = TextEditorTool::new_20250728().with_ephemeral_cache_control();

        let json = to_value(&param).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "str_replace_based_edit_tool",
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }

    #[test]
    fn tool_text_editor_20250728_param_with_max_characters() {
        let param = TextEditorTool::new_20250728()
            .with_max_characters(1000)
            .with_ephemeral_cache_control();

        let json = to_value(&param).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "str_replace_based_edit_tool",
                "cache_control": {
                    "type": "ephemeral"
                },
                "max_characters": 1000
            })
        );
    }

    #[test]
    fn tool_text_editor_20250728_param_deserialization() {
        let json = json!({
            "name": "str_replace_based_edit_tool",
            "cache_control": {
                "type": "ephemeral"
            },
            "max_characters": 2000
        });

        let param: TextEditorTool = serde_json::from_value(json).unwrap();
        assert_eq!(param.name, "str_replace_based_edit_tool");
        assert!(param.cache_control.is_some());
        assert_eq!(param.max_characters, Some(2000));
    }
}
