use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::types::CacheControlEphemeral;

/// A file produced by code execution.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CodeExecutionOutput {
    /// The ID of the produced file; download it with the Files API.
    pub file_id: String,
}

impl Serialize for CodeExecutionOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("CodeExecutionOutput", 2)?;
        state.serialize_field("type", "code_execution_output")?;
        state.serialize_field("file_id", &self.file_id)?;
        state.end()
    }
}

/// The outcome of a code execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum CodeExecutionToolResultContent {
    /// The code ran.
    #[serde(rename = "code_execution_result")]
    Result {
        /// Standard output.
        stdout: String,
        /// Standard error.
        stderr: String,
        /// Process return code.
        return_code: i32,
        /// Files produced by the run.
        content: Vec<CodeExecutionOutput>,
    },
    /// An encrypted result; the remaining fields are kept verbatim.
    #[serde(rename = "encrypted_code_execution_result")]
    Encrypted {
        /// Every field of the object except `type`.
        #[serde(flatten)]
        fields: Map<String, Value>,
    },
    /// The execution failed.
    #[serde(rename = "code_execution_tool_result_error")]
    Error {
        /// The error code reported by the API.
        error_code: String,
    },
}

/// The result of a server-side code execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeExecutionToolResultBlock {
    /// The ID of the `server_tool_use` block this result answers.
    pub tool_use_id: String,

    /// The execution result or error.
    pub content: CodeExecutionToolResultContent,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}
