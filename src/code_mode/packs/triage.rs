//! The `triage` pack.

use crate::code_mode::{Example, Pack};

/// `classify_all`, `score_all` and `group_by`. Requires the `jev` pack.
#[must_use]
pub fn triage() -> Pack {
    Pack::new("triage", "classify and score many items at once")
        .with_source(include_str!("triage.py"))
        .requires(["jev"])
        .with_python_fn(
            "classify_all",
            "async def classify_all(*, items: list, question: str, options: list[str], \
             min_confidence: float = 0.7) -> list[dict]",
            "choose per item concurrently; items (dicts, or strings as {text}) come back with \
             answer and confidence added; an item that could not be judged has answer and \
             confidence None and an \"error\" key, so filter on it",
        )
        .with_python_fn(
            "score_all",
            "async def score_all(*, items: list, question: str, levels: list[str], \
             min_confidence: float = 0.7) -> list[dict]",
            "the same with rate; levels lowest first; failed items carry an \"error\" key",
        )
        .with_python_fn(
            "group_by",
            "def group_by(rows: list[dict], key: str) -> dict",
            "rows grouped by their key value, None under \"unsure\"",
        )
        .with_example(
            "classify_all",
            Example {
                code: r#"await classify_all(items=["The app crashes", "broken"], question="Which team?", options=["billing", "technical"])"#,
                expect: "=> [{'text': 'The app crashes', 'answer': 'technical', 'confidence': 0.9}, {'text': 'broken', 'answer': None, 'confidence': None, 'error': 'jev failed'}]",
            },
        )
        .with_example(
            "score_all",
            Example {
                code: r#"await score_all(items=[{"id": 1, "text": "Fix it today"}, {"id": 2, "text": "broken"}], question="How urgent?", levels=["low", "high"])"#,
                expect: "=> [{'id': 1, 'text': 'Fix it today', 'answer': 'high', 'confidence': 0.9}, {'id': 2, 'text': 'broken', 'answer': None, 'confidence': None, 'error': 'jev failed'}]",
            },
        )
        .with_example(
            "group_by",
            Example {
                code: r#"group_by([{"answer": "a"}, {"answer": None}, {"answer": "a"}], "answer")"#,
                expect: "=> {'a': [{'answer': 'a'}, {'answer': 'a'}], 'unsure': [{'answer': None}]}",
            },
        )
}
