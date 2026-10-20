use std::fmt;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Tool configuration for an MCP (Model Context Protocol) server.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolConfiguration {
    /// Whether the server's tools are enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    /// Restrict the model to these tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
}

/// A remote MCP server the API connects to on the model's behalf.
///
/// Requires the `mcp-client-2025-11-20` beta, which is added automatically.
#[derive(Clone, PartialEq, Eq)]
pub struct McpServer {
    /// URL of the MCP server; must start with `https://`.
    pub url: String,

    /// Name identifying the server within the request.
    pub name: String,

    /// OAuth bearer token for the server. Redacted in `Debug` output.
    pub authorization_token: Option<String>,

    /// Which of the server's tools may be used.
    pub tool_configuration: Option<McpToolConfiguration>,
}

impl McpServer {
    /// Create an MCP server entry.
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            name: name.into(),
            authorization_token: None,
            tool_configuration: None,
        }
    }

    /// Set the authorization token.
    pub fn with_authorization_token(mut self, token: impl Into<String>) -> Self {
        self.authorization_token = Some(token.into());
        self
    }

    /// Set the tool configuration.
    pub fn with_tool_configuration(mut self, configuration: McpToolConfiguration) -> Self {
        self.tool_configuration = Some(configuration);
        self
    }
}

impl fmt::Debug for McpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpServer")
            .field("url", &self.url)
            .field("name", &self.name)
            .field(
                "authorization_token",
                &self.authorization_token.as_ref().map(|_| "<redacted>"),
            )
            .field("tool_configuration", &self.tool_configuration)
            .finish()
    }
}

impl Serialize for McpServer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = 3
            + usize::from(self.authorization_token.is_some())
            + usize::from(self.tool_configuration.is_some());
        let mut state = serializer.serialize_struct("McpServer", len)?;
        state.serialize_field("type", "url")?;
        state.serialize_field("url", &self.url)?;
        state.serialize_field("name", &self.name)?;
        if let Some(token) = &self.authorization_token {
            state.serialize_field("authorization_token", token)?;
        }
        if let Some(configuration) = &self.tool_configuration {
            state.serialize_field("tool_configuration", configuration)?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for McpServer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            url: String,
            name: String,
            #[serde(default)]
            authorization_token: Option<String>,
            #[serde(default)]
            tool_configuration: Option<McpToolConfiguration>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            url: raw.url,
            name: raw.name,
            authorization_token: raw.authorization_token,
            tool_configuration: raw.tool_configuration,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialization_always_emits_type() {
        let server = McpServer::new("example", "https://mcp.example.com/sse");
        assert_eq!(
            serde_json::to_value(&server).unwrap(),
            json!({"type": "url", "url": "https://mcp.example.com/sse", "name": "example"})
        );
    }

    #[test]
    fn serialization_with_options() {
        let server = McpServer::new("example", "https://mcp.example.com/sse")
            .with_authorization_token("secret")
            .with_tool_configuration(McpToolConfiguration {
                enabled: Some(true),
                allowed_tools: Some(vec!["search".to_string()]),
            });
        assert_eq!(
            serde_json::to_value(&server).unwrap(),
            json!({
                "type": "url",
                "url": "https://mcp.example.com/sse",
                "name": "example",
                "authorization_token": "secret",
                "tool_configuration": {"enabled": true, "allowed_tools": ["search"]}
            })
        );
    }

    #[test]
    fn debug_redacts_token() {
        let server = McpServer::new("example", "https://mcp.example.com/sse")
            .with_authorization_token("super-secret-token");
        let debug = format!("{server:?}");
        assert!(!debug.contains("super-secret-token"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn round_trip() {
        let server = McpServer::new("a", "https://x").with_authorization_token("t");
        let value = serde_json::to_value(&server).unwrap();
        let back: McpServer = serde_json::from_value(value).unwrap();
        assert_eq!(back, server);
    }
}
