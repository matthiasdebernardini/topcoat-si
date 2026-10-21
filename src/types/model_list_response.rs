use serde::{Deserialize, Serialize};

use crate::types::ModelInfo;

/// Response from the list models API endpoint.
///
/// Contains a list of available models and pagination information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelListResponse {
    /// List of models returned by the API.
    pub data: Vec<ModelInfo>,

    /// Indicates whether there are more results available.
    ///
    /// If `true`, there are more models available that can be retrieved
    /// by making another request with pagination parameters.
    pub has_more: bool,

    /// The ID of the first object in the current page.
    ///
    /// Can be used for pagination when requesting the previous page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_id: Option<String>,

    /// The ID of the last object in the current page.
    ///
    /// Can be used for pagination when requesting the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_id: Option<String>,
}

impl ModelListResponse {
    /// Get the list of models.
    #[must_use]
    pub fn models(&self) -> &[ModelInfo] {
        &self.data
    }

    /// Check if there are more results available.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    /// Get the first model ID for pagination.
    #[must_use]
    pub fn first_id(&self) -> Option<&str> {
        self.first_id.as_deref()
    }

    /// Get the last model ID for pagination.
    #[must_use]
    pub fn last_id(&self) -> Option<&str> {
        self.last_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelType;
    use time::macros::datetime;

    #[test]
    fn model_list_response_serialization() {
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

        let response = ModelListResponse {
            data: vec![model_info],
            has_more: false,
            first_id: Some("first_id".to_string()),
            last_id: Some("last_id".to_string()),
        };

        let json = serde_json::to_value(&response).unwrap();
        let expected = serde_json::json!({
            "data": [{
                "id": "claude-3-7-sonnet-20250219",
                "created_at": "2025-02-19T00:00:00Z",
                "display_name": "Claude 3.7 Sonnet",
                "type": "model"
            }],
            "has_more": false,
            "first_id": "first_id",
            "last_id": "last_id"
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn model_list_response_deserialization() {
        let json = serde_json::json!({
            "data": [{
                "id": "claude-3-7-sonnet-20250219",
                "created_at": "2025-02-19T00:00:00Z",
                "display_name": "Claude 3.7 Sonnet",
                "type": "model"
            }],
            "has_more": false,
            "first_id": "first_id",
            "last_id": "last_id"
        });
        let response: ModelListResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].id, "claude-3-7-sonnet-20250219");
        assert!(!response.has_more);
        assert_eq!(response.first_id, Some("first_id".to_string()));
        assert_eq!(response.last_id, Some("last_id".to_string()));
    }

    #[test]
    fn model_list_response_accessors() {
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

        let response = ModelListResponse {
            data: vec![model_info],
            has_more: true,
            first_id: None,
            last_id: Some("last_id".to_string()),
        };

        assert_eq!(response.models().len(), 1);
        assert!(response.has_more());
        assert_eq!(response.first_id(), None);
        assert_eq!(response.last_id(), Some("last_id"));
    }
}
