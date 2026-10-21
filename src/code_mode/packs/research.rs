//! The `research` pack.

use crate::code_mode::{Example, Pack};

/// `search_and_rank`, `scrape_many` and `dedupe_urls`. Requires the `firecrawl` and `jev`
/// packs.
#[must_use]
pub fn research() -> Pack {
    Pack::new("research", "search, read and rank pages")
        .with_source(include_str!("research.py"))
        .requires(["firecrawl", "jev"])
        .with_python_fn(
            "search_and_rank",
            "async def search_and_rank(*, query: str, task: str, limit: int = 5, keep: int = 3, \
             min_confidence: float = 0.7) -> list[dict]",
            "search with scrape, rate each page against task, return the top keep with url, \
             title, level (None when unsure) and confidence; a page that could not be rated has \
             level and confidence None and an \"error\" key, sorts last, and should be filtered \
             out",
        )
        .with_python_fn(
            "scrape_many",
            "async def scrape_many(*, urls: list[str]) -> list[dict]",
            "scrape concurrently, one dict per URL; a failure is {url, error} instead of raising",
        )
        .with_python_fn(
            "dedupe_urls",
            "def dedupe_urls(urls: list[str]) -> list[str]",
            "drop repeats ignoring scheme, host case, www., trailing slash and fragment, keeping \
             order",
        )
        .with_example(
            "search_and_rank",
            Example {
                code: r#"[(r["url"], r["level"], r.get("error")) for r in await search_and_rank(query="rust", task="Rust async guides")]"#,
                expect: "=> [('https://example.com/a', 'directly useful', None), ('https://example.com/b', None, None), ('https://example.com/c', None, 'jev failed')]",
            },
        )
        .with_example(
            "scrape_many",
            Example {
                code: r#"await scrape_many(urls=["https://example.com/a", "https://example.com/missing"])"#,
                expect: "=> [{'url': 'https://example.com/a', 'title': 'Page A'}, {'url': 'https://example.com/missing', 'error': 'not found'}]",
            },
        )
        .with_example(
            "dedupe_urls",
            Example {
                code: r#"dedupe_urls(["https://www.example.com/a/", "http://Example.com/a#top", "https://example.com/b"])"#,
                expect: "=> ['https://www.example.com/a/', 'https://example.com/b']",
            },
        )
}
