use std::collections::HashSet;
use std::error::Error;
use std::io;

use serde::Deserialize;
use serde_json::json;
use topcoat_si::{
    AccumulatingStream, Anthropic, ContentBlock, ContentBlockDelta, KnownModel, Message,
    MessageCreateParams, MessageParam, MessageRole, MessageStreamEvent, Model, StopReason,
    ToolParam, ToolResultBlock, ToolUnionParam, ToolUseBlock,
};

// Each turn is a billed model call; stop a run that keeps calling tools.
const MAX_TURNS: usize = 8;

#[derive(Debug, Deserialize)]
struct MathAddInput {
    a: i64,
    b: i64,
}

#[derive(Debug, Deserialize)]
struct ShoutInput {
    text: String,
}

fn run_tool(tool_use: &ToolUseBlock) -> ToolResultBlock {
    match tool_use.name.as_str() {
        "math_add" => match MathAddInput::deserialize(&tool_use.input) {
            Ok(input) => ToolResultBlock::new(tool_use.id.clone())
                .with_string_content((input.a + input.b).to_string()),
            Err(err) => {
                ToolResultBlock::error(tool_use.id.clone(), format!("invalid input: {err}"))
            }
        },
        "shout" => match ShoutInput::deserialize(&tool_use.input) {
            Ok(input) => ToolResultBlock::new(tool_use.id.clone())
                .with_string_content(input.text.to_uppercase()),
            Err(err) => {
                ToolResultBlock::error(tool_use.id.clone(), format!("invalid input: {err}"))
            }
        },
        _ => ToolResultBlock::error(
            tool_use.id.clone(),
            format!("unknown tool: {}", tool_use.name),
        ),
    }
}

/// Whether sending a failed stream's request again is clean: the error is retryable and the
/// stream had not yielded an event yet. The crate never resends a stream itself.
fn clean_to_resend(err: &(dyn Error + 'static)) -> bool {
    err.downcast_ref::<topcoat_si::Error>()
        .is_some_and(|err| err.is_retryable() && err.events_delivered().unwrap_or(0) == 0)
}

async fn stream_message(
    client: &Anthropic,
    params: &MessageCreateParams,
) -> Result<Message, Box<dyn Error>> {
    use futures::StreamExt;
    use std::io::Write;

    let stream = client.stream(params).await?;
    let (mut events, done) = AccumulatingStream::new(stream);
    let mut active_tool_uses = HashSet::new();
    let mut stdout = io::stdout();

    while let Some(event) = events.next().await {
        match event? {
            MessageStreamEvent::ContentBlockStart(start) => match start.content_block {
                ContentBlock::Text(text) if !text.text.is_empty() => {
                    print!("{}", text.text);
                    stdout.flush()?;
                }
                ContentBlock::ToolUse(tool_use) => {
                    active_tool_uses.insert(start.index);
                    println!("\nTool use: {} ({})", tool_use.name, tool_use.id);
                    print!("Input: ");
                    stdout.flush()?;
                }
                _ => {}
            },
            MessageStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                ContentBlockDelta::TextDelta(text_delta) => {
                    print!("{}", text_delta.text);
                    stdout.flush()?;
                }
                ContentBlockDelta::InputJsonDelta(json_delta)
                    if active_tool_uses.contains(&delta.index) =>
                {
                    print!("{}", json_delta.partial_json);
                    stdout.flush()?;
                }
                _ => {}
            },
            MessageStreamEvent::ContentBlockStop(stop) if active_tool_uses.remove(&stop.index) => {
                println!();
            }
            _ => {}
        }
    }

    match done.await {
        Ok(result) => Ok(result?),
        Err(_) => Err("failed to receive accumulated streaming message".into()),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Anthropic::try_new(None)?;
    let prompt = "Use the math_add tool to add 24 and 18. Then respond with the sum.";

    let tools = vec![
        ToolUnionParam::CustomTool(
            ToolParam::new(
                "math_add".to_string(),
                json!({
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer", "description": "First number" },
                        "b": { "type": "integer", "description": "Second number" }
                    },
                    "required": ["a", "b"]
                }),
            )
            .with_description("Add two integers and return the sum as a string.".to_string()),
        ),
        ToolUnionParam::CustomTool(
            ToolParam::new(
                "shout".to_string(),
                json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Text to uppercase" }
                    },
                    "required": ["text"]
                }),
            )
            .with_description("Uppercase the provided text.".to_string()),
        ),
    ];

    let mut messages = vec![MessageParam::user(prompt)];

    for turn in 1..=MAX_TURNS {
        println!("\n--- Turn {turn} ---");
        let params = MessageCreateParams::new_streaming(
            1024,
            messages.clone(),
            Model::Known(KnownModel::ClaudeHaiku55),
        )
        .with_tools(tools.clone());

        let response = match stream_message(&client, &params).await {
            Ok(response) => response,
            Err(err) if clean_to_resend(err.as_ref()) => {
                println!("\nStream failed before any event ({err}); sending it again");
                stream_message(&client, &params).await?
            }
            Err(err) => return Err(err),
        };

        if response.stop_reason != Some(StopReason::ToolUse) {
            // Push the final reply so `messages` holds the whole conversation, e.g. for a
            // follow-up turn.
            messages.push(MessageParam::from(response));
            return Ok(());
        }

        let tool_results: Vec<ContentBlock> = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tool_use) => {
                    Some(ContentBlock::ToolResult(run_tool(tool_use)))
                }
                _ => None,
            })
            .collect();

        messages.push(MessageParam::from(response));
        if tool_results.is_empty() {
            return Ok(());
        }
        messages.push(MessageParam::new_with_blocks(
            tool_results,
            MessageRole::User,
        ));
    }

    println!("\nStopped after {MAX_TURNS} turns: Claude was still calling tools.");
    Ok(())
}
