//! Client-side tools: code in your process that answers Claude's tool calls.

use futures::future::BoxFuture;

use crate::error::Result;
use crate::types::{ToolResultBlock, ToolUnionParam, ToolUseBlock};

/// A tool that runs in your process when Claude calls it.
///
/// Return `Ok` with an `is_error` result for problems the model can fix (bad input, a page that
/// will not load, a rate limit). Return `Err` only for problems it cannot fix (missing or rejected
/// API key, no credits left, bad configuration); a tool loop stops on `Err`.
pub trait ClientTool: Send + Sync {
    /// The name Claude uses to call this tool. Must be unique among the tools in one request.
    fn name(&self) -> &str;
    /// The definition sent to Claude in `tools`.
    fn definition(&self) -> ToolUnionParam;
    /// Answers one call whose `name` equals [`ClientTool::name`].
    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>>;
}
