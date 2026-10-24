//! The `turbopuffer` pack: one read-only search function per app search tool.

use std::sync::Arc;

use ::turbopuffer::types::Id;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ClientTool;
use crate::code_mode::{HostError, Pack, PyError, host_fn, value};
use crate::turbopuffer::{SearchMode, TurbopufferSearch};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    mode: SearchMode,
}

/// One hit as the sandbox's search functions return it.
#[derive(Serialize)]
struct PackHit {
    id: Id,
    score: f64,
    text: Option<String>,
    attributes: Map<String, Value>,
}

/// One host function per search, named after the tool, so its namespace, filter, 10-result
/// cap and text cut apply exactly as in the tool loop.
///
/// Each takes `query`, `limit` and `mode` (`"hybrid"`, `"vector"` or `"keyword"`) and returns
/// hits with `id`, `score`, `text` and `attributes`; a namespace that was never written returns
/// `[]`. Two searches with the same name make `CodeMode::new` fail.
pub fn turbopuffer(searches: impl IntoIterator<Item = TurbopufferSearch>) -> Pack {
    let mut pack = Pack::new("turbopuffer", "search the app's document collections");
    for search in searches {
        let name = search.name().to_string();
        let signature = format!(
            "{name}(*, query: str, limit: int = {}, mode: str = \"hybrid\") -> list[dict]",
            search.default_limit()
        );
        let summary = format!(
            "search {}; at most 10 hits, each with id, score, text and attributes",
            search.corpus()
        );
        // Shared by every concurrent call of this function.
        let search = Arc::new(search);
        pack = pack.with_host(host_fn(
            name,
            signature,
            summary,
            move |args: SearchArgs| {
                let search = search.clone();
                async move {
                    let hits = match search.search_hits(args.query, args.limit, args.mode).await {
                        Ok(Ok(hits)) => hits,
                        Ok(Err(message)) => {
                            return Err(HostError::Raise {
                                kind: PyError::RuntimeError,
                                message,
                            });
                        }
                        Err(error) => return Err(HostError::Fatal(error)),
                    };
                    let hits: Vec<PackHit> = hits
                        .into_iter()
                        .map(|hit| PackHit {
                            id: hit.id,
                            score: hit.score,
                            text: hit.text,
                            attributes: hit.attributes,
                        })
                        .collect();
                    value::to_json(hits)
                }
            },
        ));
    }
    pack
}
