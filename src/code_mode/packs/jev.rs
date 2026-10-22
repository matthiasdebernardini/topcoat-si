//! The `jev` pack: typed judgments from Jev inside the sandbox.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::code_mode::{Example, HostError, Pack, PyError, host_fn, value};
use crate::error::{ErrorKind, Result};
use crate::jev::{self, JevClient, JevResponse, Outcome, Questions, ToolQuestion};

// The one question the shortcuts ask. Jev never sees names.
const QUESTION: &str = "answer";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    state: Value,
    questions: Vec<ToolQuestion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct YesNoArgs {
    state: Value,
    question: String,
    #[serde(default = "default_certainty")]
    min_certainty: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChooseArgs {
    state: Value,
    question: String,
    options: Vec<String>,
    #[serde(default = "default_confidence")]
    min_confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RateArgs {
    state: Value,
    question: String,
    levels: Vec<String>,
    #[serde(default = "default_confidence")]
    min_confidence: f64,
}

fn default_certainty() -> f64 {
    0.8
}

fn default_confidence() -> f64 {
    0.7
}

/// `ask`, `yes_no`, `choose` and `rate` over one Jev client. Errors map as
/// [`JevTool`](crate::jev::JevTool) maps them: a rejected key ends the run, everything else
/// raises.
#[must_use]
pub fn jev(client: JevClient) -> Pack {
    let client = Arc::new(client);
    let (yes_no_client, choose_client, rate_client) =
        (client.clone(), client.clone(), client.clone());
    Pack::new("jev", "fast typed judgments about a state (text, dict or list)")
        .with_host(host_fn(
            "ask",
            "ask(*, state: object, questions: list[dict]) -> dict",
            "several questions in one request; questions as in the jev tool (name, type noul, \
             choice or score, instructions, options or levels); answers by name",
            move |args: AskArgs| {
                let client = client.clone();
                async move {
                    let questions = Questions::from_tool(&args.questions);
                    let response = mapped(client.ask(args.state, &questions).await)?;
                    Ok(Value::Object(jev::answers(&response, &args.questions)))
                }
            },
        ))
        .with_host(host_fn(
            "yes_no",
            "yes_no(*, state: object, question: str, min_certainty: float = 0.8) -> dict",
            "answer is True, False or None when unsure; also best and certainty",
            move |args: YesNoArgs| {
                let client = yes_no_client.clone();
                async move {
                    let questions = Questions::new().noul(QUESTION, args.question);
                    let response = mapped(client.ask(args.state, &questions).await)?;
                    let (outcome, certainty) =
                        mapped(response.yes_no_certainty(QUESTION, args.min_certainty))?;
                    let (Outcome::Answered(best) | Outcome::Unsure { best, .. }) = outcome;
                    Ok(json!({"answer": outcome.answered(), "best": best, "certainty": certainty}))
                }
            },
        ))
        .with_host(host_fn(
            "choose",
            "choose(*, state: object, question: str, options: list[str], min_confidence: float \
             = 0.7) -> dict",
            "pick one option; answer is the option or None when unsure; also best, confidence \
             and probabilities",
            move |args: ChooseArgs| {
                let client = choose_client.clone();
                async move {
                    let questions = Questions::new().choice(
                        QUESTION,
                        args.question,
                        args.options.into_iter().map(|o| (o, None::<String>)),
                    );
                    let response = mapped(client.ask(args.state, &questions).await)?;
                    value::to_json(choice(&response, args.min_confidence)?)
                }
            },
        ))
        .with_host(host_fn(
            "rate",
            "rate(*, state: object, question: str, levels: list[str], min_confidence: float = \
             0.7) -> dict",
            "rate on levels, lowest first; answer is the level or None when unsure; also best, \
             confidence and probabilities",
            move |args: RateArgs| {
                let client = rate_client.clone();
                async move {
                    let questions =
                        Questions::new().score(QUESTION, args.question, &args.levels);
                    let response = mapped(client.ask(args.state, &questions).await)?;
                    value::to_json(score(&response, &args.levels, args.min_confidence)?)
                }
            },
        ))
        .with_example(
            "yes_no",
            Example {
                code: r#"await yes_no(state="Please refund me", question="Is this a refund request?")"#,
                expect: "=> {'answer': True, 'best': True, 'certainty': 0.98}",
            },
        )
        .with_example(
            "choose",
            Example {
                code: r#"(await choose(state="The app crashes", question="Which team?", options=["billing", "technical"]))["answer"]"#,
                expect: "=> 'technical'",
            },
        )
        .with_example(
            "rate",
            Example {
                code: r#"(await rate(state="Fix it today", question="How urgent?", levels=["low", "high"]))["answer"]"#,
                expect: "=> 'high'",
            },
        )
        .with_example(
            "ask",
            Example {
                code: r#"(await ask(state="Refund me", questions=[{"name": "refund", "type": "noul", "instructions": "Refund requested?"}]))["refund"]["probability_yes"]"#,
                expect: "=> 0.99",
            },
        )
}

/// A gated pick: `answer` is `None` when unsure.
#[derive(Serialize)]
struct Judgment<'label> {
    answer: Option<&'label str>,
    best: Option<&'label str>,
    confidence: f64,
    probabilities: Map<String, Value>,
}

fn choice(
    response: &JevResponse,
    min_confidence: f64,
) -> std::result::Result<Judgment<'_>, HostError> {
    let answer = mapped(response.choice(QUESTION))?;
    Ok(Judgment {
        answer: (answer.confidence >= min_confidence).then_some(answer.choice.as_str()),
        best: Some(&answer.choice),
        confidence: answer.confidence,
        probabilities: answer
            .probabilities
            .iter()
            .map(|option| (option.label.clone(), json!(option.probability)))
            .collect(),
    })
}

fn score<'a>(
    response: &'a JevResponse,
    levels: &'a [String],
    min_confidence: f64,
) -> std::result::Result<Judgment<'a>, HostError> {
    let answer = mapped(response.score(QUESTION))?;
    let best = answer
        .mode()
        .and_then(|index| levels.get(index))
        .map(String::as_str);
    Ok(Judgment {
        answer: best.filter(|_| answer.confidence >= min_confidence),
        best,
        confidence: answer.confidence,
        probabilities: levels
            .iter()
            .zip(&answer.probabilities)
            .map(|(level, p)| (level.clone(), json!(p)))
            .collect(),
    })
}

/// Maps a Jev error as the `jev` tool does: fatal ends the run, a failed check raises
/// `ValueError`, anything else `RuntimeError`.
fn mapped<T>(outcome: Result<T>) -> std::result::Result<T, HostError> {
    outcome.map_err(|error| match error {
        error if jev::is_fatal(&error) => HostError::Fatal(error),
        error if matches!(error.kind(), ErrorKind::Validation { .. }) => HostError::Raise {
            kind: PyError::ValueError,
            message: format!("Invalid jev call: {error}"),
        },
        error => HostError::Raise {
            kind: PyError::RuntimeError,
            message: format!("Jev failed: {error}"),
        },
    })
}
