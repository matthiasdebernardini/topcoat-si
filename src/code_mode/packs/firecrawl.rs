//! The `firecrawl` pack: `search` and `scrape` through the app's Firecrawl tools.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

use super::to_json;
use crate::code_mode::{Example, HostError, Pack, PyError, host_fn};
use crate::error::Result;
use crate::firecrawl::{FirecrawlFetchPage, FirecrawlWebSearch};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    scrape: bool,
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrapeArgs {
    url: String,
}

/// Firecrawl credits used by one conversation, shared by `search` and `scrape`.
struct Credits {
    used: AtomicU64,
    max: u32,
}

impl Credits {
    fn check(&self) -> std::result::Result<(), HostError> {
        if self.used.load(Ordering::SeqCst) >= u64::from(self.max) {
            return Err(HostError::Raise {
                kind: PyError::RuntimeError,
                message: format!(
                    "Firecrawl credit budget of {} used up; finish with what you have",
                    self.max
                ),
            });
        }
        Ok(())
    }

    fn add(&self, credits: u64) {
        self.used.fetch_add(credits, Ordering::SeqCst);
    }
}

/// `search` and `scrape`, built from the app's tools so their domain lists, the 10-result cap,
/// scrape options and content cut apply exactly as in the tool loop.
///
/// `max_credits` is a per-conversation Firecrawl credit budget: a search counts its
/// `credits_used`, or 1 when the response reports none, and a successful scrape counts 1. Once reached, both functions raise
/// `RuntimeError`; calls already running in one `gather` finish, so the total can overshoot by
/// those. Build a new pack for each conversation.
pub fn firecrawl(search: FirecrawlWebSearch, fetch: FirecrawlFetchPage, max_credits: u32) -> Pack {
    let credits = Arc::new(Credits {
        used: AtomicU64::new(0),
        max: max_credits,
    });
    let search_credits = credits.clone();
    let search = Arc::new(search);
    let fetch = Arc::new(fetch);
    Pack::new("firecrawl", "web search and page scraping")
        .with_host(host_fn(
            "search",
            "search(*, query: str, limit: int = 5, scrape: bool = False, include_domains: \
             list[str] | None = None, exclude_domains: list[str] | None = None) -> list[dict]",
            "search the web; results have url, title, description, status and, with \
             scrape=True, markdown",
            move |args: SearchArgs| {
                let search = search.clone();
                let credits = search_credits.clone();
                async move {
                    credits.check()?;
                    let found = search
                        .search_values(
                            args.query,
                            args.limit,
                            args.scrape,
                            args.include_domains,
                            args.exclude_domains,
                        )
                        .await;
                    let (results, used) = checked(found)?;
                    // A search that reports no credits still cost at least one.
                    credits.add(used.unwrap_or(1));
                    to_json(results)
                }
            },
        ))
        .with_host(host_fn(
            "scrape",
            "scrape(*, url: str) -> dict",
            "one page as markdown, with its final url, status and title",
            move |args: ScrapeArgs| {
                let fetch = fetch.clone();
                let credits = credits.clone();
                async move {
                    credits.check()?;
                    let page = checked(fetch.page_value(&args.url).await)?;
                    credits.add(1);
                    to_json(page)
                }
            },
        ))
        .with_example(
            "search",
            Example {
                code: r#"[hit["url"] for hit in await search(query="rust async", limit=2)]"#,
                expect: "=> ['https://example.com/a', 'https://example.com/b']",
            },
        )
        .with_example(
            "scrape",
            Example {
                code: r#"(await scrape(url="https://example.com/a"))["title"]"#,
                expect: "=> 'Page A'",
            },
        )
}

/// Maps a tool's outcome: a fatal error ends the run, a message raises `RuntimeError`.
fn checked<T>(
    outcome: Result<std::result::Result<T, String>>,
) -> std::result::Result<T, HostError> {
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(HostError::Raise {
            kind: PyError::RuntimeError,
            message,
        }),
        Err(error) => Err(HostError::Fatal(error)),
    }
}
