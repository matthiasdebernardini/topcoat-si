//! The case file: one JSON object per line, validated as a whole.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Which part of the suite a case belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// Cases used while tuning prompts.
    Dev,
    /// Cases kept back to check the tuning did not overfit.
    Holdout,
}

/// What a case exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Many independent lookups.
    Fanout,
    /// Classify many items.
    Classify,
    /// Combine many results into one number or table.
    Aggregate,
    /// One simple lookup, where code mode should not help.
    Control,
    /// Each lookup depends on the one before.
    Multihop,
}

impl Kind {
    /// The name used in the case file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fanout => "fanout",
            Self::Classify => "classify",
            Self::Aggregate => "aggregate",
            Self::Control => "control",
            Self::Multihop => "multihop",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Split {
    /// The name used in the case file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Holdout => "holdout",
        }
    }
}

impl fmt::Display for Split {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One eval case.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub split: Split,
    pub kind: Kind,
    pub prompt: String,
    /// Groups of alternatives: each group passes if the answer contains any one of its strings.
    pub must_include: Vec<Vec<String>>,
    #[serde(default)]
    pub must_not_include: Vec<String>,
    /// One yes/no question for Jev about the final answer.
    pub judge: String,
    /// For humans only; never sent to a model.
    #[serde(default)]
    pub sources: Vec<String>,
}

/// Parses and validates a whole case file. Errors name the line.
pub fn parse_cases(text: &str) -> Result<Vec<Case>, String> {
    let mut cases: Vec<Case> = Vec::new();
    let mut ids = HashSet::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        let case: Case =
            serde_json::from_str(line).map_err(|error| format!("line {line_number}: {error}"))?;
        validate(&case).map_err(|error| format!("line {line_number}: {error}"))?;
        if !ids.insert(case.id.clone()) {
            return Err(format!("line {line_number}: duplicate id {:?}", case.id));
        }
        cases.push(case);
    }
    if cases.is_empty() {
        return Err("the case file holds no cases".to_string());
    }
    Ok(cases)
}

fn validate(case: &Case) -> Result<(), String> {
    if case.id.trim().is_empty() {
        return Err("id is empty".to_string());
    }
    if case.prompt.trim().is_empty() {
        return Err(format!("{}: prompt is empty", case.id));
    }
    if case.judge.trim().is_empty() {
        return Err(format!("{}: judge is empty", case.id));
    }
    if case.must_include.is_empty() {
        return Err(format!("{}: must_include is empty", case.id));
    }
    for (index, group) in case.must_include.iter().enumerate() {
        if group.is_empty() || group.iter().any(|item| item.trim().is_empty()) {
            return Err(format!(
                "{}: must_include group {index} is empty or holds an empty string",
                case.id
            ));
        }
    }
    if case
        .must_not_include
        .iter()
        .any(|item| item.trim().is_empty())
    {
        return Err(format!(
            "{}: must_not_include holds an empty string",
            case.id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"id": "rust-editions", "split": "dev", "kind": "fanout", "prompt": "List every Rust edition.", "must_include": [["2015"], ["2018", "1.31"]], "must_not_include": [], "judge": "Does it list them?", "sources": ["https://doc.rust-lang.org/edition-guide/"]}"#;

    fn with(field: &str, value: &str) -> String {
        let mut case: serde_json::Value = serde_json::from_str(GOOD).unwrap();
        case[field] = serde_json::from_str(value).unwrap();
        case.to_string()
    }

    #[test]
    fn parses_a_good_case() {
        let cases = parse_cases(&format!("{GOOD}\n\n")).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "rust-editions");
        assert_eq!(cases[0].split, Split::Dev);
        assert_eq!(cases[0].kind, Kind::Fanout);
        assert_eq!(cases[0].must_include[1], vec!["2018", "1.31"]);
    }

    #[test]
    fn rejects_unknown_kind() {
        let error = parse_cases(&with("kind", r#""essay""#)).unwrap_err();
        assert!(
            error.starts_with("line 1: unknown variant `essay`"),
            "{error}"
        );
    }

    #[test]
    fn rejects_unknown_split() {
        let error = parse_cases(&with("split", r#""train""#)).unwrap_err();
        assert!(
            error.starts_with("line 1: unknown variant `train`"),
            "{error}"
        );
    }

    #[test]
    fn rejects_empty_must_include() {
        let error = parse_cases(&with("must_include", "[]")).unwrap_err();
        assert_eq!(error, "line 1: rust-editions: must_include is empty");
    }

    #[test]
    fn rejects_empty_group() {
        let error = parse_cases(&with("must_include", r#"[["2015"], []]"#)).unwrap_err();
        assert_eq!(
            error,
            "line 1: rust-editions: must_include group 1 is empty or holds an empty string"
        );
    }

    #[test]
    fn rejects_duplicate_ids() {
        let error = parse_cases(&format!("{GOOD}\n{GOOD}\n")).unwrap_err();
        assert_eq!(error, "line 2: duplicate id \"rust-editions\"");
    }

    #[test]
    fn rejects_an_empty_file() {
        assert_eq!(
            parse_cases("\n").unwrap_err(),
            "the case file holds no cases"
        );
    }

    #[test]
    fn rejects_unknown_field() {
        let error = parse_cases(&with("answer", r#""42""#)).unwrap_err();
        assert!(
            error.starts_with("line 1: unknown field `answer`"),
            "{error}"
        );
    }

    #[test]
    fn shipped_suite_has_the_documented_shape() {
        let cases = parse_cases(include_str!("../suites/code_mode.jsonl")).unwrap();
        assert_eq!(cases.len(), 24);
        let count = |split: Option<Split>, kind: Kind| {
            cases
                .iter()
                .filter(|case| case.kind == kind && split.is_none_or(|split| case.split == split))
                .count()
        };
        let dev = cases.iter().filter(|case| case.split == Split::Dev).count();
        assert_eq!((dev, cases.len() - dev), (16, 8));
        let kinds = [
            Kind::Fanout,
            Kind::Classify,
            Kind::Aggregate,
            Kind::Control,
            Kind::Multihop,
        ];
        let all: Vec<usize> = kinds.iter().map(|&kind| count(None, kind)).collect();
        assert_eq!(all, [8, 6, 4, 4, 2]);
        let holdout: Vec<usize> = kinds
            .iter()
            .map(|&kind| count(Some(Split::Holdout), kind))
            .collect();
        assert_eq!(holdout, [3, 2, 1, 1, 1]);
    }
}
