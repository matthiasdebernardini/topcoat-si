use serde::{Deserialize, Serialize};

/// Indicates whether a model capability is supported.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CapabilitySupport {
    /// Whether the capability is supported.
    pub supported: bool,
}

/// Context-management features exposed by the Models API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContextManagementCapability {
    /// Support for clearing thinking blocks.
    pub clear_thinking_20251015: Option<CapabilitySupport>,
    /// Support for clearing tool-use blocks.
    pub clear_tool_uses_20250919: Option<CapabilitySupport>,
    /// Support for server-side compaction.
    pub compact_20260112: Option<CapabilitySupport>,
    /// Whether any context-management strategy is supported.
    pub supported: bool,
}

/// Server-side tools exposed by the Models API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ServerToolsCapability {
    /// Whether server tools are supported.
    pub supported: bool,
    /// Code execution server tool support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_execution: Option<CapabilitySupport>,
}

/// Effort levels exposed by the Models API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EffortCapability {
    /// High effort support.
    pub high: CapabilitySupport,
    /// Low effort support.
    pub low: CapabilitySupport,
    /// Maximum effort support.
    pub max: CapabilitySupport,
    /// Medium effort support.
    pub medium: CapabilitySupport,
    /// Whether effort control is supported.
    pub supported: bool,
    /// Extra-high effort support, when reported.
    pub xhigh: Option<CapabilitySupport>,
}

/// Thinking modes exposed by the Models API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ThinkingTypes {
    /// Adaptive thinking support.
    pub adaptive: CapabilitySupport,
    /// Manual extended-thinking support.
    pub enabled: CapabilitySupport,
    /// Support for explicitly disabling thinking, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<CapabilitySupport>,
    /// Support for thinking between tool calls, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub between_tools: Option<CapabilitySupport>,
}

/// Thinking support exposed by the Models API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ThinkingCapability {
    /// Whether any thinking mode is supported.
    pub supported: bool,
    /// Supported thinking configurations.
    pub types: ThinkingTypes,
}

/// Capability metadata returned for a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelCapabilities {
    /// Message Batches API support.
    pub batch: CapabilitySupport,
    /// Citation generation support.
    pub citations: CapabilitySupport,
    /// Code execution support.
    pub code_execution: CapabilitySupport,
    /// Context-management support and strategies.
    pub context_management: ContextManagementCapability,
    /// Effort-control support and levels.
    pub effort: EffortCapability,
    /// Image input support.
    pub image_input: CapabilitySupport,
    /// PDF input support.
    pub pdf_input: CapabilitySupport,
    /// Server-side tool support, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_tools: Option<ServerToolsCapability>,
    /// Structured output and strict tool-schema support.
    pub structured_outputs: CapabilitySupport,
    /// Thinking support and modes.
    pub thinking: ThinkingCapability,
}
