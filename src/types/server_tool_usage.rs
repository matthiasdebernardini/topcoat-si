use std::ops::Add;

use serde::{Deserialize, Serialize};

/// Information about server tool usage for a message.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerToolUsage {
    /// The number of web search tool requests.
    pub web_search_requests: i32,

    /// The number of web fetch tool requests (omitted when serializing if zero).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub web_fetch_requests: i32,
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

impl ServerToolUsage {
    /// Create a new `ServerToolUsage` with the given web search requests count.
    pub fn new(web_search_requests: i32) -> Self {
        Self {
            web_search_requests,
            web_fetch_requests: 0,
        }
    }

    /// Set the number of web fetch tool requests.
    pub fn with_web_fetch_requests(mut self, web_fetch_requests: i32) -> Self {
        self.web_fetch_requests = web_fetch_requests;
        self
    }
}

impl Add for ServerToolUsage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            web_search_requests: self.web_search_requests + rhs.web_search_requests,
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
                "web_search_requests": 5
            })
        );
    }

    #[test]
    fn server_tool_usage_deserialization() {
        let json = json!({
            "web_search_requests": 5
        });

        let usage: ServerToolUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.web_search_requests, 5);
    }

    #[test]
    fn add_server_tool_usage() {
        let usage1 = ServerToolUsage::new(5);
        let usage2 = ServerToolUsage::new(3);
        let result = usage1 + usage2;

        assert_eq!(result.web_search_requests, 8);
    }

    #[test]
    fn add_server_tool_usage_includes_web_fetch() {
        let usage1 = ServerToolUsage::new(1).with_web_fetch_requests(2);
        let usage2 = ServerToolUsage::new(3).with_web_fetch_requests(4);
        let result = usage1 + usage2;

        assert_eq!(result.web_search_requests, 4);
        assert_eq!(result.web_fetch_requests, 6);
    }
}
