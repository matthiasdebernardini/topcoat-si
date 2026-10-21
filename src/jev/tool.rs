//! The client-side `jev` tool: Claude asks Jev typed questions about a state.

use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Answer, JevClient, JevResponse, Questions};
use crate::error::{Error, Result};
use crate::tool::ClientTool;
use crate::types::{TextBlock, ToolParam, ToolResultBlock, ToolUnionParam, ToolUseBlock};

const TOOL_NAME: &str = "jev";

/// Jev exposed to Claude as a client-side `jev` tool.
///
/// One call sends one state with any number of questions and costs one Jev request. Problems
/// the model or a retry can fix (bad input, a failed check, 400, 413, 422, 429, 5xx, timeouts,
/// connection failures, an edge-proxy block) come back as an `is_error` result. A rejected key
/// (401), a 403 with a JSON body and other configuration problems are returned as `Err`.
#[derive(Debug, Clone)]
pub struct JevTool {
    client: JevClient,
}

impl JevTool {
    /// Wrap a client. The client's model, timeout and retries apply to every call.
    pub fn new(client: JevClient) -> Self {
        Self { client }
    }

    async fn run(&self, tool_use: &ToolUseBlock) -> Result<ToolResultBlock> {
        let id = tool_use.id.clone();
        let input = match serde_json::from_value::<ToolInput>(tool_use.input.clone()) {
            Ok(input) => input,
            Err(error) => {
                return Ok(error_result(
                    id,
                    format!(
                        "Invalid jev input: {error}. Send an object with \"state\" (a string, \
                         object or array) and a non-empty \"questions\" array."
                    ),
                ));
            }
        };
        let questions = questions(&input.questions);
        match self.client.ask(input.state, &questions).await {
            Ok(response) => Ok(ToolResultBlock::new(id)
                .with_text_content(TextBlock::new(render(&response, &input.questions)))),
            Err(error) if is_fatal(&error) => Err(error),
            Err(Error::Validation { message, .. }) => {
                Ok(error_result(id, format!("Invalid jev call: {message}")))
            }
            Err(error) => Ok(error_result(id, format!("Jev failed: {error}"))),
        }
    }
}

impl ClientTool for JevTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn definition(&self) -> ToolUnionParam {
        let instructions = json!({
            "type": "string",
            "description": "The question in plain words. Jev sees this and the options, never the name."
        });
        let name = json!({
            "type": "string",
            "description": "Your key for this answer, unique within the call."
        });
        let schema = json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": ["string", "object", "array"],
                    "description": "Everything Jev should judge: text, or a JSON object or array."
                },
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "name": name,
                                    "type": {"type": "string", "enum": ["noul"]},
                                    "instructions": instructions
                                },
                                "required": ["name", "type", "instructions"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "name": name,
                                    "type": {"type": "string", "enum": ["choice"]},
                                    "instructions": instructions,
                                    "options": {
                                        "type": "array",
                                        "minItems": 2,
                                        "maxItems": 255,
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "label": {"type": "string"},
                                                "description": {"type": "string"}
                                            },
                                            "required": ["label"],
                                            "additionalProperties": false
                                        }
                                    }
                                },
                                "required": ["name", "type", "instructions", "options"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "name": name,
                                    "type": {"type": "string", "enum": ["score"]},
                                    "instructions": instructions,
                                    "levels": {
                                        "type": "array",
                                        "minItems": 2,
                                        "maxItems": 10,
                                        "items": {"type": "string"},
                                        "description": "Ordered levels, lowest first."
                                    }
                                },
                                "required": ["name", "type", "instructions", "levels"],
                                "additionalProperties": false
                            }
                        ]
                    }
                }
            },
            "required": ["state", "questions"],
            "additionalProperties": false
        });
        ToolUnionParam::CustomTool(
            ToolParam::new(TOOL_NAME.to_string(), schema).with_description(
                "Fast typed judgments about the given state: yes/no (noul), pick one (choice), \
                 or rate on an ordered scale (score). Put independent questions about the same \
                 state in one call. Jev never sees question names, so put everything it needs \
                 in the instructions and options. Confidence measures how concentrated an \
                 answer is, not whether it is correct. Jev slightly favors options listed first."
                    .to_string(),
            ),
        )
    }

    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>> {
        Box::pin(self.run(tool_use))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolInput {
    state: Value,
    questions: Vec<ToolQuestion>,
}

/// One question as the model writes it, in the `jev` tool's input.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum ToolQuestion {
    Noul {
        name: String,
        instructions: String,
    },
    Choice {
        name: String,
        instructions: String,
        options: Vec<ToolOption>,
    },
    Score {
        name: String,
        instructions: String,
        levels: Vec<String>,
    },
}

/// One choice option as the model writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

// Only these failures are the developer's to fix; everything else goes back to the model.
// Timeouts and connection failures are checked first: a body-phase timeout carries the status
// its headers arrived with, often 200.
pub(crate) fn is_fatal(error: &Error) -> bool {
    if matches!(error, Error::Timeout { .. } | Error::Connection { .. }) {
        return false;
    }
    match error.status_code() {
        // A 2xx here means the body read failed after the headers arrived.
        Some(200..=299 | 400 | 408 | 413 | 422 | 429 | 500..=599) => false,
        Some(403) => error.metadata().and_then(|m| m.error_type.as_deref()) != Some("edge_blocked"),
        Some(_) => true,
        None => false,
    }
}

fn error_result(id: String, message: String) -> ToolResultBlock {
    ToolResultBlock::new(id)
        .with_string_content(message)
        .with_error(true)
}

/// The model's questions as [`Questions`], in order.
pub(crate) fn questions(items: &[ToolQuestion]) -> Questions {
    let mut questions = Questions::new();
    for question in items {
        questions = match question {
            ToolQuestion::Noul { name, instructions } => {
                questions.noul(name.as_str(), instructions.as_str())
            }
            ToolQuestion::Choice {
                name,
                instructions,
                options,
            } => questions.choice(
                name.as_str(),
                instructions.as_str(),
                options
                    .iter()
                    .map(|o| (o.label.as_str(), o.description.as_deref())),
            ),
            ToolQuestion::Score {
                name,
                instructions,
                levels,
            } => questions.score(
                name.as_str(),
                instructions.as_str(),
                levels.iter().map(String::as_str),
            ),
        };
    }
    questions
}

fn render(response: &JevResponse, questions: &[ToolQuestion]) -> String {
    json!({"model": response.model, "answers": answers(response, questions)}).to_string()
}

/// Every answer by question name, with its probabilities; a score also names its most likely
/// level.
pub(crate) fn answers(
    response: &JevResponse,
    questions: &[ToolQuestion],
) -> serde_json::Map<String, Value> {
    let levels = |name: &str| {
        questions.iter().find_map(|question| match question {
            ToolQuestion::Score {
                name: score_name,
                levels,
                ..
            } if score_name == name => Some(levels),
            _ => None,
        })
    };
    let mut answers = serde_json::Map::new();
    for (name, answer) in &response.answers {
        let value = match answer {
            Answer::Noul(p) => json!({"type": "noul", "probability_yes": p}),
            Answer::Choice(choice) => {
                let probabilities: serde_json::Map<String, Value> = choice
                    .probabilities
                    .iter()
                    .map(|(label, p)| (label.clone(), json!(p)))
                    .collect();
                json!({
                    "type": "choice",
                    "choice": choice.choice,
                    "confidence": choice.confidence,
                    "probabilities": probabilities
                })
            }
            Answer::Score(score) => {
                let level = score
                    .mode()
                    .and_then(|index| levels(name)?.get(index).cloned());
                json!({
                    "type": "score",
                    "score": score.score,
                    "level": level,
                    "confidence": score.confidence,
                    "probabilities": score.probabilities
                })
            }
            Answer::Unknown { raw, .. } => raw.clone(),
            Answer::Malformed { kind, .. } => {
                json!({"type": kind, "error": "Jev's answer could not be read"})
            }
        };
        answers.insert(name.clone(), value);
    }
    answers
}
