use claudius::{
    Anthropic, CodeExecutionTool, ContentBlock, KnownModel, MessageCreateParams, ResponseInclusion,
    ToolUnionParam, WebFetchTool, WebSearchTool,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Anthropic::new(None)?;
    let model = KnownModel::ClaudeHaiku55;

    let params = MessageCreateParams::simple(
        "Look up the current stable Rust version and fetch its release notes page title, \
         then compute 17 * 23 with code execution. Keep the answer short.",
        model,
    )
    .with_tools(vec![
        ToolUnionParam::WebSearch20260318(WebSearchTool::new().with_max_uses(3)),
        ToolUnionParam::WebFetch20260318(
            WebFetchTool::new().with_response_inclusion(ResponseInclusion::Full),
        ),
        ToolUnionParam::CodeExecution20260521(CodeExecutionTool::new()),
    ]);

    let message = client.send(params).await?;

    for block in &message.content {
        match block {
            ContentBlock::Text(text) => println!("text: {}", text.text),
            other => println!("{}", other.type_name()),
        }
    }

    println!("server_tool_use: {:?}", message.usage.server_tool_use);
    println!(
        "cost: {} micro-cents",
        model.cost_micro_cents(&message.usage)
    );
    Ok(())
}
