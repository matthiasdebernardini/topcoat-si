//! Answers a question with Firecrawl web search.
//!
//! Needs ANTHROPIC_API_KEY and FIRECRAWL_API_KEY.

use topcoat_si::{
    Anthropic, ContentBlock, FirecrawlClient, FirecrawlWebSearch, KnownModel, MessageCreateParams,
    MessageParam, MessageRole, Model, StopReason, ToolResultBlock,
};

#[tokio::main]
async fn main() -> topcoat_si::Result<()> {
    let client = Anthropic::new(None)?;
    let search = FirecrawlWebSearch::new(FirecrawlClient::from_env()?);
    let model = KnownModel::ClaudeHaiku55;

    let mut messages = vec![MessageParam::user(
        "What is the latest stable Rust release and what are its headline changes? Cite sources.",
    )];

    loop {
        let params = MessageCreateParams::new(2048, messages.clone(), Model::Known(model))
            .with_tools(vec![search.tool()]);
        let message = client.send(params).await?;
        messages.push(MessageParam::from(message.clone()));

        for block in &message.content {
            match block {
                ContentBlock::Text(text) => println!("{}", text.text),
                ContentBlock::ToolUse(tool_use) => {
                    println!("[{}] {}", tool_use.name, tool_use.input)
                }
                _ => {}
            }
        }

        if message.stop_reason != Some(StopReason::ToolUse) {
            break;
        }

        let mut results = Vec::new();
        for block in &message.content {
            let ContentBlock::ToolUse(tool_use) = block else {
                continue;
            };
            // Every tool_use needs a matching tool_result or the next request is rejected.
            let result = if search.handles(tool_use) {
                search.execute(tool_use).await?
            } else {
                ToolResultBlock::new(tool_use.id.clone())
                    .with_string_content(format!("unknown tool: {}", tool_use.name))
                    .with_error(true)
            };
            results.push(ContentBlock::ToolResult(result));
        }
        messages.push(MessageParam::new_with_blocks(results, MessageRole::User));
    }

    Ok(())
}
