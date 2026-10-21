use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

use crate::types::open_union;
use crate::types::{CacheControlEphemeral, DocumentBlock};

fn serialize_document<S>(document: &DocumentBlock, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    open_union::with_type("document", document, serializer)
}

/// The outcome of a web fetch.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
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
    /// An outcome of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for WebFetchToolResultContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum Known {
            #[serde(rename = "web_fetch_result")]
            Result {
                url: String,
                #[serde(default)]
                retrieved_at: Option<String>,
                content: DocumentBlock,
            },
            #[serde(rename = "web_fetch_tool_result_error")]
            Error { error_code: String },
            #[serde(other)]
            Other,
        }

        let value = Value::deserialize(deserializer)?;
        Ok(
            match Known::deserialize(&value).map_err(de::Error::custom)? {
                Known::Result {
                    url,
                    retrieved_at,
                    content,
                } => Self::Result {
                    url,
                    retrieved_at,
                    content,
                },
                Known::Error { error_code } => Self::Error { error_code },
                Known::Other => {
                    open_union::warn_unknown_member("WebFetchToolResultContent", &value);
                    Self::Unknown(value)
                }
            },
        )
    }
}

/// The result of a server-side web fetch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct WebFetchToolResultBlock {
    /// The ID of the `server_tool_use` block this result answers.
    pub tool_use_id: String,

    /// The fetch result or error.
    pub content: WebFetchToolResultContent,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}
