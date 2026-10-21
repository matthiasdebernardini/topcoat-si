use serde::{Deserialize, Serialize};

/// A reference to a file uploaded through the Files API.
///
/// Used as the `file` source of document and image blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileSource {
    /// The ID of the uploaded file.
    pub file_id: String,
}

impl FileSource {
    /// Creates a file source for the given file ID.
    #[must_use]
    pub fn new(file_id: impl Into<String>) -> Self {
        Self {
            file_id: file_id.into(),
        }
    }
}
