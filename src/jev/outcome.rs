//! Answers gated on a minimum confidence.

/// A gated answer: confident enough, or the best guess with its confidence.
///
/// Confidence measures how concentrated the answer is, not whether it is correct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome<T> {
    /// The answer met the threshold.
    Answered(T),
    /// The answer fell below the threshold.
    Unsure {
        /// The most likely answer.
        best: T,
        /// The confidence (or, for yes/no, the certainty) that fell short.
        confidence: f64,
    },
}

impl<T> Outcome<T> {
    /// The answer when it met the threshold, `None` when unsure.
    pub fn answered(self) -> Option<T> {
        match self {
            Self::Answered(value) => Some(value),
            Self::Unsure { .. } => None,
        }
    }
}

pub(super) fn gate<T>(value: T, confidence: f64, min_confidence: f64) -> Outcome<T> {
    if confidence >= min_confidence {
        Outcome::Answered(value)
    } else {
        Outcome::Unsure {
            best: value,
            confidence,
        }
    }
}
