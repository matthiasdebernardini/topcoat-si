//! One answer, by question kind.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::warn_unknown;

/// One answer, by question kind.
///
/// `Debug` prints the byte length of an unknown or malformed answer, not its JSON.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum Answer {
    /// A yes/no answer: the probability of yes, rounded to hundredths.
    Noul(f64),
    /// A pick-one answer.
    Choice(ChoiceAnswer),
    /// A rating on an ordered scale.
    Score(ScoreAnswer),
    /// An answer type this client does not know, as Jev sent it. It is kept so the response is
    /// never lost, and logged as a warning on `topcoat_si::unknown`; the typed accessors report
    /// it as an error.
    Unknown(Value),
    /// A noul, choice or score answer whose fields could not be decoded. It is kept so the
    /// other answers stay usable; the typed accessors report it as an error.
    Malformed {
        /// The answer's `type` field.
        kind: AnswerKind,
        /// The answer as Jev sent it.
        raw: Value,
    },
}

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Noul(p) => f.debug_tuple("Noul").field(p).finish(),
            Self::Choice(choice) => f.debug_tuple("Choice").field(choice).finish(),
            Self::Score(score) => f.debug_tuple("Score").field(score).finish(),
            Self::Unknown(raw) => f
                .debug_struct("Unknown")
                .field("type", &type_tag(raw))
                .field("raw_bytes", &raw.to_string().len())
                .finish(),
            Self::Malformed { kind, raw } => f
                .debug_struct("Malformed")
                .field("kind", kind)
                .field("raw_bytes", &raw.to_string().len())
                .finish(),
        }
    }
}

/// The kind of a question and its answer, as in an answer's `type` field.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnswerKind {
    /// A yes/no answer.
    Noul,
    /// A pick-one answer.
    Choice,
    /// A rating on an ordered scale.
    Score,
}

impl fmt::Display for AnswerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        })
    }
}

/// A choice answer. Probabilities are rounded to hundredths, so `choice` can trail another
/// label by 0.01 and the probabilities need not sum to 1.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ChoiceAnswer {
    /// The picked label.
    pub choice: String,
    /// How concentrated the probabilities are, 0 to 1. Not a measure of correctness.
    pub confidence: f64,
    /// Probability per label, in the order Jev sent them.
    pub probabilities: Vec<LabelProbability>,
}

/// The probability Jev gave one label of a choice question.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LabelProbability {
    /// The option label.
    pub label: String,
    /// Its probability, rounded to hundredths.
    pub probability: f64,
}

/// A score answer. Probabilities are rounded to hundredths and need not sum to 1.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ScoreAnswer {
    /// The probability-weighted mean level, fractional, counting the lowest level as 0.
    pub score: f64,
    /// How concentrated the probabilities are, 0 to 1. Not a measure of correctness.
    pub confidence: f64,
    /// Level index (`"0"`, `"1"`, ...) to the level as Jev echoed it.
    pub legend: serde_json::Map<String, Value>,
    /// Probability per level, lowest first.
    pub probabilities: Vec<f64>,
}

impl ScoreAnswer {
    /// The index of the most likely level, ties going to the lower level. `None` without
    /// probabilities.
    #[must_use]
    pub fn mode(&self) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (index, p) in self.probabilities.iter().copied().enumerate() {
            if best.is_none_or(|(_, top)| p > top) {
                best = Some((index, p));
            }
        }
        best.map(|(index, _)| index)
    }
}

#[derive(Deserialize)]
struct WireNoul {
    noul: f64,
}

#[derive(Deserialize)]
struct WireChoice {
    choice: String,
    confidence: f64,
    probabilities: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct WireScore {
    score: f64,
    confidence: f64,
    // A missing or null legend is empty.
    #[serde(default)]
    legend: Option<serde_json::Map<String, Value>>,
    probabilities: serde_json::Map<String, Value>,
}

/// An answer's `type` field, empty when missing.
pub(super) fn type_tag(value: &Value) -> &str {
    value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

pub(super) fn parse_answer(value: Value) -> Answer {
    let kind = match type_tag(&value) {
        "noul" => AnswerKind::Noul,
        "choice" => AnswerKind::Choice,
        "score" => AnswerKind::Score,
        tag => {
            warn_unknown("Answer", tag);
            return Answer::Unknown(value);
        }
    };
    match decode_answer(kind, &value) {
        Some(answer) => answer,
        None => {
            // Only the shape is logged: the values may echo the caller's data.
            tracing::warn!(
                target: "topcoat_si::jev",
                %kind,
                raw_bytes = value.to_string().len(),
                shape = describe_shape(&value),
                "Jev answer could not be read"
            );
            Answer::Malformed { kind, raw: value }
        }
    }
}

/// Names the JSON type of `value`, and for an object its first keys, capped so a hostile
/// response cannot flood the log.
fn describe_shape(value: &Value) -> String {
    const MAX_KEYS: usize = 8;
    const MAX_KEY_CHARS: usize = 32;
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map
                .keys()
                .take(MAX_KEYS)
                .map(|key| key.chars().take(MAX_KEY_CHARS).collect())
                .collect();
            let more = if map.len() > MAX_KEYS { ", ..." } else { "" };
            format!("object with keys {}{more}", keys.join(", "))
        }
        Value::Array(items) => format!("array of {}", items.len()),
        Value::String(_) => "string".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::Bool(_) => "boolean".to_string(),
        Value::Null => "null".to_string(),
    }
}

fn decode_answer(kind: AnswerKind, value: &Value) -> Option<Answer> {
    match kind {
        AnswerKind::Noul => {
            let wire = WireNoul::deserialize(value).ok()?;
            Some(Answer::Noul(wire.noul))
        }
        AnswerKind::Choice => {
            let wire = WireChoice::deserialize(value).ok()?;
            // The turbofish stays: `?` alone cannot tell `collect` which `Option` to build.
            let probabilities: Vec<_> = wire
                .probabilities
                .into_iter()
                .map(|(label, p)| {
                    Some(LabelProbability {
                        label,
                        probability: p.as_f64()?,
                    })
                })
                .collect::<Option<_>>()?;
            Some(Answer::Choice(ChoiceAnswer {
                choice: wire.choice,
                confidence: wire.confidence,
                probabilities,
            }))
        }
        AnswerKind::Score => {
            let wire = WireScore::deserialize(value).ok()?;
            // Keys must be the level indexes 0..n in any order.
            let mut probabilities = vec![None; wire.probabilities.len()];
            for (key, p) in &wire.probabilities {
                let slot = probabilities.get_mut(key.parse::<usize>().ok()?)?;
                if slot.is_some() {
                    return None;
                }
                *slot = Some(p.as_f64()?);
            }
            Some(Answer::Score(ScoreAnswer {
                score: wire.score,
                confidence: wire.confidence,
                legend: wire.legend.unwrap_or_default(),
                probabilities: probabilities.into_iter().collect::<Option<Vec<_>>>()?,
            }))
        }
    }
}
