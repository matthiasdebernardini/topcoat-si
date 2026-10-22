use serde::{Deserialize, Serialize};

/// Configuration for Claude's tool choice behavior.
///
/// This can be one of the following:
/// - "auto": Let the model decide if and when to use tools
/// - "none": Do not use any tools
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ToolChoice {
    /// Automatic tool choice
    Auto {
        /// Whether to disable parallel tool use.
        ///
        /// Defaults to `false`. If set to `true`, the model will output at most one tool use.
        #[serde(skip_serializing_if = "Option::is_none")]
        disable_parallel_tool_use: Option<bool>,
    },

    /// No tools
    None,
}

impl ToolChoice {
    /// Create a new `ToolChoice` with auto mode.
    #[must_use]
    pub fn auto() -> Self {
        Self::Auto {
            disable_parallel_tool_use: None,
        }
    }

    /// Create a new `ToolChoice` with auto mode, specifying whether to disable parallel tool use.
    #[must_use]
    pub fn auto_with_disable_parallel(disable: bool) -> Self {
        Self::Auto {
            disable_parallel_tool_use: Some(disable),
        }
    }

    /// Create a new `ToolChoice` with no tools.
    #[must_use]
    pub fn none() -> Self {
        Self::None
    }
}

impl Default for ToolChoice {
    fn default() -> Self {
        Self::auto()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn tool_choice_auto() {
        let param = ToolChoice::auto();
        let json = to_value(&param).unwrap();

        assert_eq!(
            json,
            json!({
                "type": "auto"
            })
        );
    }

    #[test]
    fn tool_choice_none() {
        let param = ToolChoice::none();
        let json = to_value(&param).unwrap();

        assert_eq!(
            json,
            json!({
                "type": "none"
            })
        );
    }

    #[test]
    fn tool_choice_auto_with_disable_parallel() {
        let param = ToolChoice::auto_with_disable_parallel(true);
        let json = to_value(&param).unwrap();

        assert_eq!(
            json,
            json!({
                "type": "auto",
                "disable_parallel_tool_use": true
            })
        );
    }

    #[test]
    fn tool_choice_deserialization_auto() {
        let json = json!({
            "type": "auto",
            "disable_parallel_tool_use": true
        });

        let param: ToolChoice = serde_json::from_value(json).unwrap();
        match param {
            ToolChoice::Auto {
                disable_parallel_tool_use,
            } => {
                assert_eq!(disable_parallel_tool_use, Some(true));
            }
            _ => panic!("Expected Auto variant"),
        }
    }
}
