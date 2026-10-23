//! Writes help-center articles to a fresh turbopuffer namespace, searches them three ways and,
//! with an Anthropic key, lets Claude answer from them with citations. The namespace is deleted
//! at the end, also when a step fails.
//!
//! Needs TURBOPUFFER_API_KEY and TURBOPUFFER_REGION (or TURBOPUFFER_BASE_URL), in a region
//! where voyage/voyage-4-lite is available. ANTHROPIC_API_KEY is optional.

use std::time::{SystemTime, UNIX_EPOCH};

use topcoat_si::turbopuffer::{
    Document, Filter, Namespace, Query, QueryResponse, SearchMode, TurbopufferClient,
    TurbopufferSearch,
};
use topcoat_si::{
    Anthropic, Citation, ClientTool, ContentBlock, KnownModel, MessageCreateParams, MessageParam,
    Model,
};

const MODEL: &str = "voyage/voyage-4-lite";

#[tokio::main]
async fn main() -> topcoat_si::Result<()> {
    let client = TurbopufferClient::from_env()?;
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let help = client.namespace(format!("topcoat-si-example-{seconds}"), MODEL)?;
    println!("namespace {}", help.name());

    let outcome = run(&help).await;
    let deleted = help.delete_namespace().await;
    println!("deleted namespace: {deleted:?}");
    outcome?;
    deleted
}

async fn run(help: &Namespace) -> topcoat_si::Result<()> {
    // turbopuffer does not document the status for a namespace that was never written.
    match help.query(&Query::new("refund")).await {
        Ok(response) => println!("query before any write: {} hits", response.hits.len()),
        Err(error) => println!(
            "query before any write: status {:?}: {error}",
            error.status()
        ),
    }

    let first = help
        .upsert([
            Document::new(
                "refunds",
                "Refunds go back to the original card within 5 to 10 business days.",
            )
            .with_attribute("title", "Refund policy")
            .with_attribute("url", "https://help.acme.example/refunds")
            .with_attribute("kind", "policy"),
            Document::new(
                "shipping",
                "Orders ship within two business days. Express shipping arrives the next day.",
            )
            .with_attribute("title", "Shipping times")
            .with_attribute("url", "https://help.acme.example/shipping")
            .with_attribute("kind", "policy"),
            // No url, so a citation of it uses a turbopuffer:// source.
            Document::new(
                "a-4471",
                "The A-4471 bracket fits 2x4 studs and holds 40 kg.",
            )
            .with_attribute("title", "A-4471 bracket")
            .with_attribute("kind", "product"),
        ])
        .await?;
    println!(
        "first upsert: {} rows, {} embedding tokens",
        first.rows, first.embedding_tokens
    );
    // A second write resends the same schema, which turbopuffer must accept.
    let second = help
        .upsert([
            Document::new(
                "closed-card",
                "If the original card is closed, the refund goes to the bank that issued it.",
            )
            .with_attribute("title", "Refunds to a closed card")
            .with_attribute("url", "https://help.acme.example/closed-card")
            .with_attribute("kind", "policy"),
            Document::new(
                "b-120",
                "The B-120 shelf is 120 cm wide and needs two A-4471 brackets.",
            )
            .with_attribute("title", "B-120 shelf")
            .with_attribute("kind", "product"),
        ])
        .await?;
    println!(
        "second upsert: {} rows, {} embedding tokens",
        second.rows, second.embedding_tokens
    );

    let hybrid = help
        .query(&Query::new("can I get my money back?").with_limit(3))
        .await?;
    print_hits("hybrid", &hybrid);
    let keyword = help
        .query(&Query::new("A-4471").with_mode(SearchMode::Keyword))
        .await?;
    print_hits("keyword", &keyword);
    let filtered = help
        .query(
            &Query::new("how long does it take")
                .with_mode(SearchMode::Vector)
                .with_filter(Filter::equals("kind", "policy")),
        )
        .await?;
    print_hits("vector, policies only", &filtered);

    if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|key| !key.trim().is_empty()) {
        ask_claude(help).await?;
    }
    Ok(())
}

fn print_hits(label: &str, response: &QueryResponse) {
    println!(
        "\n{label} ({} embedding tokens):",
        response.embedding_tokens
    );
    for hit in &response.hits {
        println!(
            "  {} {:.4} {}",
            hit.id,
            hit.score,
            hit.text.as_deref().unwrap_or("")
        );
    }
}

async fn ask_claude(help: &Namespace) -> topcoat_si::Result<()> {
    let anthropic = Anthropic::try_new(None)?;
    let search = TurbopufferSearch::try_new(help.clone(), "search_help", "Acme's help center")?;
    let params = MessageCreateParams::new(
        1024,
        vec![MessageParam::user(
            "My card was closed. Where does my refund go, and how many brackets does the B-120 \
             shelf need? Search the help center and cite it.",
        )],
        Model::Known(KnownModel::ClaudeSonnet55),
    );
    let tools: [&dyn ClientTool; 1] = [&search];
    let run = anthropic
        .run_tools(params, &tools, 6)
        .await
        .map_err(|failed| failed.error)?;
    println!("\nClaude:");
    for block in &run.message.content {
        let ContentBlock::Text(text) = block else {
            continue;
        };
        println!("{}", text.text);
        for citation in text.citations.iter().flatten() {
            if let Citation::SearchResultLocation(location) = citation {
                println!("  cites {} ({:?})", location.source, location.title);
            }
        }
    }
    Ok(())
}
