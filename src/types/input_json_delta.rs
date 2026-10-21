use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::str::FromStr;

/// A JSON delta, representing a piece of JSON in a streaming response.
///
/// InputJsonDelta is used in streaming responses to deliver incremental JSON
/// content, typically for tool input parameters. The JSON is streamed as fragments
/// that need to be concatenated to form the complete JSON object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InputJsonDelta {
    /// The partial JSON content.
    ///
    /// This contains a fragment of JSON that should be appended to previously
    /// received fragments to build the complete JSON structure.
    #[serde(rename = "partial_json")]
    pub partial_json: String,
}

impl InputJsonDelta {
    /// Create a new `InputJsonDelta` with the given partial JSON.
    #[must_use]
    pub fn new(partial_json: impl Into<String>) -> Self {
        Self {
            partial_json: partial_json.into(),
        }
    }
}

impl FromStr for InputJsonDelta {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn input_json_delta_serialization() {
        let delta = InputJsonDelta::new(r#"{"key":"#.to_string());
        let json = to_value(&delta).unwrap();

        assert_eq!(
            json,
            json!({
                "partial_json": r#"{"key":"#
            })
        );
    }

    #[test]
    fn input_json_delta_deserialization() {
        let json = json!({
            "partial_json": r#"{"key":"#
        });

        let delta: InputJsonDelta = serde_json::from_value(json).unwrap();
        assert_eq!(delta.partial_json, r#"{"key":"#);
    }

    #[test]
    fn from_str() {
        let delta = "partial json".parse::<InputJsonDelta>().unwrap();
        assert_eq!(delta.partial_json, "partial json");
    }
}
