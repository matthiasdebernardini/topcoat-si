//! The deterministic grader.

use crate::cases::Case;

/// Whether `answer` is non-empty text that passes every `must_include` group and contains no
/// `must_not_include` string. Matching is case-insensitive.
pub fn deterministic_pass(case: &Case, answer: Option<&str>) -> bool {
    let Some(answer) = answer.filter(|answer| !answer.trim().is_empty()) else {
        return false;
    };
    let answer = answer.to_lowercase();
    let contains = |needle: &String| answer.contains(&needle.to_lowercase());
    case.must_include
        .iter()
        .all(|group| group.iter().any(contains))
        && !case.must_not_include.iter().any(contains)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cases::{Kind, Split};

    fn case() -> Case {
        Case {
            id: "editions".to_string(),
            split: Split::Dev,
            kind: Kind::Fanout,
            prompt: "List the editions.".to_string(),
            must_include: vec![
                vec!["2015".to_string()],
                vec!["Rust 1.31".to_string(), "December 2018".to_string()],
            ],
            must_not_include: vec!["Edition 2027".to_string()],
            judge: "Does it list them?".to_string(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn passes_when_every_group_matches_case_insensitively() {
        assert!(deterministic_pass(&case(), Some("2015, then rust 1.31")));
        assert!(deterministic_pass(&case(), Some("2015 and DECEMBER 2018")));
    }

    #[test]
    fn fails_when_a_group_is_missing() {
        assert!(!deterministic_pass(&case(), Some("2015 only")));
    }

    #[test]
    fn fails_on_a_forbidden_string() {
        assert!(!deterministic_pass(
            &case(),
            Some("2015, Rust 1.31 and edition 2027")
        ));
    }

    #[test]
    fn fails_without_text() {
        assert!(!deterministic_pass(&case(), None));
        assert!(!deterministic_pass(&case(), Some("  \n")));
    }
}
