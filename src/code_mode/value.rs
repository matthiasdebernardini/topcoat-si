//! Values crossing between the sandbox and host functions: Python objects to JSON and back.

use std::fmt;

use monty_types::{CallArgs, ExcType, MontyException, MontyObject, ObjectRef};
use serde::Serialize;
use serde_json::{Map, Number, Value};

use super::HostError;
use crate::error::Error;

// Bounds the host's recursion. Cycles arrive as `cycle` leaves, which do not convert.
const MAX_DEPTH: usize = 64;
// Bounds the work and memory of one call's conversion: values are a graph, so a list that
// holds one list twice, nested 30 deep, is small in the sandbox but 2^30 items as JSON.
pub(super) const MAX_ITEMS: usize = 100_000;

/// The keyword arguments of one call as a JSON object.
pub(super) fn kwargs_to_json(args: &CallArgs) -> Result<Value, MontyException> {
    let mut walk = Walk { items: 0 };
    let mut map = Map::new();
    for (key, value) in args.kwargs() {
        let key = key.as_str().ok_or_else(|| {
            exception(ExcType::TypeError, "keyword names must be str".to_string())
        })?;
        map.insert(key.to_string(), walk.convert(&value, 0)?);
    }
    Ok(Value::Object(map))
}

/// One conversion, counting the container items it has visited.
struct Walk {
    items: usize,
}

impl Walk {
    fn count(&mut self, items: usize) -> Result<(), MontyException> {
        self.items += items;
        if self.items > MAX_ITEMS {
            return Err(exception(
                ExcType::ValueError,
                format!("value too large: more than {MAX_ITEMS} items"),
            ));
        }
        Ok(())
    }

    fn convert(&mut self, value: &ObjectRef<'_>, depth: usize) -> Result<Value, MontyException> {
        if depth > MAX_DEPTH {
            return Err(exception(
                ExcType::ValueError,
                format!("value nested deeper than {MAX_DEPTH} levels"),
            ));
        }
        let not_converted = || {
            exception(
                ExcType::TypeError,
                format!(
                    "cannot pass a {} to a host function; use None, bool, int, float, str, \
                     list, tuple or dict",
                    value.type_name()
                ),
            )
        };
        match value.type_name() {
            "NoneType" => Ok(Value::Null),
            "bool" => value.as_bool().map(Value::Bool).ok_or_else(not_converted),
            "int" => value.as_int().map(Value::from).ok_or_else(|| {
                exception(
                    ExcType::ValueError,
                    "int does not fit in 64 bits".to_string(),
                )
            }),
            "float" => value
                .as_float()
                .and_then(Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| exception(ExcType::ValueError, "float is not finite".to_string())),
            "str" => value
                .as_str()
                .map(|s| Value::String(s.to_string()))
                .ok_or_else(not_converted),
            "list" | "tuple" => {
                let items = value.items().ok_or_else(not_converted)?;
                self.count(items.len())?;
                items
                    .iter()
                    .map(|item| self.convert(item, depth + 1))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
            "dict" => {
                let pairs = value.pairs().ok_or_else(not_converted)?;
                self.count(pairs.len())?;
                let mut map = Map::new();
                for (key, item) in pairs {
                    let key = key.as_str().ok_or_else(|| {
                        exception(
                            ExcType::TypeError,
                            format!("dict keys must be str, got {}", key.type_name()),
                        )
                    })?;
                    map.insert(key.to_string(), self.convert(&item, depth + 1)?);
                }
                Ok(Value::Object(map))
            }
            _ => Err(not_converted()),
        }
    }
}

/// A host function's JSON result as a Python value: objects become dicts, arrays lists.
pub(super) fn json_to_object(value: &Value) -> MontyObject {
    match value {
        Value::Null => MontyObject::none(),
        Value::Bool(b) => MontyObject::bool(*b),
        Value::Number(n) => n.as_i64().map_or_else(
            || MontyObject::float(n.as_f64().unwrap_or(f64::NAN)),
            MontyObject::int,
        ),
        Value::String(s) => MontyObject::string(s.clone()),
        Value::Array(items) => MontyObject::list(items.iter().map(json_to_object)),
        Value::Object(map) => MontyObject::dict(
            map.iter()
                .map(|(k, v)| (MontyObject::string(k.clone()), json_to_object(v))),
        ),
    }
}

/// The `repr()` of a final value, stopped after `max_chars` characters; `true` when cut.
///
/// Streams the value rather than building its whole repr, which for a graph with shared
/// references can be exponentially longer than the value is in memory.
pub(super) fn capped_repr(value: &MontyObject, max_chars: usize) -> (String, bool) {
    use std::fmt::Write;

    let mut out = Capped {
        text: String::new(),
        room: max_chars,
    };
    // A `str` displays as itself, so take its repr; it is no bigger than the sandbox's memory.
    let cut = if value.type_name() == "str" {
        out.write_str(&value.py_repr()).is_err()
    } else {
        write!(out, "{value}").is_err()
    };
    (out.text, cut)
}

/// A writer that keeps `room` more characters, then fails to stop the formatting.
struct Capped {
    text: String,
    room: usize,
}

impl fmt::Write for Capped {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut chars = s.chars();
        self.text.extend(chars.by_ref().take(self.room));
        self.room = self.room.saturating_sub(s.chars().count());
        if chars.next().is_some() {
            return Err(fmt::Error);
        }
        Ok(())
    }
}

/// A Python exception to hand the sandbox, which raises it; building one raises nothing.
pub(super) fn exception(kind: ExcType, message: String) -> MontyException {
    MontyException::new(kind, Some(message))
}

/// A host function's typed result as JSON.
pub(super) fn to_json(value: impl Serialize) -> Result<Value, HostError> {
    serde_json::to_value(value).map_err(|error| {
        HostError::Fatal(
            Error::validation("could not serialize a host function's result", None)
                .with_source(error),
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn kwargs_convert_to_json() {
        let mut args = CallArgs::new();
        args.push_kwarg("n", MontyObject::int(3));
        args.push_kwarg("x", MontyObject::float(1.5));
        args.push_kwarg("s", MontyObject::string("hi"));
        args.push_kwarg("none", MontyObject::none());
        args.push_kwarg(
            "items",
            MontyObject::tuple([MontyObject::bool(true), MontyObject::list([])]),
        );
        args.push_kwarg(
            "map",
            MontyObject::dict([(MontyObject::string("k"), MontyObject::int(1))]),
        );
        assert_eq!(
            kwargs_to_json(&args).unwrap(),
            json!({"n": 3, "x": 1.5, "s": "hi", "none": null, "items": [true, []], "map": {"k": 1}})
        );
    }

    #[test]
    fn unconvertible_values_raise() {
        let cases = [
            (MontyObject::float(f64::NAN), ExcType::ValueError),
            (
                MontyObject::dict([(MontyObject::int(1), MontyObject::int(2))]),
                ExcType::TypeError,
            ),
            (MontyObject::set([MontyObject::int(1)]), ExcType::TypeError),
        ];
        for (value, kind) in cases {
            let mut args = CallArgs::new();
            args.push_kwarg("v", value);
            assert_eq!(kwargs_to_json(&args).unwrap_err().exc_type(), kind);
        }
    }

    #[test]
    fn json_converts_back() {
        let value = json!({"a": [1, 2.5, "s", null, false]});
        assert_eq!(
            json_to_object(&value).py_repr(),
            "{'a': [1, 2.5, 's', None, False]}"
        );
    }
}
