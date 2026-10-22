//! Live checks against the real Anthropic, Firecrawl and Jev APIs. They cost money, so they are
//! ignored; run them with `cargo test --test live -- --ignored`.
//!
//! Needs ANTHROPIC_API_KEY, FIRECRAWL_API_KEY, TYPESAFE_API_KEY and TYPESAFE_DEFAULT_MODEL.

use futures::StreamExt;
use topcoat_si::jev::{JevClient, JevTool};
use topcoat_si::{
    AccumulatingStream, Anthropic, ClientTool, ContentBlock, FirecrawlClient, FirecrawlWebSearch,
    KnownModel, Message, MessageCreateParams, MessageParam, Model, StopReason,
};

// Enough for the models whose thinking cannot be disabled to think and still answer.
const MAX_TOKENS: u32 = 4096;
const PROMPT: &str = "Reply with the single word: pong";

fn text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

/// Why a finished reply does not count as an answer, if it does not: it must end its turn and
/// carry text.
fn unanswered(message: &Message) -> Option<String> {
    if message.stop_reason != Some(StopReason::EndTurn) {
        return Some(format!(
            "stopped with {:?}, not end_turn: {message:?}",
            message.stop_reason
        ));
    }
    if text(message).trim().is_empty() {
        return Some(format!("gave no text: {message:?}"));
    }
    None
}

#[tokio::test]
#[ignore = "live: calls the real APIs and costs money"]
async fn every_model_answers() {
    let client = Anthropic::try_new(None).expect("ANTHROPIC_API_KEY");
    let mut failures = Vec::new();
    // Listed by hand so a new variant is a conscious add.
    for model in [
        KnownModel::ClaudeOpus55,
        KnownModel::ClaudeSonnet55,
        KnownModel::ClaudeHaiku55,
        KnownModel::ClaudeFable51,
    ] {
        let messages = vec![MessageParam::user(PROMPT)];

        let params = MessageCreateParams::new(MAX_TOKENS, messages.clone(), Model::Known(model));
        match client.send(&params).await {
            Ok(message) => {
                if let Some(why) = unanswered(&message) {
                    failures.push(format!("{model} send {why}"));
                } else if message.usage.output_tokens == 0 {
                    failures.push(format!(
                        "{model} send reported no output tokens: {:?}",
                        message.usage
                    ));
                }
            }
            Err(err) => failures.push(format!("{model} send failed: {err} ({:?})", err.kind())),
        }

        let params = MessageCreateParams::new_streaming(MAX_TOKENS, messages, Model::Known(model));
        let events = match client.stream(&params).await {
            Ok(events) => events,
            Err(err) => {
                failures.push(format!("{model} stream failed: {err} ({:?})", err.kind()));
                continue;
            }
        };
        let (mut events, done) = AccumulatingStream::new(events);
        let mut event_failed = false;
        while let Some(event) = events.next().await {
            if let Err(err) = event {
                failures.push(format!(
                    "{model} stream event failed: {err} ({:?})",
                    err.kind()
                ));
                event_failed = true;
            }
        }
        if event_failed {
            continue;
        }
        match done.await {
            Ok(Ok(message)) => {
                if let Some(why) = unanswered(&message) {
                    failures.push(format!("{model} stream {why}"));
                }
            }
            Ok(Err(err)) => failures.push(format!(
                "{model} stream ended with: {err} ({:?})",
                err.kind()
            )),
            Err(_) => failures.push(format!("{model} stream ended without a final message")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test]
#[ignore = "live: calls the real APIs and costs money"]
async fn tools_reach_firecrawl_and_jev() {
    let client = Anthropic::try_new(None).expect("ANTHROPIC_API_KEY");
    let search = FirecrawlWebSearch::new(FirecrawlClient::from_env().expect("FIRECRAWL_API_KEY"))
        .with_default_limit(2);
    let jev = JevTool::new(JevClient::from_env().expect("TYPESAFE_API_KEY and model"));
    let model = KnownModel::ClaudeHaiku55;

    let params = MessageCreateParams::new(
        2048,
        vec![MessageParam::user(
            "Use one web search to find the latest stable Rust version. Then ask jev one yes/no \
             question: is that version number at least 1.80? Reply with the version and jev's \
             answer.",
        )],
        Model::Known(model),
    );
    let tools: [&dyn ClientTool; 2] = [&search, &jev];
    let run = match client.run_tools(params, &tools, 6).await {
        Ok(run) => run,
        Err(failed) => panic!(
            "run_tools failed after {} responses: {} ({:?})",
            failed.responses.len(),
            failed.error,
            failed.error.kind()
        ),
    };

    let micro_cents: u64 = run
        .responses
        .iter()
        .map(|response| model.cost_micro_cents(&response.usage))
        .sum();
    println!(
        "{} model calls, {micro_cents} micro-cents (about ${:.4})",
        run.responses.len(),
        micro_cents as f64 / 100_000_000.0
    );

    assert!(
        run.text().is_some_and(|text| !text.trim().is_empty()),
        "run ended without text: {:?}",
        run.message
    );
    for tool in tools {
        let used = run.responses.iter().any(|response| {
            response.content.iter().any(
                |block| matches!(block, ContentBlock::ToolUse(tool_use) if tool_use.name == tool.name()),
            )
        });
        assert!(used, "no tool_use for {}: {:?}", tool.name(), run.responses);
    }
}
