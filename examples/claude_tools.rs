//! Claude researches a question with Firecrawl search and page fetch, and asks Jev for quick typed
//! judgments, all driven by `Anthropic::run_tools`.
//!
//! Needs ANTHROPIC_API_KEY, FIRECRAWL_API_KEY, TYPESAFE_API_KEY and TYPESAFE_DEFAULT_MODEL.

use topcoat_si::jev::{JevClient, JevTool};
use topcoat_si::{
    Anthropic, ClientTool, ContentBlock, FirecrawlClient, FirecrawlFetchPage, FirecrawlWebSearch,
    KnownModel, MessageCreateParams, MessageParam, Model,
};

#[tokio::main]
async fn main() -> topcoat_si::Result<()> {
    let client = Anthropic::new(None)?;
    let firecrawl = FirecrawlClient::from_env()?;
    let search = FirecrawlWebSearch::new(firecrawl.clone());
    let fetch = FirecrawlFetchPage::new(firecrawl);
    let jev = JevTool::new(JevClient::from_env()?);
    let model = KnownModel::ClaudeSonnet55;

    let params = MessageCreateParams::new(
        4096,
        vec![MessageParam::user(
            "Find the three most recent stable Rust releases. For each, decide whether it changed \
             the language itself or only the tooling, and cite your sources.",
        )],
        Model::Known(model),
    );
    let tools: [&dyn ClientTool; 3] = [&search, &fetch, &jev];
    let run = client.run_tools(params, &tools, 12).await?;

    for block in &run.message.content {
        if let ContentBlock::Text(text) = block {
            println!("{}", text.text);
        }
    }
    // Every turn is billed, so total the usage across all responses, not just the last one.
    let micro_cents: u64 = run
        .responses
        .iter()
        .map(|response| model.cost_micro_cents(&response.usage))
        .sum();
    println!(
        "\n{} model calls, about ${:.4}",
        run.responses.len(),
        micro_cents as f64 / 100_000_000.0
    );
    Ok(())
}
