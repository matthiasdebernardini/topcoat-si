//! Writes help-center articles to a fresh turbopuffer namespace through turbopuffer's SDK,
//! searches them three ways and, with an Anthropic key, lets Claude answer from them with
//! citations. The namespace is deleted at the end, also when a step fails.
//!
//! Needs TURBOPUFFER_API_KEY and TURBOPUFFER_REGION (or TURBOPUFFER_BASE_URL), in a region
//! where voyage/voyage-4-lite is available. ANTHROPIC_API_KEY is optional.

use std::time::{SystemTime, UNIX_EPOCH};

use topcoat_si::turbopuffer::{Client, Filter, Namespace, TurbopufferSearch};
use topcoat_si::{
    Anthropic, Citation, ClientTool, ContentBlock, KnownModel, MessageCreateParams, MessageParam,
    Model,
};
use turbopuffer::types::{AttributeSchemaConfig, Expr, QueryResult, RankBy, RerankBy, Row};

const MODEL: &str = "voyage/voyage-4-lite";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::from_env()?;
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let help = client.namespace(format!("topcoat-si-example-{seconds}"));
    println!("namespace {}", help.id());

    let outcome = run(&help).await;
    let deleted = help.delete_all().send().await;
    println!("deleted namespace: {:?}", deleted.as_ref().map(drop));
    outcome?;
    deleted?;
    Ok(())
}

fn article(id: &str, text: &str, title: &str, url: Option<&str>, kind: &str) -> Row {
    let row = Row::new(id)
        .insert("text", text)
        .insert("title", title)
        .insert("kind", kind);
    match url {
        Some(url) => row.insert("url", url),
        None => row,
    }
}

/// Upserts `rows` with the schema that has turbopuffer embed `text` and index it for BM25.
async fn write(help: &Namespace, rows: Vec<Row>) -> topcoat_si::Result<()> {
    let written = help
        .write()
        .upsert_rows(rows)
        .schema([(
            "text",
            AttributeSchemaConfig::new("string")
                .embed(MODEL)
                .full_text_search(true),
        )])
        .distance_metric("cosine_distance")
        .send()
        .await?;
    println!(
        "upsert: {} rows, {} embedding tokens",
        written.rows_upserted.unwrap_or(0),
        written
            .performance
            .and_then(|performance| performance.embedding_tokens)
            .unwrap_or(0)
    );
    Ok(())
}

async fn run(help: &Namespace) -> topcoat_si::Result<()> {
    // turbopuffer does not document the status for a namespace that was never written.
    match help
        .query()
        .rank_by(RankBy::bm25("text", "refund"))
        .limit(3_u64)
        .send()
        .await
    {
        Ok(response) => println!("query before any write: {} hits", response.rows.len()),
        Err(error) => println!(
            "query before any write: status {:?}: {error}",
            error.status()
        ),
    }

    write(
        help,
        vec![
            article(
                "refunds",
                "Refunds go back to the original card within 5 to 10 business days.",
                "Refund policy",
                Some("https://help.acme.example/refunds"),
                "policy",
            ),
            article(
                "shipping",
                "Orders ship within two business days. Express shipping arrives the next day.",
                "Shipping times",
                Some("https://help.acme.example/shipping"),
                "policy",
            ),
            // No url, so a citation of it uses a turbopuffer:// source.
            article(
                "a-4471",
                "The A-4471 bracket fits 2x4 studs and holds 40 kg.",
                "A-4471 bracket",
                None,
                "product",
            ),
        ],
    )
    .await?;
    // A second write resends the same schema, which turbopuffer must accept.
    write(
        help,
        vec![
            article(
                "closed-card",
                "If the original card is closed, the refund goes to the bank that issued it.",
                "Refunds to a closed card",
                Some("https://help.acme.example/closed-card"),
                "policy",
            ),
            article(
                "b-120",
                "The B-120 shelf is 120 cm wide and needs two A-4471 brackets.",
                "B-120 shelf",
                None,
                "product",
            ),
        ],
    )
    .await?;

    // Hybrid: a vector and a BM25 subquery fused with RRF, as the search tool sends it.
    let question = "can I get my money back?";
    let hybrid = help
        .multi_query([
            turbopuffer::types::Query::new()
                .rank_by(RankBy::ann("text", Expr::embed(question)))
                .limit(3_u64)
                .exclude_attributes(["embed_text"]),
            turbopuffer::types::Query::new()
                .rank_by(RankBy::bm25("text", question))
                .limit(3_u64)
                .exclude_attributes(["embed_text"]),
        ])
        .rerank_by(RerankBy::rrf())
        .limit(3_u64)
        .send()
        .await?;
    if let Some(result) = hybrid.results.first() {
        print_rows("hybrid", &result.rows);
    }
    let keyword = help
        .query()
        .rank_by(RankBy::bm25("text", "A-4471"))
        .limit(10_u64)
        .exclude_attributes(["embed_text"])
        .send()
        .await?;
    print_result("keyword", &keyword);
    let filtered = help
        .query()
        .rank_by(RankBy::ann("text", Expr::embed("how long does it take")))
        .filters(Filter::eq("kind", "policy"))
        .limit(10_u64)
        .exclude_attributes(["embed_text"])
        .send()
        .await?;
    print_result("vector, policies only", &filtered);

    if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|key| !key.trim().is_empty()) {
        ask_claude(help).await?;
    }
    Ok(())
}

fn print_result(label: &str, result: &QueryResult) {
    print_rows(label, &result.rows);
    println!(
        "  ({} embedding tokens)",
        result.performance.embedding_tokens.unwrap_or(0)
    );
}

fn print_rows(label: &str, rows: &[Row]) {
    println!("\n{label}:");
    for row in rows {
        println!(
            "  {} {:.4} {}",
            row.id,
            row.dist().unwrap_or_default(),
            row.get("text").and_then(|text| text.as_str()).unwrap_or("")
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
