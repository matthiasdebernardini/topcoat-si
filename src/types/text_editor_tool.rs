use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

/// Parameters for the Text Editor tool, versions 20250124 and 20250429.
///
/// This tool allows the AI to perform text editing operations via the API. The version is
/// chosen by the [`ToolUnionParam`](crate::ToolUnionParam) variant; the matching constructor
/// sets the expected name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct TextEditorTool {
    /// Name of the tool. This is how the tool will be called by the model and in `tool_use` blocks.
    ///
    /// `str_replace_editor` for version 20250124, `str_replace_based_edit_tool` for 20250429.
    pub name: String,

    /// Create a cache control breakpoint at this content block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}

impl TextEditorTool {
    /// Creates the version 20250124 tool (`str_replace_editor`).
    #[must_use]
    pub fn new_20250124() -> Self {
        Self {
            name: "str_replace_editor".to_string(),
            cache_control: None,
        }
    }

    /// Creates the version 20250429 tool (`str_replace_based_edit_tool`).
    #[must_use]
    pub fn new_20250429() -> Self {
        Self {
            name: "str_replace_based_edit_tool".to_string(),
            cache_control: None,
        }
    }

    /// Sets the cache control to ephemeral for this tool.
    #[must_use]
    pub fn with_ephemeral_cache_control(mut self) -> Self {
        self.cache_control = Some(CacheControlEphemeral::new());
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
}
