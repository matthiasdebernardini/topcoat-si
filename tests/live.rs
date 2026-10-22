//! Live checks against the real Anthropic, Firecrawl and Jev APIs. They cost money, so they are
//! ignored; run them with `cargo test --test live -- --ignored`. Add `--features code-mode` and
//! set MONTY_BIN to the monty worker binary to include the code mode test.
//!
//! Needs ANTHROPIC_API_KEY, FIRECRAWL_API_KEY, TYPESAFE_API_KEY and TYPESAFE_DEFAULT_MODEL.

use futures::StreamExt;
use topcoat_si::jev::{JevClient, JevTool};
use topcoat_si::{
    AccumulatingStream, Anthropic, ClientTool, ContentBlock, FirecrawlClient, FirecrawlFetchPage,
    FirecrawlWebSearch, KnownModel, Message, MessageCreateParams, MessageParam, Model, StopReason,
    ToolRunError,
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

/// The distinct tool names Claude called across `responses`, in first-use order.
/// Fails the test with everything one run can tell: the error with its source, and the tools used.
fn tool_run_failed(failed: &ToolRunError) -> ! {
    panic!(
        "run_tools failed after {} responses: {:?}; tools used: {:?}",
        failed.responses.len(),
        failed.error,
        tool_names(&failed.responses)
    )
}

fn tool_names(responses: &[Message]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for response in responses {
        for block in &response.content {
            if let ContentBlock::ToolUse(tool_use) = block
                && !names.contains(&tool_use.name)
            {
                names.push(tool_use.name.clone());
            }
        }
    }
    names
}

/// Prints the number of model calls and their total cost.
fn print_cost(model: KnownModel, responses: &[Message]) {
    let micro_cents: u64 = responses
        .iter()
        .map(|response| model.cost_micro_cents(&response.usage))
        .sum();
    println!(
        "{} model calls, {micro_cents} micro-cents (about ${:.4})",
        responses.len(),
        micro_cents as f64 / 100_000_000.0
    );
}

#[tokio::test]
#[ignore = "live: calls the real APIs and costs money"]
async fn client_tools_answer() {
    let client = Anthropic::try_new(None).expect("ANTHROPIC_API_KEY");
    let firecrawl = FirecrawlClient::from_env().expect("FIRECRAWL_API_KEY");
    let search = FirecrawlWebSearch::new(firecrawl.clone())
        .with_default_limit(2)
        .with_scrape_options(None);
    let fetch = FirecrawlFetchPage::new(firecrawl);
    let jev = JevTool::new(JevClient::from_env().expect("TYPESAFE_API_KEY and model"));
    let model = KnownModel::ClaudeHaiku55;

    let params = MessageCreateParams::new(
        2048,
        vec![MessageParam::user(
            "Do these three steps in order, each with its tool. First, use web_search once to \
             find the Rust release announcements on blog.rust-lang.org. Second, use fetch_page \
             on one result URL to read that announcement in full. Third, call jev once with the \
             fetched page's first paragraph as the state and one yes/no question: does this \
             text announce a new Rust release? Reply with the URL you fetched and jev's answer.",
        )],
        Model::Known(model),
    );
    let tools: [&dyn ClientTool; 3] = [&search, &fetch, &jev];
    let run = match client.run_tools(params, &tools, 8).await {
        Ok(run) => run,
        Err(failed) => tool_run_failed(&failed),
    };
    print_cost(model, &run.responses);

    assert!(
        run.text().is_some_and(|text| !text.trim().is_empty()),
        "run ended without text: {:?}",
        run.message
    );
    let used = tool_names(&run.responses);
    for tool in tools {
        assert!(
            used.iter().any(|name| name == tool.name()),
            "no tool_use for {}; tools used: {used:?}",
            tool.name()
        );
    }
}

#[cfg(feature = "code-mode")]
#[tokio::test]
#[ignore = "live: calls the real APIs and costs money"]
async fn code_mode_answers() {
    use topcoat_si::code_mode::{self, CodeMode, Limits, packs};
    use topcoat_si::{CacheControlEphemeral, MessageParamContent};

    let client = Anthropic::try_new(None).expect("ANTHROPIC_API_KEY");
    let monty = std::env::var_os("MONTY_BIN").expect("MONTY_BIN");
    let pool = code_mode::pool(monty, 1).await.expect("sandbox pool");
    let firecrawl = FirecrawlClient::from_env().expect("FIRECRAWL_API_KEY");
    let search = FirecrawlWebSearch::new(firecrawl.clone());
    let fetch = FirecrawlFetchPage::new(firecrawl);
    let conversation_packs = vec![
        packs::firecrawl(search, fetch, 20),
        packs::jev(JevClient::from_env().expect("TYPESAFE_API_KEY and model")),
        packs::research(),
        packs::triage(),
        packs::reports(),
    ];
    let code =
        CodeMode::try_new(pool, conversation_packs, Limits::default()).expect("code mode tool");
    let model = KnownModel::ClaudeSonnet55;

    let system = "Do each step in one snippet: fetch everything you need at once with \
        asyncio.gather, then extract in Python. Print only what you need for the answer, \
        not raw pages. When you have enough, stop calling the tool and answer.";
    let params = MessageCreateParams::new(
        4096,
        vec![MessageParam::user(
            "Find three Rust web frameworks, classify each with Jev as actively maintained or \
             not, and return a table.",
        )],
        Model::Known(model),
    )
    .with_system(system)
    .with_cache_control(CacheControlEphemeral::new());

    let tools: [&dyn ClientTool; 1] = [&code];
    let run = client.run_tools(params, &tools, 15).await;
    let finished = code.finish().await;
    let run = match run {
        Ok(run) => run,
        Err(failed) => tool_run_failed(&failed),
    };
    print_cost(model, &run.responses);
    if let Err(err) = finished {
        panic!("finish failed: {err:?}");
    }

    assert!(
        run.text().is_some_and(|text| !text.trim().is_empty()),
        "run ended without text: {:?}",
        run.message
    );
    let used = tool_names(&run.responses);
    assert!(
        used.iter().any(|name| name == "python"),
        "no tool_use for python; tools used: {used:?}"
    );
    // Calling python proves nothing if every snippet raised.
    let results: Vec<_> = run
        .messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageParamContent::Array(blocks) => Some(blocks),
            MessageParamContent::String(_) => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();
    assert!(
        results.iter().any(|result| result.is_error != Some(true)),
        "every python call failed: {results:?}"
    );
}
