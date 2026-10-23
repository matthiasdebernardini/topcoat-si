//! Live checks against the real Anthropic, Firecrawl and Jev APIs. They cost money, so they are
//! ignored; run them with `cargo test --test live -- --ignored`. Add `--features code-mode` and
//! set MONTY_BIN to the monty worker binary to include the code mode tests.
//!
//! Needs ANTHROPIC_API_KEY, FIRECRAWL_API_KEY, TYPESAFE_API_KEY and TYPESAFE_DEFAULT_MODEL.
//!
//! Expected Firecrawl credits per call, from Firecrawl's billing:
//! - `gov_search`: 0 (the Government Index is free);
//! - `map`: 1;
//! - PDF scrape: 1 credit per page read, so at most 3 here;
//! - `credit_usage`: 0;
//! - code mode `gov_search` and `map_site`: 0 and 1.
//!
//! Credits per test, at most:
//! - `firecrawl_endpoints_answer`: about 4, and no Claude;
//! - `code_mode_firecrawl_pack_answers`: about 1, and a few cents of Haiku;
//! - `client_tools_answer`: about 3, one search and one page;
//! - `code_mode_answers`: budgeted at 20 by its pack (the last call can overshoot by its own
//!   cost), typically well under that.
//!
//! A full code mode run typically uses well under 20 credits, and at most about 28.

use futures::StreamExt;
use topcoat_si::firecrawl::{GovSearchRequest, MapRequest, ScrapeOptions, ScrapeRequest};
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

#[tokio::test]
#[ignore = "live: calls the real APIs and costs money"]
async fn firecrawl_endpoints_answer() {
    let firecrawl = FirecrawlClient::from_env().expect("FIRECRAWL_API_KEY");
    let before = firecrawl.credit_usage().await.expect("credit_usage");
    assert!(
        before.remaining_credits >= 0.0,
        "remaining credits: {before:?}"
    );

    let zoning = GovSearchRequest::new("zoning variance board of zoning appeals")
        .with_jurisdiction("us-tn")
        .with_k(3);
    let hits = firecrawl
        .gov_search(&zoning)
        .await
        .expect("gov_search")
        .data
        .web;
    assert!(!hits.is_empty(), "gov_search found nothing");
    assert!(hits.len() <= 3, "gov_search ignored k: {}", hits.len());
    assert!(
        hits.iter().all(|hit| hit.url.is_some()),
        "a hit has no url: {hits:?}"
    );

    let map = MapRequest::new("https://www.nashville.gov")
        .with_search("zoning")
        .with_limit(5);
    let links = firecrawl.map(&map).await.expect("map").links;
    assert!(!links.is_empty(), "map found no links");
    assert!(links.len() <= 5, "map ignored limit: {}", links.len());
    assert!(
        links.iter().all(|link| link.url.is_some()),
        "a link has no url: {links:?}"
    );

    // The IRS W-9 has 6 pages, so 3 are read and page markers are present.
    let options = ScrapeOptions::new()
        .with_pdf_max_pages(3)
        .with_pdf_pages(true)
        .with_pdf_page_markers(true);
    let request =
        ScrapeRequest::new("https://www.irs.gov/pub/irs-pdf/fw9.pdf").with_options(options.clone());
    let scraped = firecrawl.scrape(&request).await.expect("scrape").data;
    let markdown = scraped.markdown.as_deref().unwrap_or_default();
    assert!(!markdown.trim().is_empty(), "the PDF had no markdown");
    // Firecrawl can skip a page number when a paragraph runs across a break, so check the shape.
    let has_marker = markdown.match_indices("<!-- page ").any(|(at, marker)| {
        let digits: String = markdown[at + marker.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        !digits.is_empty() && markdown[at + marker.len() + digits.len()..].starts_with(" -->")
    });
    assert!(has_marker, "no page marker in: {markdown:.300}");
    let pages = scraped.pages.as_deref().unwrap_or_default();
    assert!(
        (2..=3).contains(&pages.len()),
        "expected 2 or 3 pages, got {}",
        pages.len()
    );
    let credits = scraped.credits(&options);
    assert!((1..=3).contains(&credits), "scrape credits: {credits}");
    println!("PDF scrape: {} pages, {credits} credits", pages.len());

    let after = firecrawl.credit_usage().await.expect("credit_usage");
    println!(
        "credits remaining: {} before, {} after",
        before.remaining_credits, after.remaining_credits
    );
    assert!(after.remaining_credits.is_finite() && after.remaining_credits >= 0.0);
}

#[cfg(feature = "code-mode")]
#[tokio::test]
#[ignore = "live: calls the real APIs and costs money"]
async fn code_mode_firecrawl_pack_answers() {
    use std::sync::{Arc, Mutex};

    use topcoat_si::code_mode::packs::{FirecrawlEvent, FirecrawlObserver, FirecrawlPack};
    use topcoat_si::code_mode::{self, CodeMode, Limits};

    struct Recorder(Mutex<Vec<FirecrawlEvent>>);
    impl FirecrawlObserver for Recorder {
        fn event(&self, event: FirecrawlEvent) {
            self.0.lock().expect("recorder lock").push(event);
        }
    }

    let client = Anthropic::try_new(None).expect("ANTHROPIC_API_KEY");
    let monty = std::env::var_os("MONTY_BIN").expect("MONTY_BIN");
    let pool = code_mode::pool(monty, 1).await.expect("sandbox pool");
    let firecrawl = FirecrawlClient::from_env().expect("FIRECRAWL_API_KEY");
    let recorder = Arc::new(Recorder(Mutex::new(Vec::new())));
    let pack = FirecrawlPack::new(firecrawl)
        .with_observer(recorder.clone())
        .build();
    let code = CodeMode::try_new(pool, vec![pack], Limits::default()).expect("code mode tool");
    let model = KnownModel::ClaudeHaiku55;

    let system = "Use each function exactly once, in one snippet, then answer in one sentence.";
    let params = MessageCreateParams::new(
        2048,
        vec![MessageParam::user(
            "Call gov_search(query=\"zoning variance\", k=3, jurisdiction=\"us-tn\") and \
             map_site(url=\"https://www.nashville.gov\", search=\"zoning\", limit=5). Print the \
             number of results from each.",
        )],
        Model::Known(model),
    )
    .with_system(system);

    let tools: [&dyn ClientTool; 1] = [&code];
    let run = client.run_tools(params, &tools, 6).await;
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

    let events = recorder.0.lock().expect("recorder lock");
    let gov = events.iter().find_map(|event| match event {
        FirecrawlEvent::GovSearched { hits, .. } => Some(hits.len()),
        _ => None,
    });
    let map = events.iter().find_map(|event| match event {
        FirecrawlEvent::Mapped { links, credits, .. } => Some((links.len(), *credits)),
        _ => None,
    });
    assert!(gov.is_some_and(|hits| hits > 0), "gov_search: {events:?}");
    assert!(
        map.is_some_and(|(links, credits)| links > 0 && credits <= 1),
        "map_site: {events:?}"
    );
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
