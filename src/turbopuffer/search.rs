//! A read-only client-side tool that lets Claude search one namespace.

use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::filter::Filter;
use super::namespace::Namespace;
use super::query::{Hit, Query, SearchMode};
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
    /// A filter with an empty `all` or `any` anywhere is rejected here, since every search would
    /// fail on it.
    pub fn with_filter(mut self, filter: Filter) -> Result<Self> {
        filter.validate()?;
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
        let limit = limit
            .unwrap_or(self.default_limit)
            .clamp(1, MAX_MODEL_LIMIT);
        let mut request = Query::new(query).with_mode(mode).with_limit(limit);
        if let Some(filter) = &self.filter {
            request = request.with_filter(filter.clone());
        }
        match self.namespace.query(&request).await {
            Ok(response) => Ok(Ok(response
                .hits
                .into_iter()
                .map(|mut hit| {
                    hit.text = hit.text.map(|text| {
                        http::truncate_chars(&text, self.max_text_chars, " [truncated]")
                    });
                    hit
                })
                .collect())),
            Err(error) if error.status() == Some(404) => {
                tracing::warn!(
                    namespace = self.namespace.name(),
                    "turbopuffer namespace not found; answering with no documents"
                );
                Ok(Ok(Vec::new()))
            }
            Err(error) if is_fatal(&error) => Err(error),
            Err(error) if matches!(error.kind(), ErrorKind::Validation { .. }) => {
                Ok(Err(error.to_string()))
            }
            Err(error) => Ok(Err(format!("Search failed: {error}"))),
        }
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
            _ => format!("turbopuffer://{}/{}", self.namespace.name(), hit.id),
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

// Only these failures are the developer's to fix (401, 403 and other unexpected statuses);
// everything else goes back to the model. 404 is handled before this is asked.
fn is_fatal(error: &Error) -> bool {
    match error.status() {
        // 409: the query depends on an index still being built. The client has already retried
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
