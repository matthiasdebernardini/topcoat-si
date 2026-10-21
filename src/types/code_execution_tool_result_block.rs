use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value};

use crate::types::{CacheControlEphemeral, open_union};

/// A file produced by code execution.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
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
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
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
    /// An outcome of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for CodeExecutionToolResultContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum Known {
            #[serde(rename = "code_execution_result")]
            Result {
                stdout: String,
                stderr: String,
                return_code: i32,
                content: Vec<CodeExecutionOutput>,
            },
            #[serde(rename = "encrypted_code_execution_result")]
            Encrypted {
                #[serde(flatten)]
                fields: Map<String, Value>,
            },
            #[serde(rename = "code_execution_tool_result_error")]
            Error { error_code: String },
            #[serde(other)]
            Other,
        }

        let value = Value::deserialize(deserializer)?;
        Ok(
            match Known::deserialize(&value).map_err(de::Error::custom)? {
                Known::Result {
                    stdout,
                    stderr,
                    return_code,
                    content,
                } => Self::Result {
                    stdout,
                    stderr,
                    return_code,
                    content,
                },
                Known::Encrypted { fields } => Self::Encrypted { fields },
                Known::Error { error_code } => Self::Error { error_code },
                Known::Other => {
                    open_union::warn_unknown_member("CodeExecutionToolResultContent", &value);
                    Self::Unknown(value)
                }
            },
        )
    }
}

/// The result of a server-side code execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct CodeExecutionToolResultBlock {
    /// The ID of the `server_tool_use` block this result answers.
    pub tool_use_id: String,

    /// The execution result or error.
    pub content: CodeExecutionToolResultContent,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}
