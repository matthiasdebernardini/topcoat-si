//! Queries and their results.

use serde::Deserialize;
use serde_json::{Map, Value};

use super::document::DocumentId;
use super::filter::Filter;
use crate::error::{Error, Result};

const DEFAULT_LIMIT: u32 = 10;
const MAX_LIMIT: u32 = 10_000;
// turbopuffer's cap on a full-text query; it applies to the BM25 subquery only.
const MAX_KEYWORD_QUERY_BYTES: usize = 8_192;

/// How a query ranks documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SearchMode {
    /// Vector and keyword results fused server-side with reciprocal rank fusion: matches both
    /// meaning and exact words.
    #[default]
    Hybrid,
    /// Nearest neighbors of the query's embedding: matches meaning and paraphrase.
    Vector,
    /// BM25 full-text ranking: matches exact words, names, codes and ids.
    Keyword,
}

/// A search over one namespace's `text`, for [`Namespace::query`](super::Namespace::query).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub(super) text: String,
    pub(super) mode: SearchMode,
    pub(super) limit: u32,
    pub(super) filter: Option<Filter>,
}

impl Query {
    /// A hybrid query for `text` returning up to 10 hits.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            mode: SearchMode::default(),
            limit: DEFAULT_LIMIT,
            filter: None,
        }
    }

    /// Set how documents are ranked.
    #[must_use]
    pub fn with_mode(mut self, mode: SearchMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the most hits returned, 1 to 10,000.
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// Only return documents matching `filter`.
    #[must_use]
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Checks what turbopuffer would otherwise reject with a 400, and empty filter groups.
    pub(super) fn validate(&self) -> Result<()> {
        let invalid =
            |message: &str, param: &str| Err(Error::validation(message, Some(param.to_string())));
        if self.text.trim().is_empty() {
            return invalid("query text must not be empty", "query");
        }
        if self.mode != SearchMode::Vector && self.text.len() > MAX_KEYWORD_QUERY_BYTES {
            return invalid(
                "query text must be at most 8192 bytes for hybrid and keyword search",
                "query",
            );
        }
        if !(1..=MAX_LIMIT).contains(&self.limit) {
            return invalid("limit must be between 1 and 10000", "limit");
        }
        match &self.filter {
            Some(filter) => filter.validate(),
            None => Ok(()),
        }
    }
}

/// The hits of one query, best first.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct QueryResponse {
    /// Matching documents, best first.
    pub hits: Vec<Hit>,
    /// Tokens embedded for the query text; 0 for keyword search.
    pub embedding_tokens: u64,
}

/// One matching document.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Hit {
    /// The document's id.
    pub id: DocumentId,
    /// The document's text.
    pub text: Option<String>,
    /// Higher is better in every mode; not comparable across modes. Hybrid: the RRF
    /// (reciprocal rank fusion) score. Vector: cosine similarity, `1 - cosine distance`.
    /// Keyword: the BM25 score.
    pub score: f64,
    /// The document's other attributes.
    pub attributes: Map<String, Value>,
}
