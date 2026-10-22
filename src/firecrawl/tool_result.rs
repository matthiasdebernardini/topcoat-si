//! Tool result blocks shared by the Firecrawl tools.

use crate::http;
use crate::types::{CitationsConfig, Content, SearchResultBlock, TextBlock};

pub(super) fn search_result(
    url: String,
    title: Option<String>,
    mut content: Vec<TextBlock>,
) -> Content {
    let title = title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| url.clone());
    // The API rejects a search result without content.
    if content.is_empty() {
        content.push(TextBlock::new(title.clone()));
    }
    Content::SearchResult(
        SearchResultBlock::new(url, title, content).with_citations(CitationsConfig::enabled()),
    )
}

pub(super) fn push_text(content: &mut Vec<TextBlock>, text: Option<String>) {
    if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
        content.push(TextBlock::new(text));
    }
}

pub(super) fn truncate(text: &str, max_chars: usize) -> String {
    http::truncate_chars(text, max_chars, " [truncated]")
}
