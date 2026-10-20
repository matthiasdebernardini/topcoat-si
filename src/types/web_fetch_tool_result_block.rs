use serde::{Deserialize, Serialize};

use crate::types::open_union::with_type;
use crate::types::{CacheControlEphemeral, DocumentBlock};

fn serialize_document<S>(document: &DocumentBlock, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    with_type("document", document, serializer)
}

/// The outcome of a web fetch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum WebFetchToolResultContent {
    /// The page was fetched.
    #[serde(rename = "web_fetch_result")]
    Result {
        /// The fetched URL.
        url: String,
        /// When the page was retrieved (RFC 3339).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retrieved_at: Option<String>,
        /// The page content as a document.
        #[serde(serialize_with = "serialize_document")]
        content: DocumentBlock,
    },
    /// The fetch failed.
    #[serde(rename = "web_fetch_tool_result_error")]
    Error {
        /// The error code reported by the API.
        error_code: String,
    },
}

/// The result of a server-side web fetch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebFetchToolResultBlock {
    /// The ID of the `server_tool_use` block this result answers.
    pub tool_use_id: String,

    /// The fetch result or error.
    pub content: WebFetchToolResultContent,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}
