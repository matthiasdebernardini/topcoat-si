//! The `reports` pack.

use crate::code_mode::{Example, Pack};

/// `table`, `to_csv` and `count_by`: plain functions, no host calls.
#[must_use]
pub fn reports() -> Pack {
    Pack::new("reports", "tables, CSV and counts from lists of dicts")
        .with_source(include_str!("reports.py"))
        .with_python_fn(
            "table",
            "def table(rows: list[dict], columns: list[str] | None = None) -> str",
            "markdown table; columns default to the first row's keys, so an \"error\" column \
             appears only when the first row has one",
        )
        .with_python_fn(
            "to_csv",
            "def to_csv(rows: list[dict], columns: list[str] | None = None) -> str",
            "CSV text; columns default to the first row's keys, as for table",
        )
        .with_python_fn(
            "count_by",
            "def count_by(rows: list[dict], key: str) -> list[dict]",
            "[{key: value, \"count\": n}] sorted by count, descending",
        )
        .with_example(
            "table",
            Example {
                code: r#"print(table([{"name": "a", "n": 1}, {"name": "b", "n": 2}]))"#,
                expect: "| name | n |\n|---|---|\n| a | 1 |\n| b | 2 |\n=> None",
            },
        )
        .with_example(
            "to_csv",
            Example {
                code: r#"to_csv([{"name": "a, b", "n": 1}])"#,
                expect: "=> 'name,n\\r\\n\"a, b\",1\\r\\n'",
            },
        )
        .with_example(
            "count_by",
            Example {
                code: r#"count_by([{"team": "a"}, {"team": "b"}, {"team": "b"}], "team")"#,
                expect: "=> [{'team': 'b', 'count': 2}, {'team': 'a', 'count': 1}]",
            },
        )
}
