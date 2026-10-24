//! A read-only client-side tool that lets Claude search one namespace.

use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use turbopuffer::Namespace;
use turbopuffer::types::{Expr, Filter, Id, Query, RankBy, RerankBy, Row};

use crate::ClientTool;
use crate::error::{Error, ErrorKind, Result};
use crate::http;
use crate::types::{
    CitationsConfig, Content, SearchResultBlock, TextBlock, ToolParam, ToolResultBlock,
    ToolUnionParam, ToolUseBlock,
};

// Caps results per call, bounding context size.
const MAX_MODEL_LIMIT: u32 = 10;
const DEFAULT_LIMIT: u32 = 5;
const DEFAULT_MAX_TEXT_CHARS: usize = 4000;
const MAX_NAME_CHARS: usize = 64;
// Keeps the stored vectors out of every response.
const EXCLUDED_ATTRIBUTES: [&str; 1] = ["embed_text"];

/// How the tool ranks documents.
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

/// One matching document, as the tool and the code-mode pack use it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hit {
    pub(crate) id: Id,
    pub(crate) text: Option<String>,
    /// Higher is better in every mode; not comparable across modes. Hybrid: the RRF score.
    /// Vector: cosine similarity, `1 - cosine distance`. Keyword: the BM25 score.
    pub(crate) score: f64,
    pub(crate) attributes: Map<String, Value>,
}

/// A turbopuffer namespace exposed to Claude as a client-side search tool.
///
/// Read-only by design: Claude can search, never write. Its input has no filter field, so the
/// filter set here applies to every call and cannot be removed or widened. Each hit becomes a
/// citable search result. Bad input, 400, and 408, 409, 429, 5xx, timeouts and connection
/// failures that outlast the client's retries come back as `is_error` results; 401, 403 and
/// other configuration problems are returned as `Err`. A namespace that was never written
/// answers with no documents and a `tracing` warning, so a new tenant works and a typo or wrong
/// region shows in the logs.
#[derive(Debug, Clone)]
pub struct TurbopufferSearch {
    namespace: Namespace,
    name: String,
    corpus: String,
    default_limit: u32,
    filter: Option<Filter>,
    max_text_chars: usize,
}

impl TurbopufferSearch {
    /// A tool named `name` that searches `namespace`, described to Claude as searching
    /// `corpus`, such as "Acme's help center articles".
    ///
    /// `name` must match `^[a-z][a-z0-9_]{0,63}$`, so it is both a valid tool name and a Python
    /// identifier in code mode. Avoid Python builtins and keywords (`list`, `print`, `if`);
    /// `help` is taken by code mode.
    pub fn try_new(
        namespace: Namespace,
        name: impl Into<String>,
        corpus: impl Into<String>,
    ) -> Result<Self> {
        let name: String = name.into();
        let mut chars = name.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && name.len() <= MAX_NAME_CHARS;
        if !valid {
            return Err(Error::validation(
                "tool name must be a lowercase letter followed by up to 63 lowercase letters, \
                 digits and underscores",
                Some("name".to_string()),
            ));
        }
        Ok(Self {
            namespace,
            name,
            corpus: corpus.into(),
            default_limit: DEFAULT_LIMIT,
            filter: None,
            max_text_chars: DEFAULT_MAX_TEXT_CHARS,
        })
    }

    /// Hits returned when Claude does not ask for a number. Clamped to 1 to 10; defaults to 5.
    #[must_use]
    pub fn with_default_limit(mut self, limit: u32) -> Self {
        self.default_limit = limit.clamp(1, MAX_MODEL_LIMIT);
        self
    }

    /// Only ever return documents matching `filter`.
    ///
    /// A filter with an empty `And` or `Or` anywhere is rejected here, since it would match
    /// everything or fail every search.
    pub fn with_filter(mut self, filter: Filter) -> Result<Self> {
        validate_filter(&filter)?;
        self.filter = Some(filter);
        Ok(self)
    }

    /// Characters of document text kept per hit. Defaults to 4000.
    #[must_use]
    pub fn with_max_text_chars(mut self, max_text_chars: usize) -> Self {
        self.max_text_chars = max_text_chars;
        self
    }

    #[cfg(feature = "code-mode")]
    pub(crate) fn corpus(&self) -> &str {
        &self.corpus
    }

    #[cfg(feature = "code-mode")]
    pub(crate) fn default_limit(&self) -> u32 {
        self.default_limit
    }

    /// Runs one search with the app's filter, the limit clamped to 1 to 10 and each text cut to
    /// the tool's limit. The inner `Err` is a message the model can act on; `Err` is fatal. A
    /// namespace that does not exist yet has no hits.
    pub(crate) async fn search_hits(
        &self,
        query: String,
        limit: Option<u32>,
        mode: SearchMode,
    ) -> Result<std::result::Result<Vec<Hit>, String>> {
        if query.trim().is_empty() {
            return Ok(Err(Error::validation(
                "query text must not be empty",
                Some("query".to_string()),
            )
            .to_string()));
        }
        let limit = u64::from(
            limit
                .unwrap_or(self.default_limit)
                .clamp(1, MAX_MODEL_LIMIT),
        );
        let rows = match mode {
            SearchMode::Hybrid => self.hybrid(&query, limit).await,
            SearchMode::Vector => self.single(vector(&query), limit).await,
            SearchMode::Keyword => self.single(keyword(&query), limit).await,
        };
        let hits = rows.and_then(|rows| {
            rows.into_iter()
                .map(|row| hit(row, mode, self.max_text_chars))
                .collect::<Result<Vec<Hit>>>()
        });
        match hits {
            Ok(hits) => Ok(Ok(hits)),
            Err(error) if error.status() == Some(404) => {
                tracing::warn!(
                    namespace = self.namespace.id(),
                    "turbopuffer namespace not found; answering with no documents"
                );
                Ok(Ok(Vec::new()))
            }
            Err(error) if is_fatal(&error) => Err(error),
            Err(error) => Ok(Err(format!("Search failed: {error}"))),
        }
    }

    /// A vector and a keyword subquery fused with RRF.
    async fn hybrid(&self, query: &str, limit: u64) -> Result<Vec<Row>> {
        let subquery = |rank_by: RankBy| {
            let subquery = Query::new()
                .rank_by(rank_by)
                .limit(limit)
                .exclude_attributes(EXCLUDED_ATTRIBUTES);
            match &self.filter {
                Some(filter) => subquery.filters(filter.clone()),
                None => subquery,
            }
        };
        let response = self
            .namespace
            .multi_query([subquery(vector(query)), subquery(keyword(query))])
            .rerank_by(RerankBy::rrf())
            .limit(limit)
            .send()
            .await?;
        // With rerank_by the server answers with one fused result.
        response
            .results
            .into_iter()
            .next()
            .map(|result| result.rows)
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Decode,
                    "turbopuffer hybrid response has no results",
                )
            })
    }

    async fn single(&self, rank_by: RankBy, limit: u64) -> Result<Vec<Row>> {
        let request = self
            .namespace
            .query()
            .rank_by(rank_by)
            .limit(limit)
            .exclude_attributes(EXCLUDED_ATTRIBUTES);
        let request = match &self.filter {
            Some(filter) => request.filters(filter.clone()),
            None => request,
        };
        Ok(request.send().await?.rows)
    }

    async fn run(&self, tool_use: &ToolUseBlock) -> Result<ToolResultBlock> {
        let id = tool_use.id.clone();
        let input = match serde_json::from_value::<ToolInput>(tool_use.input.clone()) {
            Ok(input) => input,
            Err(error) => {
                return Ok(error_result(
                    id,
                    format!(
                        "Invalid {} input: {error}. Send an object with a \"query\" string and \
                         only the optional fields limit and mode.",
                        self.name
                    ),
                ));
            }
        };
        let mode = input.mode.unwrap_or_default();
        Ok(
            match self
                .search_hits(input.query.clone(), input.limit, mode)
                .await?
            {
                Ok(hits) if hits.is_empty() => ToolResultBlock::new(id)
                    .with_string_content(format!("No documents match {}", input.query)),
                Ok(hits) => ToolResultBlock::new(id).with_array_content(
                    hits.into_iter()
                        .map(|hit| self.search_result(hit))
                        .collect(),
                ),
                Err(message) => error_result(id, message),
            },
        )
    }

    /// One hit as a citable search result: source from the `url` attribute, title from `title`,
    /// then the text and the remaining attributes.
    fn search_result(&self, hit: Hit) -> Content {
        let mut attributes = hit.attributes;
        let source = match attributes.remove("url") {
            Some(Value::String(url)) if !url.trim().is_empty() => url,
            _ => format!("turbopuffer://{}/{}", self.namespace.id(), hit.id),
        };
        let title = match attributes.remove("title") {
            Some(Value::String(title)) if !title.trim().is_empty() => title,
            _ => hit.id.to_string(),
        };
        let mut content = Vec::new();
        if let Some(text) = hit.text.filter(|text| !text.trim().is_empty()) {
            content.push(TextBlock::new(text));
        }
        if !attributes.is_empty() {
            content.push(TextBlock::new(format!(
                "Attributes: {}",
                Value::Object(attributes)
            )));
        }
        // The API rejects a search result without content.
        if content.is_empty() {
            content.push(TextBlock::new(title.clone()));
        }
        Content::SearchResult(
            SearchResultBlock::new(source, title, content)
                .with_citations(CitationsConfig::enabled()),
        )
    }
}

impl ClientTool for TurbopufferSearch {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolUnionParam {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to search for."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_MODEL_LIMIT,
                    "description": "Number of results."
                },
                "mode": {
                    "type": "string",
                    "enum": ["hybrid", "vector", "keyword"],
                    "description": "How to match. Defaults to hybrid."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        });
        let description = format!(
            "Search {}. Hybrid (the default) matches both meaning and exact words; use keyword \
             for exact names, codes and ids, vector for loose paraphrase. Each result carries \
             the document's text (cut after {} characters) and its attributes. At most \
             {MAX_MODEL_LIMIT} results per call.",
            self.corpus, self.max_text_chars
        );
        ToolUnionParam::CustomTool(
            ToolParam::new(self.name.clone(), schema).with_description(description),
        )
    }

    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>> {
        Box::pin(self.run(tool_use))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolInput {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    mode: Option<SearchMode>,
}

// The query embeds through the model set on the `text` attribute when it was written.
fn vector(query: &str) -> RankBy {
    RankBy::ann("text", Expr::embed(query))
}

fn keyword(query: &str) -> RankBy {
    RankBy::bm25("text", query).into()
}

/// One response row as a hit: `$dist` and `text` taken out, `embed_text` dropped and the text
/// cut to `max_text_chars`.
fn hit(row: Row, mode: SearchMode, max_text_chars: usize) -> Result<Hit> {
    let dist = row
        .dist()
        .ok_or_else(|| Error::new(ErrorKind::Decode, "turbopuffer row has no $dist score"))?;
    let mut attributes = row.extra;
    attributes.remove("$dist");
    attributes.remove("embed_text");
    let text = match attributes.remove("text") {
        Some(Value::String(text)) => {
            Some(http::truncate_chars(&text, max_text_chars, " [truncated]"))
        }
        _ => None,
    };
    let score = match mode {
        SearchMode::Vector => 1.0 - dist,
        SearchMode::Hybrid | SearchMode::Keyword => dist,
    };
    Ok(Hit {
        id: row.id,
        text,
        score,
        attributes: attributes.into_iter().collect(),
    })
}

/// Rejects an empty `And` or `Or` anywhere in `filter`.
fn validate_filter(filter: &Filter) -> Result<()> {
    match filter {
        Filter::And { filters } | Filter::Or { filters } if filters.is_empty() => {
            Err(Error::validation(
                "a Filter::and or Filter::or must hold at least one filter",
                Some("filter".to_string()),
            ))
        }
        Filter::And { filters } | Filter::Or { filters } => {
            filters.iter().try_for_each(validate_filter)
        }
        Filter::Not { filter } => validate_filter(filter),
        _ => Ok(()),
    }
}

// Only these failures are the developer's to fix (401, 403, other unexpected statuses, and a
// request the SDK could not build); everything else goes back to the model. 404 is handled
// before this is asked, and a blank query never reaches the SDK.
fn is_fatal(error: &Error) -> bool {
    if matches!(error.kind(), ErrorKind::Validation { .. }) {
        return true;
    }
    match error.status() {
        // 409: the query depends on an index still being built. The SDK has already retried
        // it, and a later call can still work.
        Some(400 | 408 | 409 | 429 | 500..=599) => false,
        Some(_) => true,
        None => false,
    }
}

fn error_result(id: String, message: String) -> ToolResultBlock {
    ToolResultBlock::new(id)
        .with_string_content(message)
        .with_error(true)
}
