use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::types::{ModelCapabilities, ModelLifecycle};

/// Information about a specific model.
///
/// This struct contains details about an Anthropic model, including its
/// unique identifier, creation time, display name, and type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Unique model identifier.
    pub id: String,

    /// Model capability information, when supplied by the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,

    /// RFC 3339 datetime string representing the time at which the model was released.
    ///
    /// May be set to an epoch value if the release date is unknown.
    #[serde(rename = "created_at", with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,

    /// A human-readable name for the model.
    #[serde(rename = "display_name")]
    pub display_name: String,

    /// Maximum input context window size in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,

    /// Maximum accepted value for the Messages API `max_tokens` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,

    /// When the model was deprecated, if it has been.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub deprecated_at: Option<OffsetDateTime>,

    /// When the model retires (or retired), if scheduled.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub retires_at: Option<OffsetDateTime>,

    /// Lifecycle state of the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<ModelLifecycle>,

    /// The model line this model belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,

    /// Object type.
    ///
    /// For Models, this is always `"model"`.
    #[serde(rename = "type")]
    pub r#type: ModelType,
}

/// Type of the model object.
///
/// For model objects, this is always "model".
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelType {
    /// Model type
    Model,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn model_info_serialization() {
        let model_info = ModelInfo {
            id: "claude-3-7-sonnet-20250219".to_string(),
            capabilities: None,
            created_at: datetime!(2025-02-19 0:00:00 UTC),
            display_name: "Claude 3.7 Sonnet".to_string(),
            max_input_tokens: None,
            max_tokens: None,
            deprecated_at: None,
            retires_at: None,
            lifecycle: None,
            line: None,
            r#type: ModelType::Model,
        };

        let json = serde_json::to_value(&model_info).unwrap();
        let expected = serde_json::json!({
            "id": "claude-3-7-sonnet-20250219",
            "created_at": "2025-02-19T00:00:00Z",
            "display_name": "Claude 3.7 Sonnet",
            "type": "model"
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn model_info_deserialization() {
        let json = serde_json::json!({
            "id": "claude-3-7-sonnet-20250219",
            "created_at": "2025-02-19T00:00:00Z",
            "display_name": "Claude 3.7 Sonnet",
            "type": "model"
        });
        let model_info: ModelInfo = serde_json::from_value(json).unwrap();

        assert_eq!(model_info.id, "claude-3-7-sonnet-20250219");
        assert_eq!(model_info.created_at, datetime!(2025-02-19 0:00:00 UTC));
        assert_eq!(model_info.display_name, "Claude 3.7 Sonnet");
        assert_eq!(model_info.r#type, ModelType::Model);
    }

    #[test]
    fn model_info_deserializes_current_capabilities_and_limits() {
        let json = serde_json::json!({
            "id": "claude-opus-5-5",
            "capabilities": {
                "batch": { "supported": true },
                "citations": { "supported": true },
                "code_execution": { "supported": true },
                "context_management": {
                    "clear_thinking_20251015": { "supported": true },
                    "clear_tool_uses_20250919": { "supported": true },
                    "compact_20260112": { "supported": true },
                    "supported": true
                },
                "effort": {
                    "high": { "supported": true },
                    "low": { "supported": true },
                    "max": { "supported": true },
                    "medium": { "supported": true },
                    "supported": true,
                    "xhigh": { "supported": true }
                },
                "image_input": { "supported": true },
                "pdf_input": { "supported": true },
                "structured_outputs": { "supported": true },
                "thinking": {
                    "supported": true,
                    "types": {
                        "adaptive": { "supported": true },
                        "enabled": { "supported": true }
                    }
                }
            },
            "created_at": "2026-07-24T00:00:00Z",
            "display_name": "Claude Opus 5",
            "max_input_tokens": 1_000_000,
            "max_tokens": 128_000,
            "type": "model"
        });

        let model: ModelInfo = serde_json::from_value(json).unwrap();
        assert_eq!(model.max_input_tokens, Some(1_000_000));
        assert_eq!(model.max_tokens, Some(128_000));
        assert!(model.capabilities.unwrap().effort.max.supported);
    }

    #[test]
    fn model_info_deserializes_lifecycle_and_server_tools() {
        let json = serde_json::json!({
            "id": "claude-old",
            "capabilities": {
                "batch": { "supported": true },
                "citations": { "supported": true },
                "code_execution": { "supported": true },
                "context_management": { "supported": false },
                "effort": {
                    "high": { "supported": true },
                    "low": { "supported": true },
                    "max": { "supported": true },
                    "medium": { "supported": true },
                    "supported": true
                },
                "image_input": { "supported": true },
                "pdf_input": { "supported": true },
                "server_tools": {
                    "supported": true,
                    "code_execution": { "supported": true },
                    "web_search": { "supported": false }
                },
                "structured_outputs": { "supported": true },
                "thinking": {
                    "supported": true,
                    "types": {
                        "adaptive": { "supported": true },
                        "enabled": { "supported": true },
                        "disabled": { "supported": true },
                        "between_tools": { "supported": false }
                    }
                }
            },
            "created_at": "2025-02-19T00:00:00Z",
            "display_name": "Claude Old",
            "lifecycle": "deprecated",
            "line": "sonnet",
            "deprecated_at": "2026-09-01T00:00:00Z",
            "retires_at": "2027-01-01T00:00:00Z",
            "type": "model"
        });

        let model: ModelInfo = serde_json::from_value(json).unwrap();
        assert_eq!(model.lifecycle, Some(ModelLifecycle::Deprecated));
        assert_eq!(model.line.as_deref(), Some("sonnet"));
        assert_eq!(model.deprecated_at, Some(datetime!(2026-09-01 0:00:00 UTC)));
        assert_eq!(model.retires_at, Some(datetime!(2027-01-01 0:00:00 UTC)));
        let capabilities = model.capabilities.unwrap();
        let server_tools = capabilities.server_tools.unwrap();
        assert!(server_tools.supported);
        assert_eq!(server_tools.code_execution.map(|c| c.supported), Some(true));
        let types = capabilities.thinking.types;
        assert_eq!(types.disabled.map(|c| c.supported), Some(true));
        assert_eq!(types.between_tools.map(|c| c.supported), Some(false));
    }
}
