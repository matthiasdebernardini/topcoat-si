use serde::{Deserialize, Serialize};

use crate::types::{ModelLifecycle, open_union};

/// Parameters for listing models.
///
/// This struct contains the parameters that can be passed when listing models
/// from the Anthropic API.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelListParams {
    /// ID of the object to use as a cursor for pagination.
    ///
    /// When provided, returns the page of results immediately after this object.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "after_id")]
    pub after_id: Option<String>,

    /// ID of the object to use as a cursor for pagination.
    ///
    /// When provided, returns the page of results immediately before this object.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "before_id")]
    pub before_id: Option<String>,

    /// Number of items to return per page.
    ///
    /// Defaults to `20`. Ranges from `1` to `1000`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,

    /// Only return models in these lifecycle states.
    ///
    /// Sent as repeated `lifecycle[]=<value>` query pairs. When omitted, the API
    /// returns active and deprecated models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle: Vec<ModelLifecycle>,

    /// Optional header to specify the beta version(s) you want to use.
    ///
    /// These are sent as the `anthropic-beta` HTTP header, not as query
    /// parameters. They are merged with any client-level default betas.
    #[serde(skip)]
    pub betas: Option<Vec<String>>,
}

impl ModelListParams {
    /// Create a new, empty instance of ModelListParams.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by lifecycle state; may be called repeatedly.
    #[must_use]
    pub fn with_lifecycle(mut self, lifecycle: ModelLifecycle) -> Self {
        self.lifecycle.push(lifecycle);
        self
    }

    /// Rejects an `Unknown` lifecycle read from a reply, which has no API value to send.
    pub(crate) fn check_known_values(&self) -> crate::Result<()> {
        self.lifecycle
            .iter()
            .position(|lifecycle| *lifecycle == ModelLifecycle::Unknown)
            .map_or(Ok(()), |index| {
                Err(open_union::unknown_in_request(format!(
                    "lifecycle[{index}]"
                )))
            })
    }

    /// The query-string pairs for this request, with `lifecycle[]` repeated per value.
    pub(crate) fn query_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        if let Some(after_id) = &self.after_id {
            pairs.push(("after_id".to_string(), after_id.clone()));
        }
        if let Some(before_id) = &self.before_id {
            pairs.push(("before_id".to_string(), before_id.clone()));
        }
        if let Some(limit) = self.limit {
            pairs.push(("limit".to_string(), limit.to_string()));
        }
        pairs.extend(
            self.lifecycle
                .iter()
                .map(|lifecycle| ("lifecycle[]".to_string(), lifecycle.as_str().to_string())),
        );
        pairs
    }

    /// Set the after_id parameter for pagination.
    ///
    /// When provided, returns the page of results immediately after this object.
    #[must_use]
    pub fn with_after_id(mut self, after_id: impl Into<String>) -> Self {
        self.after_id = Some(after_id.into());
        self
    }

    /// Set the before_id parameter for pagination.
    ///
    /// When provided, returns the page of results immediately before this object.
    #[must_use]
    pub fn with_before_id(mut self, before_id: impl Into<String>) -> Self {
        self.before_id = Some(before_id.into());
        self
    }

    /// Set the limit for the number of items to return per page.
    ///
    /// Defaults to `20`. Ranges from `1` to `1000`.
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the beta versions to use for this request.
    #[must_use]
    pub fn with_betas(mut self, betas: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.betas = Some(betas.into_iter().map(Into::into).collect());
        self
    }

    /// Add a single beta version to use for this request.
    #[must_use]
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        self.betas.get_or_insert_with(Vec::new).push(beta.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_list_params() {
        let params = ModelListParams::default();
        assert_eq!(params.after_id, None);
        assert_eq!(params.before_id, None);
        assert_eq!(params.limit, None);
        assert_eq!(params.betas, None);
    }

    #[test]
    fn model_list_params_builder() {
        let params = ModelListParams::new()
            .with_after_id("model_123")
            .with_limit(50)
            .with_beta("token-counting-2024-11-01");

        assert_eq!(params.after_id, Some("model_123".to_string()));
        assert_eq!(params.before_id, None);
        assert_eq!(params.limit, Some(50));
        assert_eq!(
            params.betas,
            Some(vec!["token-counting-2024-11-01".to_string()])
        );
    }

    #[test]
    fn model_list_params_serialization() {
        let params = ModelListParams::new()
            .with_limit(50)
            .with_beta("token-counting-2024-11-01");

        let json = serde_json::to_value(&params).unwrap();
        let expected = serde_json::json!({
            "limit": 50,
        });
        assert_eq!(
            json, expected,
            "betas should not appear in serialized JSON (they are sent as HTTP headers)"
        );
    }

    #[test]
    fn model_list_params_deserialization() {
        let json = serde_json::json!({
            "after_id": "model_123",
            "limit": 50,
        });
        let params: ModelListParams = serde_json::from_value(json).unwrap();

        assert_eq!(params.after_id, Some("model_123".to_string()));
        assert_eq!(params.limit, Some(50));
        assert_eq!(params.betas, None);
    }

    #[test]
    fn lifecycle_is_sent_as_repeated_bracket_pairs() {
        let params = ModelListParams::new()
            .with_limit(5)
            .with_lifecycle(ModelLifecycle::Active)
            .with_lifecycle(ModelLifecycle::Retired);

        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in params.query_pairs() {
            query.append_pair(&key, &value);
        }
        assert_eq!(
            query.finish(),
            "limit=5&lifecycle%5B%5D=active&lifecycle%5B%5D=retired"
        );
    }

    #[test]
    fn lifecycle_omitted_by_default() {
        let params = ModelListParams::new();
        assert!(params.query_pairs().is_empty());
        assert_eq!(
            serde_json::to_value(&params).unwrap(),
            serde_json::json!({})
        );
    }
}
