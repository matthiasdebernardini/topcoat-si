//! The built-in packs.
//!
//! Packs holding host functions keep per-conversation state (the Firecrawl credit budget), so
//! build them for each [`CodeMode`](super::CodeMode).

mod firecrawl;
mod jev;

pub use firecrawl::firecrawl;
pub use jev::jev;

use serde::Serialize;
use serde_json::Value;

use super::{Example, HostError, Pack};
use crate::error::Error;

/// `search_and_rank`, `scrape_many` and `dedupe_urls`. Requires the `firecrawl` and `jev`
/// packs.
pub fn research() -> Pack {
    Pack::new("research", "search, read and rank pages")
        .with_source(include_str!("research.py"))
        .requires(&["firecrawl", "jev"])
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

/// `classify_all`, `score_all` and `group_by`. Requires the `jev` pack.
pub fn triage() -> Pack {
    Pack::new("triage", "classify and score many items at once")
        .with_source(include_str!("triage.py"))
        .requires(&["jev"])
        .with_python_fn(
            "classify_all",
            "async def classify_all(*, items: list, question: str, options: list[str], \
             min_confidence: float = 0.7) -> list[dict]",
            "choose per item concurrently; items (dicts, or strings as {text}) come back with \
             answer and confidence added; an item that could not be judged has answer and \
             confidence None and an \"error\" key, so filter on it",
        )
        .with_python_fn(
            "score_all",
            "async def score_all(*, items: list, question: str, levels: list[str], \
             min_confidence: float = 0.7) -> list[dict]",
            "the same with rate; levels lowest first; failed items carry an \"error\" key",
        )
        .with_python_fn(
            "group_by",
            "def group_by(rows: list[dict], key: str) -> dict",
            "rows grouped by their key value, None under \"unsure\"",
        )
        .with_example(
            "classify_all",
            Example {
                code: r#"await classify_all(items=["The app crashes", "broken"], question="Which team?", options=["billing", "technical"])"#,
                expect: "=> [{'text': 'The app crashes', 'answer': 'technical', 'confidence': 0.9}, {'text': 'broken', 'answer': None, 'confidence': None, 'error': 'jev failed'}]",
            },
        )
        .with_example(
            "score_all",
            Example {
                code: r#"await score_all(items=[{"id": 1, "text": "Fix it today"}, {"id": 2, "text": "broken"}], question="How urgent?", levels=["low", "high"])"#,
                expect: "=> [{'id': 1, 'text': 'Fix it today', 'answer': 'high', 'confidence': 0.9}, {'id': 2, 'text': 'broken', 'answer': None, 'confidence': None, 'error': 'jev failed'}]",
            },
        )
        .with_example(
            "group_by",
            Example {
                code: r#"group_by([{"answer": "a"}, {"answer": None}, {"answer": "a"}], "answer")"#,
                expect: "=> {'a': [{'answer': 'a'}, {'answer': 'a'}], 'unsure': [{'answer': None}]}",
            },
        )
}

/// `table`, `to_csv` and `count_by`: plain functions, no host calls.
pub fn reports() -> Pack {
    Pack::new("reports", "tables, CSV and counts from lists of dicts")
        .with_source(include_str!("reports.py"))
        .with_python_fn(
            "table",
            "def table(rows: list[dict], columns: list[str] | None = None) -> str",
            "markdown table; columns default to the first row's keys, so an \"error\" column \
             appears only when the first row has one",
        )
        .with_python_fn(
            "to_csv",
            "def to_csv(rows: list[dict], columns: list[str] | None = None) -> str",
            "CSV text; columns default to the first row's keys, as for table",
        )
        .with_python_fn(
            "count_by",
            "def count_by(rows: list[dict], key: str) -> list[dict]",
            "[{key: value, \"count\": n}] sorted by count, descending",
        )
        .with_example(
            "table",
            Example {
                code: r#"print(table([{"name": "a", "n": 1}, {"name": "b", "n": 2}]))"#,
                expect: "| name | n |\n|---|---|\n| a | 1 |\n| b | 2 |\n=> None",
            },
        )
        .with_example(
            "to_csv",
            Example {
                code: r#"to_csv([{"name": "a, b", "n": 1}])"#,
                expect: "=> 'name,n\\r\\n\"a, b\",1\\r\\n'",
            },
        )
        .with_example(
            "count_by",
            Example {
                code: r#"count_by([{"team": "a"}, {"team": "b"}, {"team": "b"}], "team")"#,
                expect: "=> [{'team': 'b', 'count': 2}, {'team': 'a', 'count': 1}]",
            },
        )
}

/// A host function's typed result as JSON.
fn to_json(value: impl Serialize) -> Result<Value, HostError> {
    serde_json::to_value(value).map_err(|error| HostError::Fatal(Error::from(error)))
}
