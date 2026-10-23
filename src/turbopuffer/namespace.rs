//! One namespace: writes, deletes and queries, and their wire formats.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::client::{Request, TurbopufferClient};
use super::document::{Document, DocumentId};
use super::query::{Hit, Query, QueryResponse, SearchMode};
use crate::error::{Error, ErrorKind, Result};

const DISTANCE_METRIC: &str = "cosine_distance";
// turbopuffer's cap on rows per write when an attribute is embedded.
const MAX_BATCH_ROWS: usize = 256;
const MAX_ID_BYTES: usize = 64;
const MAX_ATTRIBUTE_BYTES: usize = 128;
// `text` and `id` are set from the document; `embed_text` is the computed vector, and an
// attribute named `vector` would be inferred as a vector column.
const RESERVED_ATTRIBUTES: [&str; 4] = ["id", "text", "embed_text", "vector"];
// Keeps the stored vectors out of every response.
const EXCLUDED_ATTRIBUTES: [&str; 1] = ["embed_text"];

/// A turbopuffer namespace whose `text` attribute is embedded with one model.
///
/// Made by [`TurbopufferClient::namespace`]. The first write creates the namespace.
#[derive(Clone, Debug)]
pub struct Namespace {
    client: TurbopufferClient,
    name: String,
    model: String,
}

/// What a [`Namespace::upsert`] wrote, summed over its batches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct UpsertSummary {
    /// Rows written.
    pub rows: u64,
    /// Tokens embedded, which the model's provider bills for.
    pub embedding_tokens: u64,
}

impl Namespace {
    pub(super) fn new(client: TurbopufferClient, name: String, model: String) -> Self {
        Self {
            client,
            name,
            model,
        }
    }

    /// The namespace name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The embedding model, such as `voyage/voyage-4-lite`.
    pub fn model(&self) -> &str {
        &self.model
    }

    fn path(&self) -> String {
        format!("/v2/namespaces/{}", self.name)
    }

    /// Write documents, replacing any with the same id.
    ///
    /// Every document is checked before anything is sent: string ids are at most 64 bytes, no
    /// id appears twice, and attribute names are 1 to 128 bytes, do not start with `$` and are
    /// not `id`, `text`, `embed_text` or `vector`. Documents go out in batches of 256, one after
    /// another. When a batch fails its error is returned and earlier batches stay written;
    /// since upserts overwrite by id, retrying the whole call is safe.
    ///
    /// Every write sends the namespace's schema, so the first creates the namespace and a write
    /// with another model fails instead of mixing vectors.
    pub async fn upsert(
        &self,
        documents: impl IntoIterator<Item = Document>,
    ) -> Result<UpsertSummary> {
        let documents: Vec<Document> = documents.into_iter().collect();
        validate_documents(&documents)?;
        let schema = json!({
            "text": {"type": "string", "embed": self.model, "full_text_search": true}
        });
        let mut summary = UpsertSummary::default();
        for batch in documents.chunks(MAX_BATCH_ROWS) {
            let rows: Vec<WireRow<'_>> = batch
                .iter()
                .map(|document| WireRow {
                    id: &document.id,
                    text: &document.text,
                    attributes: &document.attributes,
                })
                .collect();
            let body = WireWrite {
                upsert_rows: Some(rows),
                deletes: None,
                distance_metric: DISTANCE_METRIC,
                schema: Some(&schema),
            };
            let response: WireWriteResponse = self
                .client
                .post(&self.path(), &body, Request::Write)
                .await?;
            summary.rows += response.rows_upserted;
            summary.embedding_tokens += response.performance.embedding_tokens;
        }
        Ok(summary)
    }

    /// Delete documents by id and return how many were deleted. Empty input sends nothing.
    pub async fn delete<I>(&self, ids: I) -> Result<u64>
    where
        I: IntoIterator,
        I::Item: Into<DocumentId>,
    {
        let ids: Vec<DocumentId> = ids.into_iter().map(Into::into).collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let body = WireWrite {
            upsert_rows: None,
            deletes: Some(&ids),
            distance_metric: DISTANCE_METRIC,
            schema: None,
        };
        let response: WireWriteResponse = self
            .client
            .post(&self.path(), &body, Request::Write)
            .await?;
        Ok(response.rows_deleted)
    }

    /// Delete the whole namespace and every document in it. This cannot be undone.
    ///
    /// A namespace that does not exist counts as deleted.
    pub async fn delete_namespace(&self) -> Result<()> {
        match self.client.delete(&self.path()).await {
            Err(error) if error.status() == Some(404) => Ok(()),
            outcome => outcome,
        }
    }

    /// Search the namespace.
    ///
    /// The query is checked before anything is sent: non-blank text, at most 8,192 bytes for
    /// hybrid and keyword search, a limit of 1 to 10,000, and no empty `all` or `any` in the
    /// filter. A namespace that was never written is an `Err` with status 404.
    pub async fn query(&self, query: &Query) -> Result<QueryResponse> {
        query.validate()?;
        let filters = query.filter.as_ref().map(|filter| filter.to_wire());
        let subquery = |rank_by: Value| WireSubquery {
            rank_by,
            limit: query.limit,
            filters: filters.clone(),
            exclude_attributes: EXCLUDED_ATTRIBUTES,
        };
        let vector = || subquery(json!(["text", "ANN", ["Embed", query.text]]));
        let keyword = || subquery(json!(["text", "BM25", query.text]));
        let path = format!("{}/query", self.path());
        let (rows, embedding_tokens) = match query.mode {
            SearchMode::Hybrid => {
                let body = WireHybrid {
                    queries: [vector(), keyword()],
                    rerank_by: ["RRF"],
                    limit: query.limit,
                };
                let response: WireQueryResponse =
                    self.client.post(&path, &body, Request::Read).await?;
                let results = response.results.unwrap_or_default();
                // The docs do not say whether a multi-query reports its embedding tokens at the
                // top level or per subquery, so fall back to the per-subquery counts.
                let embedding_tokens = match response.performance.embedding_tokens {
                    0 => results
                        .iter()
                        .map(|result| result.performance.embedding_tokens)
                        .sum(),
                    tokens => tokens,
                };
                let rows = results
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::Decode,
                            "turbopuffer hybrid response has no results",
                        )
                    })?
                    .rows;
                (rows, embedding_tokens)
            }
            SearchMode::Vector | SearchMode::Keyword => {
                let body = if query.mode == SearchMode::Vector {
                    vector()
                } else {
                    keyword()
                };
                let response: WireQueryResponse =
                    self.client.post(&path, &body, Request::Read).await?;
                (response.rows, response.performance.embedding_tokens)
            }
        };
        let hits: Result<Vec<Hit>> = rows.into_iter().map(|row| hit(row, query.mode)).collect();
        Ok(QueryResponse {
            hits: hits?,
            embedding_tokens,
        })
    }
}

fn validate_documents(documents: &[Document]) -> Result<()> {
    let invalid = |message: String| Err(Error::validation(message, Some("documents".to_string())));
    let mut seen = HashSet::new();
    for document in documents {
        if let DocumentId::Text(id) = &document.id
            && id.len() > MAX_ID_BYTES
        {
            return invalid(format!(
                "document id {id:?} is longer than {MAX_ID_BYTES} bytes"
            ));
        }
        if !seen.insert(&document.id) {
            return invalid(format!("document id {} appears twice", document.id));
        }
        for name in document.attributes.keys() {
            if name.is_empty() || name.len() > MAX_ATTRIBUTE_BYTES {
                return invalid(format!(
                    "attribute name {name:?} must be 1 to {MAX_ATTRIBUTE_BYTES} bytes"
                ));
            }
            if name.starts_with('$') {
                return invalid(format!("attribute name {name:?} must not start with $"));
            }
            if RESERVED_ATTRIBUTES.contains(&name.as_str()) {
                return invalid(format!("attribute name {name:?} is reserved"));
            }
        }
    }
    Ok(())
}

/// One response row as a hit: `id`, `$dist` and `text` taken out, `embed_text` dropped.
fn hit(mut row: Map<String, Value>, mode: SearchMode) -> Result<Hit> {
    let malformed = |message: &str| Error::new(ErrorKind::Decode, message);
    let id = row
        .remove("id")
        .and_then(|id| serde_json::from_value::<DocumentId>(id).ok())
        .ok_or_else(|| malformed("turbopuffer row has no valid id"))?;
    let dist = row
        .remove("$dist")
        .and_then(|dist| dist.as_f64())
        .ok_or_else(|| malformed("turbopuffer row has no $dist score"))?;
    let text = match row.remove("text") {
        Some(Value::String(text)) => Some(text),
        _ => None,
    };
    row.remove("embed_text");
    let score = match mode {
        SearchMode::Vector => 1.0 - dist,
        SearchMode::Hybrid | SearchMode::Keyword => dist,
    };
    Ok(Hit {
        id,
        text,
        score,
        attributes: row,
    })
}

#[derive(Serialize)]
struct WireWrite<'body> {
    #[serde(skip_serializing_if = "Option::is_none")]
    upsert_rows: Option<Vec<WireRow<'body>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletes: Option<&'body [DocumentId]>,
    distance_metric: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<&'body Value>,
}

#[derive(Serialize)]
struct WireRow<'body> {
    id: &'body DocumentId,
    text: &'body str,
    #[serde(flatten)]
    attributes: &'body Map<String, Value>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WireWriteResponse {
    rows_upserted: u64,
    rows_deleted: u64,
    performance: WirePerformance,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WirePerformance {
    embedding_tokens: u64,
}

#[derive(Serialize)]
struct WireSubquery {
    rank_by: Value,
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<Value>,
    exclude_attributes: [&'static str; 1],
}

#[derive(Serialize)]
struct WireHybrid {
    queries: [WireSubquery; 2],
    rerank_by: [&'static str; 1],
    limit: u32,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WireQueryResponse {
    rows: Vec<Map<String, Value>>,
    results: Option<Vec<WireResult>>,
    performance: WirePerformance,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WireResult {
    rows: Vec<Map<String, Value>>,
    performance: WirePerformance,
}
