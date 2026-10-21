//! A tool loop: call Claude, run the client tools it asks for, send the results back, repeat.

use crate::client::Anthropic;
use crate::error::{Error, Result};
use crate::tool::ClientTool;
use crate::types::{
    ContentBlock, Message, MessageCreateParams, MessageParam, MessageRole, StopReason, ToolChoice,
    ToolResultBlock, ToolUnionParam, ToolUseBlock,
};

/// The outcome of [`Anthropic::run_tools`].
///
/// `responses` holds every response in order, the final one included, so summing their `usage`
/// gives the cost of the whole run. If the run ended on the turn budget while Claude was still
/// calling tools, the final assistant turn holds `tool_use` blocks with no answers yet: answer
/// them before sending `messages` again.
#[derive(Debug, Clone)]
pub struct ToolRun {
    /// The last response Claude sent; its `stop_reason` says why the run ended.
    pub message: Message,
    /// Every response in the order received, `message` included.
    pub responses: Vec<Message>,
    /// The whole conversation, starting with the caller's messages and ending with the final
    /// assistant turn, ready to extend with a new user message and send again.
    pub messages: Vec<MessageParam>,
}

impl Anthropic {
    /// Runs Claude with client tools until it stops asking for them or `max_turns` calls are made.
    ///
    /// Each tool's definition is appended to `params.tools`, after any tools already there (server
    /// tools such as web fetch or code execution keep working). `max_turns` counts model calls; when
    /// the request has any tools, the last allowed call is sent with `tool_choice: none` so Claude
    /// must answer in text, and every other call keeps the caller's `tool_choice`. Each call goes
    /// through [`Anthropic::send`] and its retries.
    ///
    /// When Claude stops for `tool_use`, every client `tool_use` block runs in order, one at a
    /// time, and the results go back in one user message. A call naming no known tool gets an
    /// `is_error` result. A tool returning `Err` ends the run with that error. A `pause_turn` stop
    /// is resent as is. Any other stop reason ends the run.
    ///
    /// History is append-only: each assistant turn is kept exactly as returned, thinking
    /// signatures and server tool blocks included, because models reject replayed thinking when
    /// earlier messages change. The conversation therefore grows every turn; for long runs set
    /// `context_management` or `compaction` on `params` rather than trimming messages.
    ///
    /// Fails with a validation error, before any request, when `max_turns` is 0 or two tools share
    /// a name, counting the named tools already in `params.tools` (an MCP toolset has no name).
    pub async fn run_tools(
        &self,
        mut params: MessageCreateParams,
        tools: &[&dyn ClientTool],
        max_turns: u32,
    ) -> Result<ToolRun> {
        if max_turns == 0 {
            return Err(Error::validation(
                "max_turns must be at least 1",
                Some("max_turns".to_string()),
            ));
        }
        let existing = params.tools.iter().flatten().filter_map(tool_name);
        let names: Vec<&str> = existing
            .chain(tools.iter().map(|tool| tool.name()))
            .collect();
        for (index, name) in names.iter().enumerate() {
            if names.iter().take(index).any(|earlier| earlier == name) {
                return Err(Error::validation(
                    format!("two tools are named {name}"),
                    Some("tools".to_string()),
                ));
            }
        }

        if !tools.is_empty() {
            params
                .tools
                .get_or_insert_with(Vec::new)
                .extend(tools.iter().map(|tool| tool.definition()));
        }
        // `tool_choice: none` is only valid alongside tools.
        let has_tools = params.tools.as_ref().is_some_and(|tools| !tools.is_empty());
        let caller_tool_choice = params.tool_choice.clone();
        let mut responses = Vec::new();
        let mut turn = 0_u32;

        loop {
            turn += 1;
            let last_turn = turn >= max_turns;
            params.tool_choice = if last_turn && has_tools {
                Some(ToolChoice::None)
            } else {
                caller_tool_choice.clone()
            };

            let response = self.send(params.clone()).await?;
            params.messages.push(MessageParam::new_with_blocks(
                response.content.clone(),
                MessageRole::Assistant,
            ));

            let tool_uses: Vec<&ToolUseBlock> = response
                .content
                .iter()
                .filter_map(ContentBlock::as_tool_use)
                .collect();
            tracing::debug!(
                turn,
                stop_reason = ?response.stop_reason,
                tools = ?tool_uses.iter().map(|tool_use| tool_use.name.as_str()).collect::<Vec<_>>(),
                "tool loop turn"
            );

            let next = match response.stop_reason {
                Some(StopReason::ToolUse) if !tool_uses.is_empty() && !last_turn => {
                    let mut results = Vec::with_capacity(tool_uses.len());
                    for tool_use in tool_uses {
                        let result = match tools.iter().find(|tool| tool.name() == tool_use.name) {
                            Some(tool) => tool.call(tool_use).await?,
                            None => ToolResultBlock::new(tool_use.id.clone())
                                .with_string_content(format!(
                                    "No tool named {} is available.",
                                    tool_use.name
                                ))
                                .with_error(true),
                        };
                        results.push(ContentBlock::ToolResult(result));
                    }
                    params
                        .messages
                        .push(MessageParam::new_with_blocks(results, MessageRole::User));
                    true
                }
                Some(StopReason::PauseTurn) if !last_turn => true,
                _ => false,
            };

            if !next {
                responses.push(response.clone());
                return Ok(ToolRun {
                    message: response,
                    responses,
                    messages: params.messages,
                });
            }
            responses.push(response);
        }
    }
}

/// The name a tool definition gives Claude; an MCP toolset names a server, not a tool.
fn tool_name(tool: &ToolUnionParam) -> Option<&str> {
    match tool {
        ToolUnionParam::CustomTool(tool) => Some(&tool.name),
        ToolUnionParam::Bash20241022(tool) => Some(&tool.name),
        ToolUnionParam::Bash20250124(tool) => Some(&tool.name),
        ToolUnionParam::TextEditor20250124(tool) => Some(&tool.name),
        ToolUnionParam::TextEditor20250429(tool) => Some(&tool.name),
        ToolUnionParam::TextEditor20250728(tool) => Some(&tool.name),
        ToolUnionParam::WebFetch20250910(tool)
        | ToolUnionParam::WebFetch20260209(tool)
        | ToolUnionParam::WebFetch20260309(tool)
        | ToolUnionParam::WebFetch20260318(tool) => Some(&tool.name),
        ToolUnionParam::CodeExecution20250825(tool)
        | ToolUnionParam::CodeExecution20260120(tool)
        | ToolUnionParam::CodeExecution20260521(tool) => Some(&tool.name),
        ToolUnionParam::ToolSearchToolRegex20251119(tool)
        | ToolUnionParam::ToolSearchToolBm2520251119(tool) => Some(&tool.name),
        ToolUnionParam::Memory20250818(tool) => Some(&tool.name),
        ToolUnionParam::McpToolset(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use futures::future::BoxFuture;
    use serde_json::{Value, json};

    use super::*;
    use crate::test_support::{Canned, serve};
    use crate::types::{KnownModel, Model};

    /// Answers with its input, or fails with an authentication error when `fail` is set.
    struct Echo {
        name: &'static str,
        fail: bool,
    }

    impl ClientTool for Echo {
        fn name(&self) -> &str {
            self.name
        }

        fn definition(&self) -> ToolUnionParam {
            ToolUnionParam::new_custom_tool(
                self.name.to_string(),
                json!({"type": "object", "properties": {"text": {"type": "string"}}}),
            )
        }

        fn call<'a>(
            &'a self,
            tool_use: &'a ToolUseBlock,
        ) -> BoxFuture<'a, Result<ToolResultBlock>> {
            Box::pin(async move {
                if self.fail {
                    return Err(Error::authentication("rejected key"));
                }
                Ok(ToolResultBlock::new(tool_use.id.clone())
                    .with_string_content(format!("echo {}", tool_use.input["text"])))
            })
        }
    }

    const ECHO: Echo = Echo {
        name: "echo",
        fail: false,
    };

    fn reply(stop_reason: &str, content: Value) -> Canned {
        Canned::json(
            200,
            json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-5-5",
                "content": content,
                "stop_reason": stop_reason,
                "stop_sequence": null,
                "usage": {"input_tokens": 3, "output_tokens": 2}
            })
            .to_string(),
        )
    }

    fn tool_use(id: &str, name: &str) -> Value {
        json!({"type": "tool_use", "id": id, "name": name, "input": {"text": "hi"}})
    }

    fn client(base_url: String) -> Anthropic {
        Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
            .with_max_retries(0)
    }

    fn params() -> MessageCreateParams {
        MessageCreateParams::new(
            256,
            vec![MessageParam::user("question")],
            Model::Known(KnownModel::ClaudeSonnet55),
        )
    }

    fn bodies(recorded: &std::sync::Mutex<Vec<crate::test_support::Recorded>>) -> Vec<Value> {
        recorded
            .lock()
            .unwrap()
            .iter()
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn tool_use_then_end_turn_appends_assistant_turn_and_results() {
        let assistant_content = json!([
            {"type": "text", "text": "Let me check."},
            tool_use("toolu_1", "echo")
        ]);
        let (base_url, recorded) = serve(vec![
            reply("tool_use", assistant_content.clone()),
            reply("end_turn", json!([{"type": "text", "text": "Done."}])),
        ])
        .await;
        let web_fetch = ToolUnionParam::new_web_fetch_tool();
        let run = client(base_url)
            .run_tools(params().with_tools(vec![web_fetch]), &[&ECHO], 5)
            .await
            .unwrap();

        let bodies = bodies(&recorded);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["tools"], bodies[1]["tools"]);
        let tools = bodies[0]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1]["name"], "echo");
        assert_eq!(
            bodies[1]["messages"],
            json!([
                {"role": "user", "content": "question"},
                {"role": "assistant", "content": assistant_content},
                {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": "echo \"hi\""
                }]}
            ])
        );

        assert_eq!(run.responses.len(), 2);
        assert_eq!(run.message.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(run.messages.len(), 4);
        assert_eq!(run.messages[3].role, MessageRole::Assistant);
    }

    #[tokio::test]
    async fn thinking_block_is_replayed_verbatim() {
        let thinking = json!({
            "type": "thinking",
            "thinking": "The user wants an echo.",
            "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds"
        });
        let (base_url, recorded) = serve(vec![
            reply(
                "tool_use",
                json!([thinking.clone(), tool_use("toolu_1", "echo")]),
            ),
            reply("end_turn", json!([{"type": "text", "text": "Done."}])),
        ])
        .await;
        client(base_url)
            .run_tools(params(), &[&ECHO], 5)
            .await
            .unwrap();

        let bodies = bodies(&recorded);
        assert_eq!(bodies[1]["messages"][1]["content"][0], thinking);
    }

    #[tokio::test]
    async fn unknown_tool_gets_error_result() {
        let (base_url, recorded) = serve(vec![
            reply("tool_use", json!([tool_use("toolu_1", "missing")])),
            reply("end_turn", json!([{"type": "text", "text": "Done."}])),
        ])
        .await;
        client(base_url)
            .run_tools(params(), &[&ECHO], 5)
            .await
            .unwrap();

        let bodies = bodies(&recorded);
        assert_eq!(
            bodies[1]["messages"][2]["content"],
            json!([{
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": "No tool named missing is available.",
                "is_error": true
            }])
        );
    }

    #[tokio::test]
    async fn tool_error_ends_run_without_another_request() {
        let failing = Echo {
            name: "echo",
            fail: true,
        };
        let (base_url, recorded) = serve(vec![
            reply("tool_use", json!([tool_use("toolu_1", "echo")])),
            reply("end_turn", json!([{"type": "text", "text": "Done."}])),
        ])
        .await;
        let err = client(base_url)
            .run_tools(params(), &[&failing], 5)
            .await
            .unwrap_err();

        assert!(err.is_authentication(), "{err}");
        assert_eq!(recorded.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pause_turn_resends_with_assistant_turn_and_no_user_message() {
        let paused = json!([{"type": "text", "text": "Still working."}]);
        let (base_url, recorded) = serve(vec![
            reply("pause_turn", paused.clone()),
            reply("end_turn", json!([{"type": "text", "text": "Done."}])),
        ])
        .await;
        let run = client(base_url)
            .run_tools(params(), &[&ECHO], 5)
            .await
            .unwrap();

        let bodies = bodies(&recorded);
        assert_eq!(
            bodies[1]["messages"],
            json!([
                {"role": "user", "content": "question"},
                {"role": "assistant", "content": paused}
            ])
        );
        assert_eq!(run.responses.len(), 2);
    }

    #[tokio::test]
    async fn last_allowed_turn_forces_tool_choice_none() {
        let (base_url, recorded) = serve(vec![
            reply("tool_use", json!([tool_use("toolu_1", "echo")])),
            reply("tool_use", json!([tool_use("toolu_2", "echo")])),
            reply("tool_use", json!([tool_use("toolu_3", "echo")])),
        ])
        .await;
        let run = client(base_url)
            .run_tools(params().with_tool_choice(ToolChoice::auto()), &[&ECHO], 2)
            .await
            .unwrap();

        let bodies = bodies(&recorded);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["tool_choice"], json!({"type": "auto"}));
        assert_eq!(bodies[1]["tool_choice"], json!({"type": "none"}));
        assert_eq!(run.responses.len(), 2);
        assert_eq!(run.message.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(run.messages.len(), 4);
    }

    #[tokio::test]
    async fn invalid_arguments_send_nothing() {
        let (base_url, recorded) = serve(vec![reply(
            "end_turn",
            json!([{"type": "text", "text": "x"}]),
        )])
        .await;
        let client = client(base_url);

        let err = client
            .run_tools(params(), &[&ECHO, &ECHO], 5)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Validation { .. }), "{err}");

        let err = client.run_tools(params(), &[&ECHO], 0).await.unwrap_err();
        assert!(matches!(err, Error::Validation { .. }), "{err}");

        assert!(recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn names_clashing_with_params_tools_send_nothing() {
        let (base_url, recorded) = serve(vec![reply(
            "end_turn",
            json!([{"type": "text", "text": "x"}]),
        )])
        .await;
        let client = client(base_url);
        let fetch = Echo {
            name: "web_fetch",
            fail: false,
        };
        let cases = [
            (ECHO.definition(), &ECHO, "two tools are named echo"),
            (
                ToolUnionParam::new_web_fetch_tool(),
                &fetch,
                "two tools are named web_fetch",
            ),
        ];
        for (existing, tool, expected) in cases {
            let err = client
                .run_tools(params().with_tools(vec![existing]), &[tool], 5)
                .await
                .unwrap_err();
            match err {
                Error::Validation { message, param } => {
                    assert_eq!(message, expected);
                    assert_eq!(param.as_deref(), Some("tools"));
                }
                other => panic!("expected a validation error, got {other}"),
            }
        }

        assert!(recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_tools_keeps_the_callers_tool_choice() {
        let (base_url, recorded) = serve(vec![reply(
            "end_turn",
            json!([{"type": "text", "text": "Done."}]),
        )])
        .await;
        let run = client(base_url)
            .run_tools(params().with_tool_choice(ToolChoice::auto()), &[], 1)
            .await
            .unwrap();

        let bodies = bodies(&recorded);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["tool_choice"], json!({"type": "auto"}));
        assert!(bodies[0].get("tools").is_none(), "{}", bodies[0]);
        assert_eq!(run.message.stop_reason, Some(StopReason::EndTurn));
    }
}
