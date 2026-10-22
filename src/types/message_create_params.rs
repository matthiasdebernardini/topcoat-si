use serde::{Deserialize, Serialize};

use crate::types::{
    CacheControlEphemeral, CompactionRequest, Container, ContextManagement, DiagnosticsRequest,
    FallbackConfig, Fallbacks, InferenceSpeed, McpServer, MessageParam, Metadata, Model,
    OutputConfig, ServiceTierChoice, SkillType, SystemPrompt, ThinkingConfig, ThinkingDisplay,
    ToolChoice, ToolUnionParam,
};
use crate::types::{message_param, open_union};

/// Security limits for DoS prevention
const MAX_MESSAGE_COUNT: usize = 1000;
const MAX_STOP_SEQUENCES: usize = 100;
const MAX_STOP_SEQUENCE_LENGTH: usize = 1000;
const MAX_TOOLS_COUNT: usize = 100;
const MAX_CONTAINER_SKILLS: usize = 20;
/// Minimum `output_config.task_budget.total` accepted by the API.
const MIN_TASK_BUDGET_TOTAL: u64 = 20_000;

/// Parameters for creating messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct MessageCreateParams {
    /// The maximum number of tokens to generate before stopping.
    ///
    /// Note that our models may stop _before_ reaching this maximum. This parameter
    /// only specifies the absolute maximum number of tokens to generate.
    ///
    /// Different models have different maximum values for this parameter. See
    /// [models](https://docs.anthropic.com/en/docs/models-overview) for details.
    pub max_tokens: u32,

    /// Input messages.
    ///
    /// Our models are trained to operate on alternating `user` and `assistant`
    /// conversational turns. When creating a new `Message`, you specify the prior
    /// conversational turns with the `messages` parameter, and the model then generates
    /// the next `Message` in the conversation. Consecutive `user` or `assistant` turns
    /// in your request will be combined into a single turn.
    pub messages: Vec<MessageParam>,

    /// The model that will complete your prompt.
    ///
    /// See [models](https://docs.anthropic.com/en/docs/models-overview) for additional
    /// details and options.
    pub model: Model,

    /// Top-level cache control for automatic prompt caching.
    ///
    /// When set, the system automatically applies the cache breakpoint to the last
    /// cacheable block in the request. This is the simplest way to enable prompt caching
    /// for multi-turn conversations where the growing message history should be cached
    /// automatically.
    ///
    /// See
    /// [automatic caching](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching#automatic-caching)
    /// for details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,

    /// An object describing metadata about the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,

    /// Output configuration for structured outputs, effort control and task budgets.
    ///
    /// `format` constrains Claude's response to a JSON schema (no beta header
    /// needed), `effort` controls thinking depth with adaptive thinking, and
    /// `task_budget` sets a token budget for the whole task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,

    /// Custom text sequences that will cause the model to stop generating.
    ///
    /// Our models will normally stop when they have naturally completed their turn,
    /// which will result in a response `stop_reason` of `"end_turn"`.
    ///
    /// If you want the model to stop generating when it encounters custom strings of
    /// text, you can use the `stop_sequences` parameter. If the model encounters one of
    /// the custom sequences, the response `stop_reason` value will be `"stop_sequence"`
    /// and the response `stop_sequence` value will contain the matched stop sequence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,

    /// System prompt.
    ///
    /// A system prompt is a way of providing context and instructions to Claude, such
    /// as specifying a particular goal or role. See the
    /// [guide to system prompts](https://docs.anthropic.com/en/docs/system-prompts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,

    /// Configuration for Claude's extended thinking.
    ///
    /// When thinking is on, responses include `thinking` content blocks before the final
    /// answer, and thinking tokens count towards `max_tokens`. Current models reject
    /// `enabled`; use `adaptive`. Opus 5.5, Sonnet 5.5 and Fable 5.1 also reject
    /// `disabled`; Haiku 5.5 rejects it at effort `xhigh` or `max`.
    ///
    /// See
    /// [extended thinking](https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking)
    /// for details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,

    /// How the model should use the provided tools.
    ///
    /// The model can decide by itself or not use tools at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// Definitions of tools that the model may use.
    ///
    /// If you include `tools` in your API request, the model may return `tool_use`
    /// content blocks that represent the model's use of those tools. You can then run
    /// those tools using the tool input generated by the model and then optionally
    /// return results back to the model using `tool_result` content blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolUnionParam>>,

    /// Models to fall back to if this request is refused.
    ///
    /// When a model with safety classifiers (such as Claude Fable 5) declines a request
    /// with `stop_reason: "refusal"`, the API can transparently retry the request against
    /// the models listed here, in order, and return the first successful response.
    ///
    /// Server-side fallback is in beta and requires the `fallbacks` beta header. See
    /// [server-side fallback](https://docs.anthropic.com/en/docs/build-with-claude/refusals-and-fallback#server-side-fallback)
    /// for details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallbacks: Option<Fallbacks>,

    /// Opaque cache-miss credit from a refused request, redeemed when manually
    /// retrying on a fallback model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_credit_token: Option<String>,

    /// Which service tier this request may use.
    ///
    /// Distinct from the response `ServiceTier`, which reports the tier used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTierChoice>,

    /// Inference speed for this request.
    ///
    /// Fast mode is generally available on Opus 5.5 and needs no beta header. It
    /// is not allowed in Message Batches; batch validation rejects it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<InferenceSpeed>,

    /// Where inference runs: `"global"` or `"us"`.
    ///
    /// `"us"` is billed at 1.1x.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_geo: Option<String>,

    /// Container to run code execution and skills in, either an existing
    /// container id or a configuration with at most 20 skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<Container>,

    /// Opt in to prompt-cache diagnostics.
    ///
    /// Send `previous_message_id: null` on the first turn and pass the previous
    /// response `id` on later turns; the response `diagnostics` field then
    /// reports the earliest divergence that caused a cache miss. Generally
    /// available; no beta header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<DiagnosticsRequest>,

    /// Server-side context management (tool-use clearing, thinking clearing,
    /// compaction edits).
    ///
    /// Requires the `context-management-2025-06-27` beta, plus
    /// `compact-2026-01-12` when a compact edit is present; both are added
    /// automatically. Mutually exclusive with `compaction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_management: Option<ContextManagement>,

    /// Compaction on demand.
    ///
    /// The response contains a `compaction` content block and
    /// `stop_reason: "compaction"`; the caller must re-send with that block
    /// first in `messages` and the summarized messages removed. Requires the
    /// `compact-2026-09-04` beta, added automatically. Mutually exclusive with
    /// `context_management`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionRequest>,

    /// Remote MCP servers the API connects to.
    ///
    /// Requires the `mcp-client-2025-11-20` beta, added automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<McpServer>>,

    /// Whether to incrementally stream the response using server-sent events.
    ///
    /// See [streaming](https://docs.anthropic.com/en/api/messages-streaming) for
    /// details.
    pub stream: bool,

    /// Beta feature headers to include with this request.
    ///
    /// These are merged with any client-level default betas and the betas
    /// auto-detected from the features this request uses. Duplicates are
    /// removed before sending.
    #[serde(skip)]
    pub betas: Option<Vec<String>>,
}

impl MessageCreateParams {
    /// Create a new message creation parameters with streaming disabled.
    #[must_use]
    pub fn new(max_tokens: u32, messages: Vec<MessageParam>, model: Model) -> Self {
        Self {
            max_tokens,
            messages,
            model,
            cache_control: None,
            metadata: None,
            output_config: None,
            stop_sequences: None,
            system: None,
            thinking: None,
            tool_choice: None,
            tools: None,
            fallbacks: None,
            fallback_credit_token: None,
            service_tier: None,
            speed: None,
            inference_geo: None,
            container: None,
            diagnostics: None,
            context_management: None,
            compaction: None,
            mcp_servers: None,
            stream: false,
            betas: None,
        }
    }

    /// Create new streaming message creation parameters.
    #[must_use]
    pub fn new_streaming(max_tokens: u32, messages: Vec<MessageParam>, model: Model) -> Self {
        Self {
            max_tokens,
            messages,
            model,
            cache_control: None,
            metadata: None,
            output_config: None,
            stop_sequences: None,
            system: None,
            thinking: None,
            tool_choice: None,
            tools: None,
            fallbacks: None,
            fallback_credit_token: None,
            service_tier: None,
            speed: None,
            inference_geo: None,
            container: None,
            diagnostics: None,
            context_management: None,
            compaction: None,
            mcp_servers: None,
            stream: true,
            betas: None,
        }
    }

    /// Enable automatic prompt caching.
    ///
    /// Applies the cache breakpoint to the last cacheable block automatically.
    #[must_use]
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }

    /// Add metadata to the parameters.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Add output configuration.
    ///
    /// Supports the structured output format, the effort parameter for adaptive
    /// thinking, and a task budget. Structured outputs need no beta header.
    ///
    /// # Example
    ///
    /// ```
    /// use serde_json::json;
    /// use topcoat_si::{MessageCreateParams, OutputConfig, OutputFormat, KnownModel};
    ///
    /// let params = MessageCreateParams::simple("Extract info", KnownModel::ClaudeHaiku55)
    ///     .with_output_config(OutputConfig::new().with_format(OutputFormat::json_schema(json!({
    ///         "type": "object",
    ///         "properties": {
    ///             "name": { "type": "string" }
    ///         },
    ///         "required": ["name"],
    ///         "additionalProperties": false
    ///     }))));
    /// ```
    #[must_use]
    pub fn with_output_config(mut self, output_config: OutputConfig) -> Self {
        self.output_config = Some(output_config);
        self
    }

    /// Add stop sequences to the parameters.
    #[must_use]
    pub fn with_stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.stop_sequences = Some(stop_sequences);
        self
    }

    /// Add a system prompt: a `String`, a `&str` or a `Vec<TextBlock>`.
    #[must_use]
    pub fn with_system(mut self, system: impl Into<SystemPrompt>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// Add thinking configuration to the parameters.
    #[must_use]
    pub fn with_thinking(mut self, thinking: ThinkingConfig) -> Self {
        self.thinking = Some(thinking);
        self
    }

    /// Add tool choice to the parameters.
    #[must_use]
    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    /// Add tools to the parameters.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolUnionParam>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Sets the streaming option.
    #[must_use]
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// Set the models to fall back to if this request is refused.
    ///
    /// Replaces any previously configured fallbacks. Server-side fallback is in beta and
    /// requires the `fallbacks` beta header.
    #[must_use]
    pub fn with_fallbacks(
        mut self,
        fallbacks: impl IntoIterator<Item = impl Into<FallbackConfig>>,
    ) -> Self {
        self.fallbacks = Some(Fallbacks::Models(
            fallbacks.into_iter().map(Into::into).collect(),
        ));
        self
    }

    /// Use Anthropic's recommended fallback routing for each refusal category.
    #[must_use]
    pub fn with_default_fallbacks(mut self) -> Self {
        self.fallbacks = Some(Fallbacks::Default);
        self
    }

    /// Redeem the cache-miss credit returned by a prior refusal.
    #[must_use]
    pub fn with_fallback_credit_token(mut self, token: impl Into<String>) -> Self {
        self.fallback_credit_token = Some(token.into());
        self
    }

    /// Add a single fallback model for this request.
    ///
    /// Appends to any previously configured fallbacks, preserving order.
    #[must_use]
    pub fn with_fallback(mut self, fallback: impl Into<FallbackConfig>) -> Self {
        let config: FallbackConfig = fallback.into();
        match self
            .fallbacks
            .get_or_insert_with(|| Fallbacks::Models(Vec::new()))
        {
            Fallbacks::Models(fallbacks) => fallbacks.push(config),
            Fallbacks::Default => self.fallbacks = Some(Fallbacks::Models(vec![config])),
        }
        self
    }

    /// Choose the service tier for this request.
    #[must_use]
    pub fn with_service_tier(mut self, service_tier: ServiceTierChoice) -> Self {
        self.service_tier = Some(service_tier);
        self
    }

    /// Set the inference speed (fast mode is not allowed in batches).
    #[must_use]
    pub fn with_speed(mut self, speed: InferenceSpeed) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Set the inference geography (`"global"` or `"us"`).
    #[must_use]
    pub fn with_inference_geo(mut self, inference_geo: impl Into<String>) -> Self {
        self.inference_geo = Some(inference_geo.into());
        self
    }

    /// Set the container for code execution and skills.
    #[must_use]
    pub fn with_container(mut self, container: Container) -> Self {
        self.container = Some(container);
        self
    }

    /// Opt in to cache diagnostics, comparing against the previous response.
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: DiagnosticsRequest) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// Set server-side context management.
    #[must_use]
    pub fn with_context_management(mut self, context_management: ContextManagement) -> Self {
        self.context_management = Some(context_management);
        self
    }

    /// Request compaction on demand.
    #[must_use]
    pub fn with_compaction(mut self, compaction: CompactionRequest) -> Self {
        self.compaction = Some(compaction);
        self
    }

    /// Set the remote MCP servers.
    #[must_use]
    pub fn with_mcp_servers(mut self, mcp_servers: Vec<McpServer>) -> Self {
        self.mcp_servers = Some(mcp_servers);
        self
    }

    /// Set the beta feature headers for this request.
    ///
    /// These are merged with any client-level default betas and the betas
    /// auto-detected from the features this request uses. Duplicates are
    /// removed before sending.
    #[must_use]
    pub fn with_betas(mut self, betas: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.betas = Some(betas.into_iter().map(Into::into).collect());
        self
    }

    /// Add a single beta feature header for this request.
    ///
    /// Appends to any previously set betas.
    #[must_use]
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        self.betas.get_or_insert_with(Vec::new).push(beta.into());
        self
    }

    /// Validate all parameters before sending to the API with security checks.
    ///
    /// Performs comprehensive validation including DoS prevention measures:
    /// - Limits on message count and size to prevent resource exhaustion
    /// - Validation of all numeric ranges
    /// - Security checks on string inputs
    pub fn validate(&self) -> Result<(), crate::Error> {
        // Basic parameter validation
        if self.max_tokens == 0 {
            let requires_output = [
                self.stream,
                self.thinking
                    .is_some_and(|thinking| thinking != ThinkingConfig::Disabled),
                self.output_config
                    .as_ref()
                    .is_some_and(|config| config.format.is_some()),
            ]
            .into_iter()
            .any(std::convert::identity);
            if requires_output {
                return Err(crate::Error::validation(
                    "max_tokens: 0 cannot request streaming, thinking, or structured output",
                    Some("max_tokens".to_string()),
                ));
            }
        }

        // Security: Prevent excessive token requests that could be expensive
        if self.max_tokens > 1_000_000 {
            return Err(crate::Error::validation(
                format!(
                    "max_tokens exceeds security limit of 1,000,000, got {}",
                    self.max_tokens
                ),
                Some("max_tokens".to_string()),
            ));
        }

        if self.messages.is_empty() {
            return Err(crate::Error::validation(
                "At least one message is required",
                Some("messages".to_string()),
            ));
        }

        // Security: Prevent DoS via excessive message count
        if self.messages.len() > MAX_MESSAGE_COUNT {
            return Err(crate::Error::validation(
                format!(
                    "Message count {} exceeds security limit of {}",
                    self.messages.len(),
                    MAX_MESSAGE_COUNT
                ),
                Some("messages".to_string()),
            ));
        }

        if let Some(tools) = &self.tools {
            for tool in tools {
                tool.validate()?;
            }
        }

        // Validate stop sequences
        if let Some(ref stop_sequences) = self.stop_sequences {
            if stop_sequences.len() > MAX_STOP_SEQUENCES {
                return Err(crate::Error::validation(
                    format!(
                        "Stop sequences count {} exceeds limit of {}",
                        stop_sequences.len(),
                        MAX_STOP_SEQUENCES
                    ),
                    Some("stop_sequences".to_string()),
                ));
            }

            for (i, seq) in stop_sequences.iter().enumerate() {
                if seq.len() > MAX_STOP_SEQUENCE_LENGTH {
                    return Err(crate::Error::validation(
                        format!(
                            "Stop sequence {} length {} exceeds limit of {}",
                            i,
                            seq.len(),
                            MAX_STOP_SEQUENCE_LENGTH
                        ),
                        Some(format!("stop_sequences[{i}]")),
                    ));
                }

                // Security: Check for potentially problematic characters
                if seq.contains('\0') {
                    return Err(crate::Error::validation(
                        format!("Stop sequence {i} contains null bytes"),
                        Some(format!("stop_sequences[{i}]")),
                    ));
                }
            }
        }

        // Validate tools count
        if let Some(ref tools) = self.tools
            && tools.len() > MAX_TOOLS_COUNT
        {
            return Err(crate::Error::validation(
                format!(
                    "Tools count {} exceeds limit of {}",
                    tools.len(),
                    MAX_TOOLS_COUNT
                ),
                Some("tools".to_string()),
            ));
        }

        if let Some(Fallbacks::Models(fallbacks)) = &self.fallbacks
            && fallbacks.len() > 3
        {
            return Err(crate::Error::validation(
                "fallbacks may contain at most 3 models",
                Some("fallbacks".to_string()),
            ));
        }
        if let Some(Fallbacks::Models(fallbacks)) = &self.fallbacks {
            if fallbacks.is_empty() {
                return Err(crate::Error::validation(
                    "fallbacks must not be empty",
                    Some("fallbacks".to_string()),
                ));
            }
            for (index, fallback) in fallbacks.iter().enumerate() {
                if fallback.model == self.model
                    || fallbacks[..index]
                        .iter()
                        .any(|previous| previous.model == fallback.model)
                {
                    return Err(crate::Error::validation(
                        "fallback models must be distinct from the primary and each other",
                        Some(format!("fallbacks[{index}].model")),
                    ));
                }
            }
        }

        message_param::check_turns(&self.messages)?;
        if self.speed == Some(InferenceSpeed::Unknown) {
            return Err(open_union::unknown_in_request("speed".to_string()));
        }
        if let Some(Fallbacks::Models(fallbacks)) = &self.fallbacks
            && let Some(index) = fallbacks
                .iter()
                .position(|fallback| fallback.speed == Some(InferenceSpeed::Unknown))
        {
            return Err(open_union::unknown_in_request(format!(
                "fallbacks[{index}].speed"
            )));
        }
        if let Some(Container::Config { skills, .. }) = &self.container
            && let Some(index) = skills
                .iter()
                .position(|skill| skill.r#type == SkillType::Unknown)
        {
            return Err(open_union::unknown_in_request(format!(
                "container.skills[{index}].type"
            )));
        }

        if self.compaction.is_some() && self.context_management.is_some() {
            return Err(crate::Error::validation(
                "compaction and context_management are mutually exclusive",
                Some("compaction".to_string()),
            ));
        }

        if let Some(task_budget) = self.output_config.as_ref().and_then(|c| c.task_budget)
            && task_budget.total < MIN_TASK_BUDGET_TOTAL
        {
            return Err(crate::Error::validation(
                format!(
                    "task_budget.total must be at least {MIN_TASK_BUDGET_TOTAL}, got {}",
                    task_budget.total
                ),
                Some("output_config.task_budget.total".to_string()),
            ));
        }

        if let Some(Container::Config { skills, .. }) = &self.container
            && skills.len() > MAX_CONTAINER_SKILLS
        {
            return Err(crate::Error::validation(
                format!(
                    "container may contain at most {MAX_CONTAINER_SKILLS} skills, got {}",
                    skills.len()
                ),
                Some("container.skills".to_string()),
            ));
        }

        // Validate thinking config with security checks
        if let Some(ref thinking) = self.thinking {
            match thinking {
                ThinkingConfig::Enabled { budget_tokens }
                | ThinkingConfig::EnabledWithDisplay { budget_tokens, .. } => {
                    if *budget_tokens < 1024 {
                        return Err(crate::Error::validation(
                            format!(
                                "Thinking budget must be at least 1024 tokens, got {budget_tokens}"
                            ),
                            Some("thinking.budget_tokens".to_string()),
                        ));
                    }
                    if *budget_tokens > self.max_tokens {
                        return Err(crate::Error::validation(
                            format!(
                                "Thinking budget ({budget_tokens}) cannot exceed max_tokens ({})",
                                self.max_tokens
                            ),
                            Some("thinking.budget_tokens".to_string()),
                        ));
                    }

                    // Security: Prevent excessive thinking budget
                    if *budget_tokens > 100_000 {
                        return Err(crate::Error::validation(
                            format!(
                                "Thinking budget {budget_tokens} exceeds security limit of 100,000"
                            ),
                            Some("thinking.budget_tokens".to_string()),
                        ));
                    }
                }
                ThinkingConfig::Disabled => {
                    // No validation needed for disabled state
                }
                ThinkingConfig::Adaptive
                | ThinkingConfig::AdaptiveWithDisplay { .. }
                | ThinkingConfig::BetweenTools => {
                    // No validation needed for adaptive or between-tools thinking
                }
            }
        }

        Ok(())
    }

    /// Create a simple message request with sensible defaults.
    ///
    /// This is a convenience method for creating basic message requests without
    /// needing to specify all parameters explicitly.
    #[must_use]
    pub fn simple(prompt: impl Into<MessageParam>, model: impl Into<Model>) -> Self {
        Self::new(
            1024, // Reasonable default for max_tokens
            vec![prompt.into()],
            model.into(),
        )
    }

    /// Create a simple streaming message request with sensible defaults.
    #[must_use]
    pub fn simple_streaming(prompt: impl Into<MessageParam>, model: impl Into<Model>) -> Self {
        Self::new_streaming(
            1024, // Reasonable default for max_tokens
            vec![prompt.into()],
            model.into(),
        )
    }

    /// Add a single message to the parameters.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<MessageParam>) -> Self {
        self.messages.push(message.into());
        self
    }

    /// Add multiple messages to the parameters.
    #[must_use]
    pub fn with_messages(
        mut self,
        messages: impl IntoIterator<Item = impl Into<MessageParam>>,
    ) -> Self {
        self.messages.extend(messages.into_iter().map(|m| m.into()));
        self
    }

    /// Check whether this request uses the beta progress-update display mode.
    pub fn requires_thinking_display_updates_beta(&self) -> bool {
        self.thinking.as_ref().and_then(ThinkingConfig::display) == Some(ThinkingDisplay::Updates)
    }
}

impl Default for MessageCreateParams {
    fn default() -> Self {
        use crate::types::KnownModel;

        Self {
            max_tokens: 1024,
            messages: vec![],
            model: Model::Known(KnownModel::ClaudeSonnet55),
            cache_control: None,
            metadata: None,
            output_config: None,
            stop_sequences: None,
            system: None,
            thinking: None,
            tool_choice: None,
            tools: None,
            fallbacks: None,
            fallback_credit_token: None,
            service_tier: None,
            speed: None,
            inference_geo: None,
            container: None,
            diagnostics: None,
            context_management: None,
            compaction: None,
            mcp_servers: None,
            stream: false,
            betas: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CacheTtl, ContentBlock, ContextEdit, Effort, KnownModel, MessageRole, OutputConfig,
        SkillRef, TaskBudget, ThinkingKeep, ToolResultBlock,
    };
    use serde_json::{json, to_value};

    #[test]
    fn message_create_params_non_streaming() {
        let message = MessageParam::user("Hello, Claude");

        let params = MessageCreateParams::new(
            1000,
            vec![message],
            Model::Known(KnownModel::ClaudeSonnet55),
        )
        .with_system("You are a helpful assistant.");

        let json = to_value(&params).unwrap();
        assert!(!params.stream);

        assert_eq!(
            json,
            json!({
                "max_tokens": 1000,
                "messages": [
                    {
                        "role": "user",
                        "content": "Hello, Claude"
                    }
                ],
                "model": "claude-sonnet-5-5",
                "system": "You are a helpful assistant.",
                "stream": false
            })
        );
    }

    #[test]
    fn message_create_params_streaming() {
        let message = MessageParam::user("Hello, Claude");

        let params = MessageCreateParams::new_streaming(
            1000,
            vec![message],
            Model::Known(KnownModel::ClaudeSonnet55),
        );

        let json = to_value(&params).unwrap();
        assert!(params.stream);

        assert_eq!(
            json,
            json!({
                "max_tokens": 1000,
                "messages": [
                    {
                        "role": "user",
                        "content": "Hello, Claude"
                    }
                ],
                "model": "claude-sonnet-5-5",
                "stream": true
            })
        );
    }

    #[test]
    fn message_create_params_with_stream() {
        let message = MessageParam::user("Hello, Claude");

        let params = MessageCreateParams::new(
            1000,
            vec![message],
            Model::Known(KnownModel::ClaudeSonnet55),
        )
        .with_stream(true);

        let json = to_value(&params).unwrap();
        assert!(params.stream);

        assert_eq!(
            json,
            json!({
                "max_tokens": 1000,
                "messages": [
                    {
                        "role": "user",
                        "content": "Hello, Claude"
                    }
                ],
                "model": "claude-sonnet-5-5",
                "stream": true
            })
        );
    }

    #[test]
    fn message_create_params_simple() {
        let params = MessageCreateParams::simple("Hello, world!", KnownModel::ClaudeSonnet55);

        assert_eq!(params.max_tokens, 1024);
        assert_eq!(params.messages.len(), 1);
        assert_eq!(params.messages[0].role, MessageRole::User);
        assert!(!params.stream);
    }

    #[test]
    fn message_create_params_simple_streaming() {
        let params =
            MessageCreateParams::simple_streaming("Tell me a joke", KnownModel::ClaudeSonnet55);

        assert_eq!(params.max_tokens, 1024);
        assert_eq!(params.messages.len(), 1);
        assert!(params.stream);
    }

    #[test]
    fn message_create_params_default() {
        let params = MessageCreateParams::default();

        assert_eq!(params.max_tokens, 1024);
        assert_eq!(params.messages.len(), 0);
        assert!(!params.stream);
    }

    #[test]
    fn message_create_params_with_message() {
        let params = MessageCreateParams::default()
            .with_message("Hello")
            .with_message(MessageParam::assistant("Hi there"));

        assert_eq!(params.messages.len(), 2);
        assert_eq!(params.messages[0].role, MessageRole::User);
        assert_eq!(params.messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn message_create_params_ergonomic_system() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
            .with_system("You are a helpful assistant.");

        assert!(params.system.is_some());
    }

    #[test]
    fn with_output_config_builder() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
            .with_output_config(OutputConfig::new().with_effort(Effort::High));

        assert!(params.output_config.is_some());
    }

    #[test]
    fn message_create_params_serializes_tool_result_messages_without_duplicate_type() {
        let message = MessageParam::new_with_blocks(
            vec![ContentBlock::ToolResult(
                ToolResultBlock::new("toolu_123").with_string_content("ok"),
            )],
            MessageRole::User,
        );

        let params = MessageCreateParams::new(
            1000,
            vec![message],
            Model::Known(KnownModel::ClaudeSonnet55),
        );

        let json = to_value(&params).unwrap();
        assert_eq!(
            json,
            json!({
                "max_tokens": 1000,
                "messages": [
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "toolu_123",
                                "content": "ok"
                            }
                        ]
                    }
                ],
                "model": "claude-sonnet-5-5",
                "stream": false
            })
        );
    }

    #[test]
    fn with_betas_sets_betas() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
            .with_betas(["compact-2026-01-12", "some-other-beta"]);

        assert_eq!(
            params.betas,
            Some(vec![
                "compact-2026-01-12".to_string(),
                "some-other-beta".to_string()
            ])
        );
    }

    #[test]
    fn with_beta_appends() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
            .with_beta("compact-2026-01-12")
            .with_beta("some-other-beta");

        assert_eq!(
            params.betas,
            Some(vec![
                "compact-2026-01-12".to_string(),
                "some-other-beta".to_string()
            ])
        );
    }

    #[test]
    fn betas_not_serialized() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
            .with_beta("compact-2026-01-12");

        let json = to_value(&params).unwrap();
        assert!(
            json.get("betas").is_none(),
            "betas should not appear in serialized JSON"
        );
    }

    #[test]
    fn fallbacks_none_by_default() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeFable51);
        assert_eq!(params.fallbacks, None);

        // Omitted from serialized JSON when unset.
        let json = to_value(&params).unwrap();
        assert!(json.get("fallbacks").is_none());
    }

    #[test]
    fn with_fallbacks_replaces() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeFable51)
            .with_fallbacks([KnownModel::ClaudeSonnet55, KnownModel::ClaudeHaiku55]);

        assert_eq!(
            params.fallbacks,
            Some(Fallbacks::Models(vec![
                FallbackConfig::new(KnownModel::ClaudeSonnet55),
                FallbackConfig::new(KnownModel::ClaudeHaiku55),
            ]))
        );
    }

    #[test]
    fn with_fallback_appends_in_order() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeFable51)
            .with_fallback(KnownModel::ClaudeSonnet55)
            .with_fallback(KnownModel::ClaudeHaiku55);

        assert_eq!(
            params.fallbacks,
            Some(Fallbacks::Models(vec![
                FallbackConfig::new(KnownModel::ClaudeSonnet55),
                FallbackConfig::new(KnownModel::ClaudeHaiku55),
            ]))
        );
    }

    #[test]
    fn fallbacks_serialization() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeFable51)
            .with_fallbacks([KnownModel::ClaudeSonnet55]);

        let json = to_value(&params).unwrap();
        assert_eq!(
            json.get("fallbacks").unwrap(),
            &json!([{ "model": "claude-sonnet-5-5" }])
        );
    }

    #[test]
    fn fallbacks_deserialization() {
        let json = json!({
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": "Hello" }],
            "model": "claude-fable-5-1",
            "stream": false,
            "fallbacks": [
                { "model": "claude-sonnet-5-5" },
                { "model": "claude-haiku-5-5" }
            ]
        });

        let params: MessageCreateParams = serde_json::from_value(json).unwrap();
        assert_eq!(
            params.fallbacks,
            Some(Fallbacks::Models(vec![
                FallbackConfig::new(KnownModel::ClaudeSonnet55),
                FallbackConfig::new(KnownModel::ClaudeHaiku55),
            ]))
        );
    }

    #[test]
    fn default_fallbacks_serialization() {
        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeFable51)
            .with_default_fallbacks();
        assert_eq!(to_value(params).unwrap()["fallbacks"], json!("default"));
    }

    fn simple_params() -> MessageCreateParams {
        MessageCreateParams::simple("Hello", KnownModel::ClaudeSonnet55)
    }

    #[test]
    fn new_fields_omitted_by_default() {
        let json = to_value(simple_params()).unwrap();
        for key in [
            "service_tier",
            "speed",
            "inference_geo",
            "container",
            "diagnostics",
            "context_management",
            "compaction",
            "mcp_servers",
            "output_format",
        ] {
            assert!(json.get(key).is_none(), "{key} should be omitted");
        }
    }

    #[test]
    fn cache_control_with_one_hour_ttl() {
        let params = simple_params().with_cache_control(CacheControlEphemeral::one_hour());
        assert_eq!(
            to_value(params).unwrap()["cache_control"],
            json!({"type": "ephemeral", "ttl": "1h"})
        );
        assert_eq!(
            CacheControlEphemeral::new()
                .with_ttl(CacheTtl::FiveMinutes)
                .ttl,
            Some(CacheTtl::FiveMinutes)
        );
    }

    #[test]
    fn service_tier_speed_and_geo_serialize() {
        let params = simple_params()
            .with_service_tier(ServiceTierChoice::StandardOnly)
            .with_speed(InferenceSpeed::Fast)
            .with_inference_geo("us");
        let json = to_value(params).unwrap();
        assert_eq!(json["service_tier"], json!("standard_only"));
        assert_eq!(json["speed"], json!("fast"));
        assert_eq!(json["inference_geo"], json!("us"));
    }

    #[test]
    fn diagnostics_first_turn_emits_null_previous_message_id() {
        let params = simple_params().with_diagnostics(DiagnosticsRequest::first_turn());
        let json = to_value(params).unwrap();
        assert_eq!(json["diagnostics"], json!({"previous_message_id": null}));
        assert!(
            json["diagnostics"]
                .as_object()
                .unwrap()
                .contains_key("previous_message_id")
        );
    }

    #[test]
    fn container_serializes_as_string_and_object() {
        let by_id = simple_params().with_container(Container::id("container_011"));
        assert_eq!(
            to_value(by_id).unwrap()["container"],
            json!("container_011")
        );

        let by_config = simple_params().with_container(Container::new_with_skills(vec![
            SkillRef::anthropic("pptx").with_version("latest"),
        ]));
        assert_eq!(
            to_value(by_config).unwrap()["container"],
            json!({"skills": [{"type": "anthropic", "skill_id": "pptx", "version": "latest"}]})
        );
    }

    #[test]
    fn context_management_example_serializes() {
        use crate::types::{ContextTrigger, KeepToolUses};
        let params = simple_params().with_context_management(ContextManagement::new(vec![
            ContextEdit::ClearToolUses20250919 {
                trigger: Some(ContextTrigger::InputTokens { value: 100_000 }),
                keep: Some(KeepToolUses { value: 3 }),
                clear_at_least: Some(ContextTrigger::InputTokens { value: 5000 }),
                exclude_tools: Some(vec!["web_search".to_string()]),
                clear_tool_inputs: Some(true),
            },
        ]));
        assert_eq!(
            to_value(params).unwrap()["context_management"],
            json!({"edits":[{"type":"clear_tool_uses_20250919","trigger":{"type":"input_tokens","value":100000},"keep":{"type":"tool_uses","value":3},"clear_at_least":{"type":"input_tokens","value":5000},"exclude_tools":["web_search"],"clear_tool_inputs":true}]})
        );
    }

    #[test]
    fn clear_thinking_keep_variants_serialize() {
        let all = simple_params().with_context_management(ContextManagement::new(vec![
            ContextEdit::ClearThinking20251015 {
                keep: Some(ThinkingKeep::All),
            },
        ]));
        assert_eq!(
            to_value(all).unwrap()["context_management"],
            json!({"edits": [{"type": "clear_thinking_20251015", "keep": "all"}]})
        );
        let turns = simple_params().with_context_management(ContextManagement::new(vec![
            ContextEdit::ClearThinking20251015 {
                keep: Some(ThinkingKeep::Turns(2)),
            },
        ]));
        assert_eq!(
            to_value(turns).unwrap()["context_management"],
            json!({"edits": [{"type": "clear_thinking_20251015", "keep": {"type": "thinking_turns", "value": 2}}]})
        );
    }

    #[test]
    fn compaction_request_serializes() {
        let params = simple_params()
            .with_compaction(CompactionRequest::new().with_instructions("Keep decisions"));
        assert_eq!(
            to_value(params).unwrap()["compaction"],
            json!({"type": "summarize", "instructions": "Keep decisions"})
        );
    }

    #[test]
    fn mcp_servers_serialize() {
        let params = simple_params().with_mcp_servers(vec![
            McpServer::new("example", "https://mcp.example.com/sse")
                .with_authorization_token("token"),
        ]);
        assert_eq!(
            to_value(params).unwrap()["mcp_servers"],
            json!([{
                "type": "url",
                "url": "https://mcp.example.com/sse",
                "name": "example",
                "authorization_token": "token"
            }])
        );
    }

    #[test]
    fn task_budget_serializes_in_output_config() {
        let params = simple_params().with_output_config(
            OutputConfig::new()
                .with_effort(Effort::High)
                .with_task_budget(TaskBudget::new(64_000)),
        );
        assert_eq!(
            to_value(params).unwrap()["output_config"],
            json!({"effort": "high", "task_budget": {"type": "tokens", "total": 64000}})
        );
    }

    #[test]
    fn between_tools_thinking_serializes() {
        let params = simple_params().with_thinking(ThinkingConfig::between_tools());
        assert_eq!(
            to_value(&params).unwrap()["thinking"],
            json!({"type": "between_tools"})
        );
        assert!(params.validate().is_ok());
    }

    #[test]
    fn validate_rejects_compaction_with_context_management() {
        let params = simple_params()
            .with_compaction(CompactionRequest::new())
            .with_context_management(ContextManagement::new(vec![ContextEdit::compact()]));
        let err = params.validate().unwrap_err();
        assert!(matches!(err.kind(), crate::ErrorKind::Validation { .. }));
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn validate_task_budget_minimum() {
        let make = |total| {
            simple_params()
                .with_output_config(OutputConfig::new().with_task_budget(TaskBudget::new(total)))
        };
        let err = make(19_999).validate().unwrap_err();
        assert!(matches!(err.kind(), crate::ErrorKind::Validation { .. }));
        assert!(err.to_string().contains("task_budget"));
        assert!(make(20_000).validate().is_ok());
    }

    #[test]
    fn validate_container_skill_limit() {
        let skills = |count: usize| {
            (0..count)
                .map(|i| SkillRef::custom(format!("skill_{i}")))
                .collect::<Vec<_>>()
        };
        let too_many = simple_params().with_container(Container::new_with_skills(skills(21)));
        let err = too_many.validate().unwrap_err();
        assert!(matches!(err.kind(), crate::ErrorKind::Validation { .. }));
        assert!(err.to_string().contains("skills"));
        let ok = simple_params().with_container(Container::new_with_skills(skills(20)));
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_values_read_from_replies() {
        let param_of = |params: MessageCreateParams| match params.validate() {
            Err(error) => match error.kind() {
                crate::ErrorKind::Validation { param, .. } => param.clone(),
                _ => panic!("expected a validation error, got {error:?}"),
            },
            other => panic!("expected a validation error, got {other:?}"),
        };

        let mut params = simple_params();
        params.messages[0].role = MessageRole::Unknown;
        assert_eq!(param_of(params).as_deref(), Some("messages[0].role"));

        let params = simple_params().with_speed(InferenceSpeed::Unknown);
        assert_eq!(param_of(params).as_deref(), Some("speed"));

        let params = MessageCreateParams::simple("Hello", KnownModel::ClaudeFable51)
            .with_fallback(KnownModel::ClaudeSonnet55)
            .with_fallback(
                FallbackConfig::new(KnownModel::ClaudeHaiku55).with_speed(InferenceSpeed::Unknown),
            );
        assert_eq!(param_of(params).as_deref(), Some("fallbacks[1].speed"));

        let mut skill = SkillRef::anthropic("xlsx");
        skill.r#type = SkillType::Unknown;
        let params = simple_params().with_container(Container::new_with_skills(vec![skill]));
        assert_eq!(
            param_of(params).as_deref(),
            Some("container.skills[0].type")
        );

        // Serializing a reply type for storage still works.
        assert_eq!(to_value(MessageRole::Unknown).unwrap(), json!("unknown"));
    }
}
