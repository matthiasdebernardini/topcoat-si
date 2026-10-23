//! Documents written to a namespace and their ids.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One document: an id, the text that is embedded and indexed, and filterable attributes.
///
/// Attribute values pass through as JSON; turbopuffer rejects unsupported types such as objects
/// with a 400. Store dates as ISO 8601 strings or Unix seconds so range filters compare them
/// correctly.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Document {
    /// Unique within the namespace. Writing an existing id replaces that document.
    pub id: DocumentId,
    /// Embedded and indexed for keyword search.
    pub text: String,
    /// Other attributes: filterable, returned with each hit.
    pub attributes: Map<String, Value>,
}

impl Document {
    /// A document with no attributes.
    pub fn new(id: impl Into<DocumentId>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            attributes: Map::new(),
        }
    }

    /// Set one attribute. Names are 1 to 128 bytes, must not start with `$`, and must not be
    /// `id`, `text`, `embed_text` or `vector`.
    #[must_use]
    pub fn with_attribute(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.attributes.insert(name.into(), value.into());
        self
    }
}

/// A document id: an unsigned integer, or a string (such as a UUID) of at most 64 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum DocumentId {
    /// A numeric id.
    Number(u64),
    /// A string id.
    Text(String),
}

impl From<u64> for DocumentId {
    fn from(id: u64) -> Self {
        Self::Number(id)
    }
}

impl From<&str> for DocumentId {
    fn from(id: &str) -> Self {
        Self::Text(id.to_string())
    }
}

impl From<String> for DocumentId {
    fn from(id: String) -> Self {
        Self::Text(id)
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(id) => write!(f, "{id}"),
            Self::Text(id) => f.write_str(id),
        }
    }
}
