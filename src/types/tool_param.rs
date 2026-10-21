use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

/// Common parameters for a custom tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolParam {
    /// [JSON schema](https://json-schema.org/draft/2020-12) for this tool's input.
    ///
    /// This defines the shape of the `input` that your tool accepts and that the model
    /// will produce.
    pub input_schema: serde_json::Value,

    /// Name of the tool.
    ///
    /// This is how the tool will be called by the model and in `tool_use` blocks.
    pub name: String,

    /// Create a cache control breakpoint at this content block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,

    /// Description of what this tool does.
    ///
    /// Tool descriptions should be as detailed as possible. The more information that
    /// the model has about what the tool is and how to use it, the better it will
    /// perform. You can use natural language descriptions to reinforce important
    /// aspects of the tool input JSON schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Enable strict mode for tool parameter validation.
    ///
    /// When enabled, Claude will guarantee that tool parameters exactly match the
    /// `input_schema`. This ensures type-safe function calls with correctly-typed
    /// arguments.
    ///
    /// This feature needs no beta header.
    ///
    /// See
    /// [structured outputs](https://docs.anthropic.com/en/docs/build-with-claude/structured-outputs)
    /// for details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,

    /// Whether the tool definition is loaded lazily via tool search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,

    /// Callers allowed to invoke this tool.
    ///
    /// Values: `"direct"`, `"code_execution_20250825"`, `"code_execution_20260120"`,
    /// `"code_execution_20260521"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_callers: Option<Vec<String>>,

    /// Example inputs that show the model how to call the tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_examples: Option<Vec<serde_json::Value>>,

    /// Stream tool input parameters eagerly, without buffering for JSON validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eager_input_streaming: Option<bool>,
}

impl ToolParam {
    /// Create a new `ToolParam` with the required fields.
    pub fn new(name: String, input_schema: serde_json::Value) -> Self {
        Self {
            name,
            input_schema,
            cache_control: None,
            description: None,
            strict: None,
            defer_loading: None,
            allowed_callers: None,
            input_examples: None,
            eager_input_streaming: None,
        }
    }

    /// Add a description to the tool.
    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    /// Add cache control to the tool.
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }

    /// Enable strict mode for tool parameter validation.
    ///
    /// When enabled, Claude will guarantee that tool parameters exactly match the
    /// `input_schema`. This ensures type-safe function calls with correctly-typed
    /// arguments.
    ///
    /// This feature needs no beta header.
    ///
    /// # Example
    ///
    /// ```
    /// use serde_json::json;
    /// use topcoat_si::ToolParam;
    ///
    /// let tool = ToolParam::new(
    ///     "get_weather".to_string(),
    ///     json!({
    ///         "type": "object",
    ///         "properties": {
    ///             "location": { "type": "string" },
    ///             "unit": { "type": "string", "enum": ["celsius", "fahrenheit"] }
    ///         },
    ///         "required": ["location"],
    ///         "additionalProperties": false
    ///     })
    /// )
    /// .with_strict(true);
    /// ```
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }

    /// Set whether the tool definition is deferred until found by tool search.
    pub fn with_defer_loading(mut self, defer_loading: bool) -> Self {
        self.defer_loading = Some(defer_loading);
        self
    }

    /// Set the callers allowed to invoke this tool.
    ///
    /// Values: `"direct"`, `"code_execution_20250825"`, `"code_execution_20260120"`,
    /// `"code_execution_20260521"`.
    pub fn with_allowed_callers(mut self, allowed_callers: Vec<String>) -> Self {
        self.allowed_callers = Some(allowed_callers);
        self
    }

    /// Set example inputs for the tool.
    pub fn with_input_examples(mut self, input_examples: Vec<serde_json::Value>) -> Self {
        self.input_examples = Some(input_examples);
        self
    }

    /// Set whether tool input is streamed eagerly.
    pub fn with_eager_input_streaming(mut self, eager_input_streaming: bool) -> Self {
        self.eager_input_streaming = Some(eager_input_streaming);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn tool_param_complete() {
        let input_schema = json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                }
            }
        });

        let cache_control = CacheControlEphemeral::new();

        let tool = ToolParam::new("search".to_string(), input_schema)
            .with_description("Search for information".to_string())
            .with_cache_control(cache_control);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        }
                    }
                },
                "name": "search",
                "cache_control": {
                    "type": "ephemeral"
                },
                "description": "Search for information"
            })
        );
    }

    #[test]
    fn tool_param_with_strict_mode() {
        let input_schema = json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "The city and state"
                },
                "unit": {
                    "type": "string",
                    "enum": ["celsius", "fahrenheit"]
                }
            },
            "required": ["location"],
            "additionalProperties": false
        });

        let tool = ToolParam::new("get_weather".to_string(), input_schema)
            .with_description("Get the current weather".to_string())
            .with_strict(true);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "The city and state"
                        },
                        "unit": {
                            "type": "string",
                            "enum": ["celsius", "fahrenheit"]
                        }
                    },
                    "required": ["location"],
                    "additionalProperties": false
                },
                "name": "get_weather",
                "description": "Get the current weather",
                "strict": true
            })
        );

        // Verify strict is Some(true)
        assert_eq!(tool.strict, Some(true));
    }

    #[test]
    fn tool_param_strict_mode_false_not_serialized() {
        // When strict is explicitly set to false, it should be serialized
        let input_schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            }
        });

        let tool = ToolParam::new("search".to_string(), input_schema).with_strict(false);

        let json = to_value(&tool).unwrap();
        // strict: false should be serialized
        assert_eq!(json["strict"], json!(false));
    }

    #[test]
    fn tool_param_strict_mode_none_not_serialized() {
        // When strict is None (default), it should not be serialized
        let input_schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            }
        });

        let tool = ToolParam::new("search".to_string(), input_schema);

        let json = to_value(&tool).unwrap();
        // strict should not be present when None
        assert!(
            json.get("strict").is_none(),
            "strict should not be serialized when None"
        );
    }

    #[test]
    fn tool_param_new_fields() {
        let tool = ToolParam::new("t".to_string(), json!({"type": "object"}))
            .with_defer_loading(true)
            .with_allowed_callers(vec!["direct".to_string()])
            .with_input_examples(vec![json!({"a": 1})])
            .with_eager_input_streaming(true);
        assert_eq!(
            to_value(&tool).unwrap(),
            json!({
                "input_schema": {"type": "object"},
                "name": "t",
                "defer_loading": true,
                "allowed_callers": ["direct"],
                "input_examples": [{"a": 1}],
                "eager_input_streaming": true
            })
        );
    }
}
