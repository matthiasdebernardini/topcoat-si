use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Metadata for a file stored through the Files API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileInfo {
    /// Unique file identifier.
    pub id: String,

    /// Object type; always `"file"`.
    pub r#type: String,

    /// Original filename of the uploaded file.
    pub filename: String,

    /// MIME type of the file.
    pub mime_type: String,

    /// Size of the file in bytes.
    pub size_bytes: u64,

    /// When the file was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,

    /// Whether the file can be downloaded; `false` when the API omits it.
    #[serde(default)]
    pub downloadable: bool,

    /// When the file expires, if it has an expiry.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,
}

/// One page of files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileList {
    /// The files on this page.
    pub data: Vec<FileInfo>,

    /// Cursor for the next page, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page: Option<String>,

    /// Whether more files are available after this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Query parameters for listing files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileListParams {
    /// Number of files to return per page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,

    /// Pagination cursor from a previous [`FileList::next_page`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

impl FileListParams {
    /// Creates empty parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the page size.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Sets the pagination cursor.
    pub fn with_page(mut self, page: impl Into<String>) -> Self {
        self.page = Some(page.into());
        self
    }

    /// The query-string pairs for this request.
    pub(crate) fn query_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        if let Some(limit) = self.limit {
            pairs.push(("limit".to_string(), limit.to_string()));
        }
        if let Some(page) = &self.page {
            pairs.push(("page".to_string(), page.clone()));
        }
        pairs
    }
}

/// Confirmation returned when a file is deleted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeletedFile {
    /// ID of the deleted file.
    pub id: String,

    /// Object type; always `"file_deleted"`.
    pub r#type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_info_parses_with_and_without_expiry() {
        let json = json!({
            "id": "file_01",
            "type": "file",
            "filename": "a.txt",
            "mime_type": "text/plain",
            "size_bytes": 5,
            "created_at": "2026-10-19T00:00:00Z",
            "downloadable": false
        });
        let info: FileInfo = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(info.expires_at, None);
        assert_eq!(serde_json::to_value(&info).unwrap(), json);

        let mut with_expiry = json;
        with_expiry["expires_at"] = json!("2026-11-19T00:00:00Z");
        let info: FileInfo = serde_json::from_value(with_expiry).unwrap();
        assert!(info.expires_at.is_some());
    }

    #[test]
    fn file_info_defaults_missing_downloadable_to_false() {
        let info: FileInfo = serde_json::from_value(json!({
            "id": "file_01",
            "type": "file",
            "filename": "a.txt",
            "mime_type": "text/plain",
            "size_bytes": 5,
            "created_at": "2026-10-19T00:00:00Z"
        }))
        .unwrap();
        assert!(!info.downloadable);
    }

    #[test]
    fn file_list_defaults_optional_fields() {
        let list: FileList = serde_json::from_value(json!({"data": []})).unwrap();
        assert!(list.data.is_empty());
        assert_eq!(list.next_page, None);
        assert_eq!(list.has_more, None);
    }

    #[test]
    fn list_params_query_pairs() {
        let params = FileListParams::new().with_limit(5).with_page("p2");
        assert_eq!(
            params.query_pairs(),
            vec![
                ("limit".to_string(), "5".to_string()),
                ("page".to_string(), "p2".to_string())
            ]
        );
    }
}
