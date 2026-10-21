//! The questions sent with one request.

use std::collections::HashSet;

use serde::{Serialize, Serializer};

use super::{Choices, Levels};
use crate::error::{Error, Result};

const MAX_CHOICE_OPTIONS: usize = 255;
const MAX_SCORE_LEVELS: usize = 10;

/// The named questions for one [`JevClient::ask`](super::JevClient::ask), kept in the order
/// they were added.
///
/// The chain never fails; [`JevClient::ask`](super::JevClient::ask) checks it before sending:
/// at least one question, names non-empty and unique, a choice has 2 to 255 options with
/// non-empty unique labels, and a score has 2 to 10 non-empty levels. Jev never sees the names,
/// only the instructions and options.
#[derive(Debug, Clone, Default)]
pub struct Questions {
    pub(super) items: Vec<(String, Question)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(super) enum Question {
    Noul {
        instructions: String,
    },
    Choice {
        instructions: String,
        #[serde(serialize_with = "criteria_map")]
        criteria: Vec<(String, Option<String>)>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

fn criteria_map<S: Serializer>(
    options: &[(String, Option<String>)],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;

    let mut map = serializer.serialize_map(Some(options.len()))?;
    for (label, description) in options {
        map.serialize_entry(label, description)?;
    }
    map.end()
}

impl Questions {
    /// An empty set of questions.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a yes/no question. Its answer is the probability of yes.
    #[must_use]
    pub fn noul(mut self, name: impl Into<String>, instructions: impl Into<String>) -> Self {
        self.items.push((
            name.into(),
            Question::Noul {
                instructions: instructions.into(),
            },
        ));
        self
    }

    /// Add a pick-one question over `(label, description)` options, in the order given.
    ///
    /// Jev slightly favors options listed first. Labels and descriptions may be `&str` or
    /// `String`; a list where every description is `None` needs a type, as in
    /// `None::<&str>`.
    #[must_use]
    pub fn choice<L, D>(
        mut self,
        name: impl Into<String>,
        instructions: impl Into<String>,
        options: impl IntoIterator<Item = (L, Option<D>)>,
    ) -> Self
    where
        L: Into<String>,
        D: Into<String>,
    {
        let criteria = options
            .into_iter()
            .map(|(label, description)| (label.into(), description.map(Into::into)))
            .collect();
        self.items.push((
            name.into(),
            Question::Choice {
                instructions: instructions.into(),
                criteria,
            },
        ));
        self
    }

    /// Add a question rating the state on `levels`, lowest first.
    #[must_use]
    pub fn score(
        mut self,
        name: impl Into<String>,
        instructions: impl Into<String>,
        levels: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.items.push((
            name.into(),
            Question::Score {
                instructions: instructions.into(),
                criteria: levels.into_iter().map(Into::into).collect(),
            },
        ));
        self
    }

    /// Add a choice question over the options of `T`, read back with
    /// [`JevResponse::choose`](super::JevResponse::choose).
    #[must_use]
    pub fn choice_of<T: Choices>(
        self,
        name: impl Into<String>,
        instructions: impl Into<String>,
    ) -> Self {
        self.choice(
            name,
            instructions,
            T::OPTIONS
                .iter()
                .map(|option| (option.label, option.description)),
        )
    }

    /// Add a score question over the levels of `T`, read back with
    /// [`JevResponse::rate`](super::JevResponse::rate).
    #[must_use]
    pub fn score_of<T: Levels>(
        self,
        name: impl Into<String>,
        instructions: impl Into<String>,
    ) -> Self {
        self.score(name, instructions, T::LEVELS.iter().map(|level| level.text))
    }

    pub(super) fn validate(&self) -> Result<()> {
        let invalid =
            |message: String, param: &str| Err(Error::validation(message, Some(param.to_string())));
        if self.items.is_empty() {
            return invalid("Jev needs at least one question".to_string(), "questions");
        }
        let mut names = HashSet::new();
        for (name, question) in &self.items {
            if name.trim().is_empty() {
                return invalid(
                    "Jev question names must not be empty".to_string(),
                    "questions",
                );
            }
            if !names.insert(name.as_str()) {
                return invalid(format!("Jev question name {name:?} is used twice"), name);
            }
            match question {
                Question::Noul { .. } => {}
                Question::Choice { criteria, .. } => {
                    if !(2..=MAX_CHOICE_OPTIONS).contains(&criteria.len()) {
                        return invalid(
                            format!(
                                "choice question {name:?} needs 2 to {MAX_CHOICE_OPTIONS} options, \
                                 got {}",
                                criteria.len()
                            ),
                            name,
                        );
                    }
                    let mut labels = HashSet::new();
                    for (label, _) in criteria {
                        if label.trim().is_empty() {
                            return invalid(
                                format!("choice question {name:?} has an empty label"),
                                name,
                            );
                        }
                        if !labels.insert(label.as_str()) {
                            return invalid(
                                format!("choice question {name:?} lists label {label:?} twice"),
                                name,
                            );
                        }
                    }
                }
                Question::Score { criteria, .. } => {
                    if !(2..=MAX_SCORE_LEVELS).contains(&criteria.len()) {
                        return invalid(
                            format!(
                                "score question {name:?} needs 2 to {MAX_SCORE_LEVELS} levels, \
                                 got {}",
                                criteria.len()
                            ),
                            name,
                        );
                    }
                    if criteria.iter().any(|level| level.trim().is_empty()) {
                        return invalid(
                            format!("score question {name:?} has an empty level"),
                            name,
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

pub(super) struct WireQuestions<'questions>(pub(super) &'questions [(String, Question)]);

impl Serialize for WireQuestions<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, question) in self.0 {
            map.serialize_entry(name, question)?;
        }
        map.end()
    }
}
