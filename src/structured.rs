//! Typed JSON replies: a schema generated from a Rust type, sent as a structured output format, and
//! the reply parsed back into that type.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::error::describe_json_error;
use crate::{
    Anthropic, ContentBlock, Error, ErrorKind, Message, MessageCreateParams, OutputFormat, Result,
    StopReason,
};

/// String formats structured outputs accept; any other `format` moves into the description.
const SUPPORTED_STRING_FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

/// JSON Schema types structured outputs accept.
const SUPPORTED_TYPES: &[&str] = &[
    "object", "array", "string", "integer", "number", "boolean", "null",
];

/// A reply parsed into `T`, with the message it came from (for usage, stop reason and request id).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Parsed<T> {
    /// The reply's JSON, deserialized.
    pub value: T,
    /// The full response the value was parsed from.
    pub message: Message,
}

impl OutputFormat {
    /// Builds a structured output format from `T`'s schemars schema.
    ///
    /// Deriving `JsonSchema` needs your own `schemars = "1"` dependency, on the same major
    /// version Topcoat SI uses; a different major version's derive does not implement this trait.
    ///
    /// The schema is rewritten to the subset structured outputs accept, following the rules in
    /// <https://platform.claude.com/docs/en/build-with-claude/structured-outputs> ("JSON Schema
    /// limitations", "How SDK transformation works") as the official Python SDK implements them in
    /// `transform_schema` (`src/anthropic/lib/_parse/_transform.py`):
    ///
    /// - Every object gets `additionalProperties: false`, replacing whatever was there, so a map
    ///   type such as `HashMap<String, u32>` becomes an object with no allowed keys.
    /// - `oneOf` becomes `anyOf`; `anyOf` and `allOf` are kept with their branches transformed.
    /// - `$defs` are transformed and kept; a `$ref` is kept and its siblings (including a doc
    ///   comment description) are dropped.
    /// - String `format` is kept only for `date-time`, `time`, `date`, `duration`, `email`,
    ///   `hostname`, `uri`, `ipv4`, `ipv6` and `uuid`; array `minItems` only when it is 0 or 1.
    /// - `enum`, `description`, `title` and `required` are kept. `const` and `default` are kept
    ///   too, because the docs list them as supported (the Python SDK moves them to the
    ///   description).
    /// - Every other keyword (`minimum`, `maximum`, `maxLength`, `pattern`, schemars' integer
    ///   `format` such as `uint32`, ...) is removed and appended to the description as
    ///   `{minimum: 1, maximum: 10}`, so the model sees it but the API does not enforce it.
    ///   Check those constraints yourself after parsing.
    /// - A `type` list such as `["string", "null"]` (how schemars writes `Option<String>`)
    ///   becomes an `anyOf` with one branch per type.
    ///
    /// # Errors
    ///
    /// An [`ErrorKind::UnsupportedSchema`] error naming the type for a recursive type, which
    /// structured outputs do not support. A validation error when the schema cannot be expressed
    /// otherwise: a subschema with no `type`, `anyOf`, `oneOf` or `allOf` (such as
    /// `serde_json::Value`), an unsupported type, or a `$ref` that does not point into `$defs`.
    pub fn json_schema_for<T: JsonSchema>() -> Result<OutputFormat> {
        let generated = SchemaSettings::draft2020_12()
            .for_deserialize()
            .with(|settings| settings.meta_schema = None)
            .into_generator()
            .into_root_schema_for::<T>()
            .to_value();
        let schema = transform(&generated)?;
        reject_recursion(&schema, &T::schema_name())?;
        Ok(OutputFormat::json_schema(schema))
    }
}

/// Rewrites one (sub)schema to the structured outputs subset; see [`OutputFormat::json_schema_for`].
fn transform(schema: &Value) -> Result<Value> {
    let Some(source) = schema.as_object() else {
        return Err(Error::validation(
            "structured outputs need every subschema to be an object with a type",
            None,
        ));
    };
    let mut rest = source.clone();
    let mut strict = Map::new();

    if let Some(defs) = rest.shift_remove("$defs") {
        let Value::Object(defs) = defs else {
            return Err(Error::validation("schema `$defs` must be an object", None));
        };
        let mut strict_defs = Map::new();
        for (name, def) in &defs {
            strict_defs.insert(name.clone(), transform(def)?);
        }
        strict.insert("$defs".to_string(), Value::Object(strict_defs));
    }

    if let Some(reference) = rest.shift_remove("$ref") {
        strict.insert("$ref".to_string(), reference);
        return Ok(Value::Object(strict));
    }

    let mut type_ = rest.shift_remove("type");
    if let Some(Value::Array(types)) = &type_
        && let [single] = types.as_slice()
    {
        type_ = Some(single.clone());
    }
    let any_of = rest.shift_remove("anyOf");
    let one_of = rest.shift_remove("oneOf");
    let all_of = rest.shift_remove("allOf");

    let mut kind = None;
    if let Some(Value::Array(branches)) = any_of.as_ref().or(one_of.as_ref()) {
        strict.insert("anyOf".to_string(), transform_all(branches)?);
    } else if let Some(Value::Array(branches)) = &all_of {
        strict.insert("allOf".to_string(), transform_all(branches)?);
    } else {
        match type_ {
            Some(Value::String(name)) if SUPPORTED_TYPES.contains(&name.as_str()) => {
                strict.insert("type".to_string(), Value::String(name.clone()));
                kind = Some(name);
            }
            Some(Value::Array(types)) => {
                strict.insert("anyOf".to_string(), split_types(&types, &mut rest)?);
            }
            Some(_) => {
                return Err(Error::validation(
                    "structured outputs support only the types object, array, string, integer, number, boolean and null",
                    None,
                ));
            }
            None => {
                return Err(Error::validation(
                    "structured outputs need every subschema to have a `type`, `anyOf`, `oneOf` or `allOf`",
                    None,
                ));
            }
        }
    }

    for keyword in ["enum", "const", "default", "description", "title"] {
        if let Some(value) = rest.shift_remove(keyword) {
            strict.insert(keyword.to_string(), value);
        }
    }

    match kind.as_deref() {
        Some("object") => {
            let mut properties = Map::new();
            if let Some(Value::Object(source)) = rest.shift_remove("properties") {
                for (name, property) in &source {
                    properties.insert(name.clone(), transform(property)?);
                }
            }
            strict.insert("properties".to_string(), Value::Object(properties));
            rest.shift_remove("additionalProperties");
            strict.insert("additionalProperties".to_string(), Value::Bool(false));
            if let Some(required) = rest.shift_remove("required") {
                strict.insert("required".to_string(), required);
            }
        }
        Some("string") => {
            if let Some(format) = rest.shift_remove("format") {
                if format
                    .as_str()
                    .is_some_and(|format| SUPPORTED_STRING_FORMATS.contains(&format))
                {
                    strict.insert("format".to_string(), format);
                } else {
                    rest.insert("format".to_string(), format);
                }
            }
        }
        Some("array") => {
            if let Some(items) = rest.shift_remove("items") {
                strict.insert("items".to_string(), transform(&items)?);
            }
            if let Some(min_items) = rest.shift_remove("minItems") {
                if min_items.as_u64().is_some_and(|min| min <= 1) {
                    strict.insert("minItems".to_string(), min_items);
                } else {
                    rest.insert("minItems".to_string(), min_items);
                }
            }
        }
        _ => {}
    }

    if !rest.is_empty() {
        let leftovers = rest
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) => format!("{key}: {text}"),
                other => format!("{key}: {other}"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let description = match strict.get("description").and_then(Value::as_str) {
            Some(description) => format!("{description}\n\n{{{leftovers}}}"),
            None => format!("{{{leftovers}}}"),
        };
        strict.insert("description".to_string(), Value::String(description));
    }

    Ok(Value::Object(strict))
}

fn transform_all(branches: &[Value]) -> Result<Value> {
    branches
        .iter()
        .map(transform)
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

/// Turns a `type` list into `anyOf` branches. Every non-null branch takes the type-specific
/// keywords; `enum`, `const`, `default`, `description` and `title` stay on the outer schema.
fn split_types(types: &[Value], rest: &mut Map<String, Value>) -> Result<Value> {
    let outer = ["enum", "const", "default", "description", "title"];
    let mut shared = Map::new();
    rest.retain(|key, value| {
        if outer.contains(&key.as_str()) {
            true
        } else {
            shared.insert(key.clone(), value.clone());
            false
        }
    });
    let mut branches = Vec::with_capacity(types.len());
    for type_ in types {
        let mut branch = if type_.as_str() == Some("null") {
            Map::new()
        } else {
            shared.clone()
        };
        branch.insert("type".to_string(), type_.clone());
        branches.push(transform(&Value::Object(branch))?);
    }
    Ok(Value::Array(branches))
}

/// Fails when a type reaches itself through `$ref`, since structured outputs reject recursive
/// schemas.
fn reject_recursion(schema: &Value, root_name: &str) -> Result<()> {
    const ROOT: &str = "#";
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut root_refs = BTreeSet::new();
    collect_refs(schema, &mut root_refs)?;
    graph.insert(ROOT.to_string(), root_refs);
    if let Some(Value::Object(defs)) = schema.get("$defs") {
        for (name, def) in defs {
            let mut refs = BTreeSet::new();
            collect_refs(def, &mut refs)?;
            graph.insert(name.clone(), refs);
        }
    }

    // Depth-first search for a back edge, with an explicit stack so hostile depth cannot overflow.
    let mut finished = BTreeSet::new();
    let mut on_path = BTreeSet::new();
    let mut stack: Vec<(String, Vec<String>)> = Vec::new();
    let children = |node: &str| -> Vec<String> {
        graph
            .get(node)
            .map(|refs| refs.iter().cloned().collect())
            .unwrap_or_default()
    };
    on_path.insert(ROOT.to_string());
    stack.push((ROOT.to_string(), children(ROOT)));
    while let Some((node, pending)) = stack.last_mut() {
        let Some(next) = pending.pop() else {
            on_path.remove(node.as_str());
            finished.insert(node.clone());
            stack.pop();
            continue;
        };
        if on_path.contains(&next) {
            let name = if next == ROOT { root_name } else { &next };
            return Err(Error::new(
                ErrorKind::UnsupportedSchema {
                    type_name: name.to_string(),
                },
                format!(
                    "structured outputs do not support recursive schemas: type `{name}` contains itself"
                ),
            ));
        }
        if !finished.contains(&next) {
            on_path.insert(next.clone());
            let grandchildren = children(&next);
            stack.push((next, grandchildren));
        }
    }
    Ok(())
}

/// Collects the `$defs` names (or `#` for the root) that `schema` references, skipping nested
/// `$defs`, which are graph nodes of their own.
fn collect_refs(schema: &Value, refs: &mut BTreeSet<String>) -> Result<()> {
    let mut pending = vec![schema];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "$defs" {
                        continue;
                    }
                    if key == "$ref" {
                        let target = child.as_str().unwrap_or_default();
                        if target == "#" {
                            refs.insert(target.to_string());
                        } else if let Some(name) = target.strip_prefix("#/$defs/") {
                            refs.insert(name.to_string());
                        } else {
                            return Err(Error::validation(
                                "structured outputs support only `$ref`s into the schema's own `$defs`",
                                None,
                            ));
                        }
                    } else {
                        pending.push(child);
                    }
                }
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    Ok(())
}

impl Message {
    /// Parses the reply's text as JSON into `T`.
    ///
    /// Text blocks are joined in order; thinking, tool and other blocks are skipped. Use it on a
    /// reply to a request with a JSON schema [`OutputFormat`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::MaxTokens`] when the reply stopped at `max_tokens` (the JSON is cut off; retry
    /// with a higher `max_tokens`), [`ErrorKind::Refusal`] when Claude refused (a refusal does not
    /// follow the schema), or [`ErrorKind::Decode`] when the text does not parse as `T`. The parse
    /// error gives the error category, line, column
    /// and the text's byte count, never the text or serde's message, which can quote the reply.
    pub fn parse_json<T: DeserializeOwned>(&self) -> Result<T> {
        match self.stop_reason {
            Some(StopReason::MaxTokens) => {
                return Err(Error::new(
                    ErrorKind::MaxTokens,
                    "the reply was cut off at max_tokens, so its JSON is incomplete; retry with a higher max_tokens",
                ));
            }
            Some(StopReason::Refusal) => {
                return Err(Error::new(
                    ErrorKind::Refusal,
                    "Claude refused the request, so the reply does not follow the schema",
                ));
            }
            _ => {}
        }
        let text: String = self
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        // serde's message and the error itself can quote the reply, so neither is kept.
        serde_json::from_str(&text).map_err(|error| {
            Error::new(
                ErrorKind::Decode,
                format!(
                    "the reply's {} bytes of text are not the expected JSON: {}",
                    text.len(),
                    describe_json_error(&error)
                ),
            )
        })
    }
}

impl Anthropic {
    /// Sends `params` with `T`'s schema as the output format and parses the reply into `T`.
    ///
    /// `T` needs `#[derive(JsonSchema)]`, which needs your own `schemars = "1"` dependency on the
    /// same major version Topcoat SI uses.
    ///
    /// Sets `output_config.format` and keeps the other `output_config` fields, such as effort.
    /// Structured outputs cannot be combined with citations: a request that enables citations on
    /// a document or search result fails with a 400 from the API.
    ///
    /// # Errors
    ///
    /// A validation error, before any request, when `params` already has an output format or `T`'s
    /// schema cannot be expressed (see [`OutputFormat::json_schema_for`]); any error from
    /// [`Anthropic::send`]; and any error from [`Message::parse_json`].
    pub async fn send_parsed<T: DeserializeOwned + JsonSchema>(
        &self,
        params: &MessageCreateParams,
    ) -> Result<Parsed<T>> {
        if params
            .output_config
            .as_ref()
            .is_some_and(|config| config.format.is_some())
        {
            return Err(Error::validation(
                "send_parsed sets output_config.format itself; remove the existing format",
                Some("output_config.format".to_string()),
            ));
        }
        let format = OutputFormat::json_schema_for::<T>()?;
        let mut params = params.clone();
        let config = params.output_config.take().unwrap_or_default();
        params.output_config = Some(config.with_format(format));
        let message = self.send(&params).await?;
        let value = message.parse_json()?;
        Ok(Parsed { value, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Canned, serve};
    use crate::{Effort, KnownModel, MessageParam, Model, OutputConfig};
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    #[serde(rename_all = "snake_case")]
    enum Mood {
        Calm,
        Angry,
    }

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    struct Step {
        /// What to do.
        action: String,
        #[schemars(range(min = 1, max = 10))]
        minutes: u8,
    }

    #[derive(Debug, PartialEq, Deserialize, JsonSchema)]
    struct Plan {
        mood: Mood,
        note: Option<String>,
        steps: Vec<Step>,
    }

    #[derive(Deserialize, JsonSchema)]
    struct Tree {
        #[allow(dead_code, reason = "only the schema is under test")]
        children: Vec<Tree>,
    }

    #[derive(Deserialize, JsonSchema)]
    struct Forest {
        #[allow(dead_code, reason = "only the schema is under test")]
        branch: Branch,
    }

    #[derive(Deserialize, JsonSchema)]
    struct Branch {
        #[allow(dead_code, reason = "only the schema is under test")]
        twigs: Vec<Branch>,
    }

    fn schema_of<T: JsonSchema>() -> Value {
        let OutputFormat::JsonSchema { schema } = OutputFormat::json_schema_for::<T>().unwrap();
        schema
    }

    fn message(stop_reason: &str, content: Value) -> Message {
        serde_json::from_value(json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-haiku-5-5",
            "content": content, "stop_reason": stop_reason, "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }))
        .unwrap()
    }

    #[test]
    fn schema_transform_matches_structured_outputs_subset() {
        assert_eq!(
            schema_of::<Plan>(),
            json!({
                "$defs": {
                    "Mood": {"type": "string", "enum": ["calm", "angry"]},
                    "Step": {
                        "type": "object",
                        "properties": {
                            "action": {"type": "string", "description": "What to do."},
                            "minutes": {
                                "type": "integer",
                                "description": "{format: uint8, minimum: 1, maximum: 10}"
                            }
                        },
                        "additionalProperties": false,
                        "required": ["action", "minutes"]
                    }
                },
                "type": "object",
                "title": "Plan",
                "properties": {
                    "mood": {"$ref": "#/$defs/Mood"},
                    "note": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "steps": {"type": "array", "items": {"$ref": "#/$defs/Step"}}
                },
                "additionalProperties": false,
                "required": ["mood", "steps"]
            })
        );
    }

    #[test]
    fn schema_transform_keeps_supported_keywords_and_moves_the_rest() {
        let schema = transform(&json!({
            "type": "object",
            "properties": {
                "when": {"type": "string", "format": "date-time"},
                "code": {"type": "string", "format": "hex", "maxLength": 4, "description": "Code."},
                "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                "many": {"type": "array", "items": {"type": "string"}, "minItems": 3},
                "kind": {"oneOf": [{"type": "string", "const": "a"}, {"type": "string", "const": "b"}]}
            },
            "additionalProperties": {"type": "string"}
        }))
        .unwrap();
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "when": {"type": "string", "format": "date-time"},
                    "code": {"type": "string", "description": "Code.\n\n{maxLength: 4, format: hex}"},
                    "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                    "many": {"type": "array", "items": {"type": "string"}, "description": "{minItems: 3}"},
                    "kind": {"anyOf": [{"type": "string", "const": "a"}, {"type": "string", "const": "b"}]}
                },
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn recursive_types_are_rejected_by_name() {
        let error = OutputFormat::json_schema_for::<Tree>().unwrap_err();
        assert_eq!(
            error.kind(),
            &ErrorKind::UnsupportedSchema {
                type_name: "Tree".to_string()
            }
        );
        assert!(error.to_string().contains("`Tree`"), "{error}");

        let error = OutputFormat::json_schema_for::<Forest>().unwrap_err();
        assert!(matches!(
            error.kind(),
            ErrorKind::UnsupportedSchema { type_name } if type_name == "Branch"
        ));
        assert!(error.to_string().contains("`Branch`"), "{error}");
    }

    #[test]
    fn untyped_values_are_rejected() {
        let error = OutputFormat::json_schema_for::<Value>().unwrap_err();
        assert!(matches!(error.kind(), ErrorKind::Validation { .. }));
    }

    #[test]
    fn parse_json_joins_text_blocks_and_skips_others() {
        let reply = message(
            "end_turn",
            json!([
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "text", "text": "{\"mood\": \"calm\", \"note\": null, "},
                {"type": "text", "text": "\"steps\": [{\"action\": \"breathe\", \"minutes\": 2}]}"}
            ]),
        );
        assert_eq!(
            reply.parse_json::<Plan>().unwrap(),
            Plan {
                mood: Mood::Calm,
                note: None,
                steps: vec![Step {
                    action: "breathe".to_string(),
                    minutes: 2
                }],
            }
        );
    }

    #[test]
    fn parse_json_reports_max_tokens_cut_off() {
        let reply = message(
            "max_tokens",
            json!([{"type": "text", "text": "{\"mood\": "}]),
        );
        let error = reply.parse_json::<Plan>().unwrap_err();
        assert!(error.to_string().contains("max_tokens"), "{error}");
    }

    #[test]
    fn parse_json_reports_refusal() {
        let reply = message("refusal", json!([{"type": "text", "text": "I can't help"}]));
        let error = reply.parse_json::<Plan>().unwrap_err();
        assert!(error.to_string().contains("refused"), "{error}");
    }

    #[test]
    fn parse_json_stops_have_their_own_kinds() {
        let cut_off = message(
            "max_tokens",
            json!([{"type": "text", "text": "{\"mood\": "}]),
        );
        let error = cut_off.parse_json::<Plan>().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::MaxTokens, "{error}");
        assert!(!error.is_retryable());

        let refused = message("refusal", json!([{"type": "text", "text": "I can't help"}]));
        let error = refused.parse_json::<Plan>().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Refusal, "{error}");
        assert!(!error.is_retryable());
    }

    #[test]
    fn parse_json_error_has_byte_count_not_text() {
        let cases = [
            ("SECRET-REPLY not json", "syntax error at line 1 column 1"),
            (
                r#"{"mood": "calm", "note": null, "steps": "SECRET-REPLY"}"#,
                "type or value mismatch at line 1 column 54",
            ),
            (
                r#"{"mood": "SECRET-REPLY", "note": null, "steps": []}"#,
                "type or value mismatch at line 1 column 23",
            ),
        ];
        for (text, expected) in cases {
            let reply = message("end_turn", json!([{"type": "text", "text": text}]));
            let error = reply.parse_json::<Plan>().unwrap_err();
            assert!(matches!(error.kind(), ErrorKind::Decode));
            let shown = error.to_string();
            assert!(shown.contains(&format!("{} bytes", text.len())), "{shown}");
            assert!(shown.contains(expected), "{shown}");
            for shown in [shown, format!("{error:?}")] {
                assert!(!shown.contains("SECRET-REPLY"), "{shown}");
            }
        }
    }

    fn client(base_url: String) -> Anthropic {
        Anthropic::try_new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
    }

    fn params() -> MessageCreateParams {
        MessageCreateParams::new(
            256,
            vec![MessageParam::user("plan my morning")],
            Model::Known(KnownModel::ClaudeHaiku55),
        )
    }

    #[tokio::test]
    async fn send_parsed_sends_schema_and_keeps_effort() {
        let reply = json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-haiku-5-5",
            "content": [{"type": "text", "text": "{\"mood\":\"angry\",\"note\":\"late\",\"steps\":[]}"}],
            "stop_reason": "end_turn", "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        });
        let (base_url, recorded) = serve(vec![Canned::json(200, reply.to_string())]).await;
        let params = params().with_output_config(OutputConfig::new().with_effort(Effort::Low));

        let parsed = client(base_url).send_parsed::<Plan>(&params).await.unwrap();

        assert_eq!(parsed.value.mood, Mood::Angry);
        assert_eq!(parsed.value.note.as_deref(), Some("late"));
        assert_eq!(parsed.message.id, "msg_1");
        let requests = recorded.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(
            body["output_config"],
            json!({"format": {"type": "json_schema", "schema": schema_of::<Plan>()}, "effort": "low"})
        );
    }

    #[tokio::test]
    async fn send_parsed_rejects_existing_format_without_a_request() {
        let (base_url, recorded) = serve(Vec::new()).await;
        let params = params().with_output_config(
            OutputConfig::new().with_format(OutputFormat::json_schema(json!({"type": "object"}))),
        );

        let error = client(base_url)
            .send_parsed::<Plan>(&params)
            .await
            .unwrap_err();

        assert!(matches!(error.kind(), ErrorKind::Validation { .. }));
        assert!(recorded.lock().unwrap().is_empty());
    }
}
