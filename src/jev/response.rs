//! Every answer from one request.

use std::fmt;

use bytes::Bytes;
use serde::Deserialize;
use serde_json::Value;

use super::{Answer, ChoiceAnswer, Choices, Levels, Outcome, ScoreAnswer};
use super::{answer, outcome};
use crate::error::{Error, ErrorKind, Result, describe_json_error};

/// Tokens one request consumed. Either count may be missing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct JevUsage {
    /// Input tokens.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Output tokens.
    #[serde(default)]
    pub output_tokens: Option<u64>,
}

/// Every answer from one [`JevClient::ask`](super::JevClient::ask), with the exact bytes sent
/// and received.
///
/// `Debug` omits the bodies, which carry the state.
#[derive(Clone)]
#[non_exhaustive]
pub struct JevResponse {
    /// The model that answered. It can differ from the alias that was sent.
    pub model: String,
    /// Answers with their question names, in the order Jev sent them.
    pub answers: Vec<NamedAnswer>,
    /// Tokens consumed.
    pub usage: JevUsage,
    /// `x-typesafe-request-id`, or `cf-ray` when that is missing.
    pub request_id: Option<String>,
    /// Attempts made, 1 when the first succeeded.
    pub attempts: usize,
    /// The request body as sent, for replay and audit.
    pub request_body: Bytes,
    /// The response body as received.
    pub response_body: Bytes,
}

/// One answer of a [`JevResponse`] with the name of its question.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct NamedAnswer {
    /// The question name.
    pub name: String,
    /// Jev's answer to it.
    pub answer: Answer,
}

impl fmt::Debug for JevResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JevResponse")
            .field("model", &self.model)
            .field("answers", &self.answers)
            .field("usage", &self.usage)
            .field("request_id", &self.request_id)
            .field("attempts", &self.attempts)
            .field("request_body_bytes", &self.request_body.len())
            .field("response_body_bytes", &self.response_body.len())
            .finish()
    }
}

#[derive(Deserialize)]
struct WireResponse {
    model: String,
    answers: serde_json::Map<String, Value>,
    #[serde(default)]
    usage: Option<JevUsage>,
}

impl JevResponse {
    pub(super) fn parse(
        request_body: Bytes,
        response_body: Bytes,
        request_id: Option<String>,
        attempts: usize,
    ) -> Result<Self> {
        let wire = match serde_json::from_slice::<WireResponse>(&response_body) {
            Ok(wire) => wire,
            Err(error) => {
                return Err(Error::new(
                    ErrorKind::Decode,
                    format!(
                        "Failed to parse Jev response ({} bytes): {}",
                        response_body.len(),
                        describe_json_error(&error)
                    ),
                )
                .with_request_id(request_id));
            }
        };
        let answers: Vec<_> = wire
            .answers
            .into_iter()
            .map(|(name, value)| NamedAnswer {
                answer: answer::parse_answer(value),
                name,
            })
            .collect();
        Ok(Self {
            model: wire.model,
            answers,
            usage: wire.usage.unwrap_or_default(),
            request_id,
            attempts,
            request_body,
            response_body,
        })
    }

    fn answer(&self, name: &str) -> Result<&Answer> {
        let answer = self
            .answers
            .iter()
            .find(|answer| answer.name == name)
            .map(|answer| &answer.answer)
            .ok_or_else(|| {
                Error::validation(
                    format!("Jev returned no answer for question {name:?}"),
                    Some(name.to_string()),
                )
            })?;
        match answer {
            Answer::Unknown(raw) => Err(Error::new(
                ErrorKind::Decode,
                format!(
                    "Jev answer {name:?} has type {:?}, which this client does not know",
                    answer::type_tag(raw)
                ),
            )),
            Answer::Malformed { kind, .. } => Err(Error::new(
                ErrorKind::Decode,
                format!("the {kind} answer for {name:?} could not be read"),
            )),
            answer => Ok(answer),
        }
    }

    /// The probability of yes for noul question `name`.
    pub fn noul(&self, name: &str) -> Result<f64> {
        match self.answer(name)? {
            Answer::Noul(p) => finite(name, *p),
            other => Err(wrong_type(name, other, "noul")),
        }
    }

    /// The yes/no answer to noul question `name`, gated on certainty `|2p - 1|`.
    ///
    /// The answer is yes when `p >= 0.5`. Certainty measures how far `p` is from a coin flip,
    /// not whether the answer is correct.
    pub fn yes_no(&self, name: &str, min_certainty: f64) -> Result<Outcome<bool>> {
        self.yes_no_certainty(name, min_certainty)
            .map(|(outcome, _)| outcome)
    }

    /// [`JevResponse::yes_no`] with the certainty, which [`Outcome::Answered`] leaves out.
    pub(crate) fn yes_no_certainty(
        &self,
        name: &str,
        min_certainty: f64,
    ) -> Result<(Outcome<bool>, f64)> {
        let p = self.noul(name)?;
        let certainty = (2.0 * p - 1.0).abs();
        Ok((outcome::gate(p >= 0.5, certainty, min_certainty), certainty))
    }

    /// The answer to choice question `name`.
    pub fn choice(&self, name: &str) -> Result<&ChoiceAnswer> {
        match self.answer(name)? {
            Answer::Choice(choice) => {
                finite(name, choice.confidence)?;
                for label in &choice.probabilities {
                    finite(name, label.probability)?;
                }
                Ok(choice)
            }
            other => Err(wrong_type(name, other, "choice")),
        }
    }

    /// The typed pick for choice question `name`, gated on Jev's `confidence`.
    ///
    /// Confidence measures how concentrated the answer is, not whether it is correct.
    pub fn choose<T: Choices>(&self, name: &str, min_confidence: f64) -> Result<Outcome<T>> {
        let answer = self.choice(name)?;
        let value = T::OPTIONS
            .iter()
            .find(|option| option.label == answer.choice)
            .map(|option| option.value)
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Decode,
                    format!(
                        "Jev's choice for {name:?} is not a label of {}",
                        std::any::type_name::<T>()
                    ),
                )
            })?;
        Ok(outcome::gate(value, answer.confidence, min_confidence))
    }

    /// The answer to score question `name`.
    pub fn score(&self, name: &str) -> Result<&ScoreAnswer> {
        match self.answer(name)? {
            Answer::Score(score) => {
                finite(name, score.score)?;
                finite(name, score.confidence)?;
                for p in &score.probabilities {
                    finite(name, *p)?;
                }
                Ok(score)
            }
            other => Err(wrong_type(name, other, "score")),
        }
    }

    /// The typed most likely level ([`ScoreAnswer::mode`]) for score question `name`, gated on
    /// Jev's `confidence`.
    ///
    /// Confidence measures how concentrated the answer is, not whether it is correct.
    pub fn rate<T: Levels>(&self, name: &str, min_confidence: f64) -> Result<Outcome<T>> {
        let answer = self.score(name)?;
        let value = answer
            .mode()
            .and_then(|index| T::LEVELS.get(index))
            .map(|level| level.value)
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Decode,
                    format!(
                        "Jev's level for {name:?} is not a level of {}",
                        std::any::type_name::<T>()
                    ),
                )
            })?;
        Ok(outcome::gate(value, answer.confidence, min_confidence))
    }
}

fn finite(name: &str, value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::new(
            ErrorKind::Decode,
            format!("Jev answer {name:?} holds a non-finite number"),
        ))
    }
}

fn wrong_type(name: &str, answer: &Answer, expected: &str) -> Error {
    let actual = match answer {
        Answer::Noul(_) => "noul",
        Answer::Choice(_) => "choice",
        Answer::Score(_) => "score",
        Answer::Unknown(_) => "unknown",
        Answer::Malformed { .. } => "malformed answer",
    };
    Error::validation(
        format!("Jev answer {name:?} is a {actual}, not a {expected}"),
        Some(name.to_string()),
    )
}
