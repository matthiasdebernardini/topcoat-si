use std::ops::Add;

use serde::{Deserialize, Serialize};

/// Information about server tool usage for a message.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ServerToolUsage {
    /// The number of web fetch tool requests (omitted when serializing if zero).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub web_fetch_requests: i32,
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

impl ServerToolUsage {
    /// Create a new `ServerToolUsage` with the given web fetch requests count.
    #[must_use]
    pub fn new(web_fetch_requests: i32) -> Self {
        Self { web_fetch_requests }
    }
}

impl Add for ServerToolUsage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            web_fetch_requests: self.web_fetch_requests + rhs.web_fetch_requests,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn server_tool_usage_serialization() {
        let usage = ServerToolUsage::new(5);
        let json = to_value(usage).unwrap();

        assert_eq!(
            json,
            json!({
                "web_fetch_requests": 5
            })
        );
    }

    #[test]
    fn server_tool_usage_deserialization() {
        let json = json!({
            "web_fetch_requests": 5
        });

        let usage: ServerToolUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.web_fetch_requests, 5);
    }

    #[test]
    fn server_tool_usage_ignores_other_server_tool_counts() {
        let json = json!({
            "web_search_requests": 3,
            "web_fetch_requests": 2
        });

        let usage: ServerToolUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage, ServerToolUsage::new(2));
    }

    #[test]
    fn add_server_tool_usage() {
        let usage1 = ServerToolUsage::new(5);
        let usage2 = ServerToolUsage::new(3);
        let result = usage1 + usage2;

        assert_eq!(result.web_fetch_requests, 8);
    }
}
