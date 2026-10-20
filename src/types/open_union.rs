//! Helpers shared by the open (forward-compatible) tagged unions.

use serde::Serialize;
use serde_json::{Map, Value};

/// Reads the `type` tag of a union member.
pub(crate) fn type_tag<E: serde::de::Error>(value: &Value) -> Result<String, E> {
    value
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| E::missing_field("type"))
}

/// Deserializes a known union member, reporting failures as the deserializer's error.
pub(crate) fn from_value<T, E>(value: Value) -> Result<T, E>
where
    T: serde::de::DeserializeOwned,
    E: serde::de::Error,
{
    serde_json::from_value(value).map_err(E::custom)
}

/// Serializes `value` as a JSON object with `"type": tag` placed first.
///
/// Used for nested items whose struct does not carry its own `type` field.
pub(crate) fn with_type<T, S>(
    tag: &'static str,
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: serde::Serializer,
{
    let inner = serde_json::to_value(value).map_err(serde::ser::Error::custom)?;
    let Value::Object(fields) = inner else {
        return Err(serde::ser::Error::custom("expected a JSON object"));
    };
    let mut tagged = Map::with_capacity(fields.len() + 1);
    tagged.insert("type".to_string(), Value::String(tag.to_string()));
    for (key, field) in fields {
        tagged.insert(key, field);
    }
    Value::Object(tagged).serialize(serializer)
}

/// Like [`with_type`], for a slice of items sharing one tag.
pub(crate) fn with_type_seq<T, S>(
    tag: &'static str,
    values: &[T],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;

    struct Tagged<'a, T>(&'static str, &'a T);
    impl<T: Serialize> Serialize for Tagged<'_, T> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            with_type(self.0, self.1, serializer)
        }
    }

    let mut seq = serializer.serialize_seq(Some(values.len()))?;
    for value in values {
        seq.serialize_element(&Tagged(tag, value))?;
    }
    seq.end()
}
