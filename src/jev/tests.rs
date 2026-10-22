#![cfg(test)]

use std::collections::HashMap;
use std::error::Error as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::client::DEFAULT_BASE_URL;
use super::*;
use crate::ClientTool;
use crate::error::{Error, ErrorCode, ErrorKind, Service};
use crate::test_support::{Canned, Recorded, serve, uses_proxies, warnings_on};
use crate::types::{
    Content, ToolResultBlock, ToolResultBlockContent, ToolUnionParam, ToolUseBlock,
};

const KEY: &str = "ts-test-key-5b1e";
const SECRET: &str = "SECRET-STATE-7f3a";
const MESSAGE: &str = "I was charged twice. Please refund the duplicate today.";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Team {
    Billing,
    Technical,
    Other,
}

impl Choices for Team {
    const OPTIONS: &'static [ChoiceOption<Self>] = &[
        ChoiceOption {
            value: Team::Billing,
            label: "billing",
            description: Some("Charges, payments, and refunds"),
        },
        ChoiceOption {
            value: Team::Technical,
            label: "technical",
            description: Some("Software faults"),
        },
        ChoiceOption {
            value: Team::Other,
            label: "other",
            description: Some("Anything else"),
        },
    ];
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Urgency {
    NoDeadline,
    Week,
    Today,
}

impl Levels for Urgency {
    const LEVELS: &'static [Level<Self>] = &[
        Level {
            value: Urgency::NoDeadline,
            text: "No deadline",
        },
        Level {
            value: Urgency::Week,
            text: "Within a week",
        },
        Level {
            value: Urgency::Today,
            text: "Today",
        },
    ];
}

fn client(base_url: &str) -> JevClient {
    JevClient::try_new(KEY, "jev-1.13.0")
        .unwrap()
        .with_base_url(base_url)
        .unwrap()
}

fn ok(body: Value) -> Canned {
    Canned::json(200, body.to_string())
}

// Live-recorded triage response (jev-1.13.0).
fn triage_response() -> Value {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "route": {
                "type": "choice",
                "choice": "billing",
                "confidence": 1.0,
                "probabilities": {"billing": 1.0, "technical": 0.0, "other": 0.0}
            },
            "urgency": {
                "type": "score",
                "score": 2.0,
                "confidence": 1.0,
                "legend": {"0": "No deadline", "1": "Within a week", "2": "Today"},
                "probabilities": {"0": 0.0, "1": 0.0, "2": 1.0}
            },
            "refund_requested": {"type": "noul", "noul": 0.99}
        },
        "usage": {"input_tokens": 403, "output_tokens": 70}
    })
}

fn triage_questions() -> Questions {
    Questions::new()
        .choice_of::<Team>("route", "Which team should handle the customer message?")
        .score_of::<Urgency>("urgency", "How soon does the customer ask for action?")
        .noul(
            "refund_requested",
            "Does the customer explicitly request a refund?",
        )
}

const TRIAGE_REQUEST: &str = concat!(
    r#"{"model":"jev-1.13.0","state":{"message":"I was charged twice. Please refund the duplicate today."},"#,
    r#""questions":{"route":{"type":"choice","instructions":"Which team should handle the customer message?","#,
    r#""criteria":{"billing":"Charges, payments, and refunds","technical":"Software faults","other":"Anything else"}},"#,
    r#""urgency":{"type":"score","instructions":"How soon does the customer ask for action?","#,
    r#""criteria":["No deadline","Within a week","Today"]},"#,
    r#""refund_requested":{"type":"noul","instructions":"Does the customer explicitly request a refund?"}}}"#
);

fn header<'a>(recorded: &'a Recorded, name: &str) -> Option<&'a str> {
    recorded.headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn parsed(body: Value) -> JevResponse {
    JevResponse::parse(Bytes::new(), Bytes::from(body.to_string()), None, 1).unwrap()
}

fn deep_state(depth: usize) -> Value {
    let mut state = json!([]);
    for _ in 1..depth {
        state = json!([state]);
    }
    state
}

fn validation_param(error: &Error) -> Option<&str> {
    match error.kind() {
        ErrorKind::Validation { param, .. } => param.as_deref(),
        _ => panic!("expected a validation error, got {error:?}"),
    }
}

fn error_code(error: &Error) -> Option<JevErrorCode> {
    match error.kind() {
        ErrorKind::Api {
            error_code: None, ..
        } => None,
        ErrorKind::Api {
            error_code: Some(ErrorCode::Jev(code)),
            ..
        } => Some(*code),
        _ => panic!("expected a Jev API error, got {error:?}"),
    }
}

#[tokio::test]
async fn golden_request_bytes_and_headers() {
    let (url, recorded) = serve(vec![
        ok(triage_response()).with_header("x-typesafe-request-id", "req_123"),
    ])
    .await;
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();

    let recorded = recorded.lock().unwrap();
    let request = &recorded[0];
    assert_eq!(request.body, TRIAGE_REQUEST);
    assert_eq!(response.request_body, TRIAGE_REQUEST.as_bytes());
    let first_line = request.headers.lines().next().unwrap();
    assert!(
        first_line.starts_with("POST /v1/systemone "),
        "{first_line}"
    );
    assert_eq!(
        header(request, "authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(header(request, "content-type"), Some("application/json"));
    assert_eq!(header(request, "accept"), Some("application/json"));
    assert_eq!(
        header(request, "user-agent"),
        Some(concat!("topcoat-si/", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(header(request, "x-typesafe-retry-count"), None);

    assert_eq!(response.model, "jev-1.13.0");
    let names: Vec<&str> = response
        .answers
        .iter()
        .map(|answer| answer.name.as_str())
        .collect();
    assert_eq!(names, ["route", "urgency", "refund_requested"]);
    assert_eq!(
        response.usage,
        JevUsage {
            input_tokens: Some(403),
            output_tokens: Some(70)
        }
    );
    assert_eq!(response.request_id.as_deref(), Some("req_123"));
    assert_eq!(response.attempts, 1);
    assert_eq!(
        response.response_body,
        triage_response().to_string().as_bytes()
    );
}

#[tokio::test]
async fn owned_options_keep_order_and_null_descriptions() {
    let (url, recorded) = serve(vec![ok(json!({"model": "jev-1.13.0", "answers": {}}))]).await;
    let rows = [
        (7, "Blue Sea Ray".to_string()),
        (3, "Red Boston Whaler".to_string()),
    ];
    let options = rows
        .iter()
        .map(|(id, line)| (format!("boat:{id}"), Some(line.clone())))
        .chain([("none".to_string(), None)]);
    let questions = Questions::new()
        .choice("boat", "Which boat is described?", options)
        .score(
            "size",
            "How large?",
            vec!["small".to_string(), "large".to_string()],
        );
    client(&url).ask("a blue boat", &questions).await.unwrap();

    assert_eq!(
        recorded.lock().unwrap()[0].body,
        concat!(
            r#"{"model":"jev-1.13.0","state":"a blue boat","questions":{"boat":{"type":"choice","#,
            r#""instructions":"Which boat is described?","criteria":{"boat:7":"Blue Sea Ray","#,
            r#""boat:3":"Red Boston Whaler","none":null}},"size":{"type":"score","#,
            r#""instructions":"How large?","criteria":["small","large"]}}}"#
        )
    );
}

#[tokio::test]
async fn validation_failures_send_nothing() {
    let (url, recorded) = serve(vec![]).await;
    let jev = client(&url);
    let one = || Questions::new().noul("q", "Is it?");
    let none = None::<&str>;
    let cases: Vec<(&str, Value, Questions, &str, &str)> = vec![
        (
            "no questions",
            json!("s"),
            Questions::new(),
            "needs at least one question",
            "questions",
        ),
        (
            "empty name",
            json!("s"),
            Questions::new().noul(" ", "Is it?"),
            "names must not be empty",
            "questions",
        ),
        (
            "duplicate name",
            json!("s"),
            one().noul("q", "Again?"),
            "name \"q\" is used twice",
            "q",
        ),
        (
            "one option",
            json!("s"),
            Questions::new().choice("c", "Which?", [("a", none)]),
            "needs 2 to 255 options, got 1",
            "c",
        ),
        (
            "256 options",
            json!("s"),
            Questions::new().choice("c", "Which?", (0..256).map(|i| (i.to_string(), none))),
            "needs 2 to 255 options, got 256",
            "c",
        ),
        (
            "empty label",
            json!("s"),
            Questions::new().choice("c", "Which?", [("a", none), (" ", none)]),
            "has an empty label",
            "c",
        ),
        (
            "duplicate label",
            json!("s"),
            Questions::new().choice("c", "Which?", [("a", none), ("a", none)]),
            "lists label \"a\" twice",
            "c",
        ),
        (
            "one level",
            json!("s"),
            Questions::new().score("s", "How?", ["low"]),
            "needs 2 to 10 levels, got 1",
            "s",
        ),
        (
            "eleven levels",
            json!("s"),
            Questions::new().score("s", "How?", (0..11).map(|i| i.to_string())),
            "needs 2 to 10 levels, got 11",
            "s",
        ),
        (
            "empty level",
            json!("s"),
            Questions::new().score("s", "How?", ["low", ""]),
            "has an empty level",
            "s",
        ),
        (
            "null state",
            Value::Null,
            one(),
            "must be a string, object or array",
            "state",
        ),
        (
            "bool state",
            json!(true),
            one(),
            "must be a string, object or array",
            "state",
        ),
        (
            "number state",
            json!(3),
            one(),
            "must be a string, object or array",
            "state",
        ),
        (
            "state 65 deep",
            deep_state(65),
            one(),
            "nested more than 64 levels deep",
            "state",
        ),
    ];
    for (case, state, questions, expected, param) in cases {
        let error = jev.ask(state, &questions).await.unwrap_err();
        match error.kind() {
            ErrorKind::Validation {
                param: Some(actual),
                ..
            } => {
                let message = error.message();
                assert!(message.contains(expected), "{case}: {message}");
                assert_eq!(actual, param, "{case}");
            }
            _ => panic!("{case}: expected a validation error with a param, got {error}"),
        }
    }
    assert!(recorded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn limits_are_inclusive() {
    let empty = || ok(json!({"model": "jev-1.13.0", "answers": {}}));
    let (url, recorded) = serve(vec![empty(), empty(), empty()]).await;
    let jev = client(&url);
    let none = None::<&str>;
    jev.ask(
        "s",
        &Questions::new().choice("c", "Which?", (0..255).map(|i| (i.to_string(), none))),
    )
    .await
    .unwrap();
    jev.ask(
        "s",
        &Questions::new().score("s", "How?", (0..10).map(|i| i.to_string())),
    )
    .await
    .unwrap();
    let response = jev
        .ask(deep_state(64), &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap();
    assert_eq!(response.usage, JevUsage::default());
    assert_eq!(recorded.lock().unwrap().len(), 3);
}

#[test]
fn rounded_probabilities_are_accepted() {
    // Live-recorded shapes: sums of 0.99 and a choice that trails another label by 0.01.
    let response = parsed(json!({
        "model": "jev-1.13.0",
        "answers": {
            "route": {
                "type": "choice",
                "choice": "technical",
                "confidence": 0.1,
                "probabilities": {"billing": 0.4, "technical": 0.39, "other": 0.2}
            },
            "progress": {
                "type": "score",
                "score": 1.83,
                "confidence": 0.66,
                "legend": {"0": {"level": "none"}, "1": "some", "2": "most", "3": "all"},
                "probabilities": {"0": 0.01, "1": 0.23, "2": 0.67, "3": 0.08}
            }
        },
        "usage": null
    }));
    let route = response.choice("route").unwrap();
    assert_eq!(route.choice, "technical");
    assert_eq!(
        route
            .probabilities
            .iter()
            .map(|label| (label.label.as_str(), label.probability))
            .collect::<Vec<_>>(),
        vec![("billing", 0.4), ("technical", 0.39), ("other", 0.2)]
    );
    assert_eq!(
        response.choose::<Team>("route", 0.1).unwrap(),
        Outcome::Answered(Team::Technical)
    );
    let progress = response.score("progress").unwrap();
    assert_eq!(progress.probabilities, vec![0.01, 0.23, 0.67, 0.08]);
    assert_eq!(progress.mode(), Some(2));
    assert_eq!(progress.legend["0"], json!({"level": "none"}));
    assert_eq!(response.usage, JevUsage::default());
}

#[test]
fn detail_error_type_maps_to_a_jev_code_and_warns_once_when_unknown() {
    let (codes, warnings) = crate::test_support::unknown_warnings(|| {
        [
            JevErrorCode::from_detail("max_tokens_exceeded"),
            JevErrorCode::from_detail("quota_melted"),
            JevErrorCode::from_detail("quota_melted"),
        ]
    });
    assert_eq!(
        codes,
        [
            JevErrorCode::MaxTokensExceeded,
            JevErrorCode::Unknown,
            JevErrorCode::Unknown
        ]
    );
    assert_eq!(warnings, ["rust_type=JevErrorCode tag=quota_melted"]);
}

#[test]
fn unknown_answer_type_is_logged_once_without_its_payload() {
    let raw = json!({"type": "hologram_answer", "secret": "user text"});
    let (answers, warnings) = crate::test_support::unknown_warnings(|| {
        [
            super::answer::parse_answer(raw.clone()),
            super::answer::parse_answer(raw.clone()),
        ]
    });
    assert_eq!(
        answers,
        [Answer::Unknown(raw.clone()), Answer::Unknown(raw)]
    );
    assert_eq!(warnings, ["rust_type=Answer tag=hologram_answer"]);
    let debug = format!("{:?}", answers[0]);
    assert!(debug.contains("hologram_answer"), "{debug}");
    assert!(debug.contains("raw_bytes"), "{debug}");
    assert!(!debug.contains("user text"), "{debug}");
}

#[tokio::test]
async fn unknown_answer_type_is_kept_and_reported_at_extraction() {
    let (url, _) = serve(vec![ok(json!({
        "model": "jev-2.0.0",
        "answers": {
            "refund": {"type": "noul", "noul": 0.97},
            "order": {"type": "ranking", "ranking": ["a", "b"]}
        }
    }))])
    .await;
    let response = client(&url)
        .ask(
            "s",
            &Questions::new()
                .noul("refund", "Refund?")
                .noul("order", "Order?"),
        )
        .await
        .unwrap();
    assert_eq!(response.noul("refund").unwrap(), 0.97);
    assert_eq!(
        response.answers[1].answer,
        Answer::Unknown(json!({"type": "ranking", "ranking": ["a", "b"]}))
    );
    let error = response.noul("order").unwrap_err();
    assert!(error.to_string().contains("\"order\""), "{error}");
    assert!(error.to_string().contains("ranking"), "{error}");
    let error = response.noul("missing").unwrap_err();
    assert!(error.to_string().contains("\"missing\""), "{error}");
    let error = response.choice("refund").unwrap_err();
    assert!(
        error.to_string().contains("is a noul, not a choice"),
        "{error}"
    );
}

#[tokio::test]
async fn rate_limit_is_retried_after_retry_after() {
    let (url, recorded) = serve(vec![
        Canned::json(429, r#"{"detail":"slow down"}"#).with_header("Retry-After", "0"),
        ok(triage_response()),
    ])
    .await;
    let started = Instant::now();
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();
    // The wait is the longer of the backoff (at least 500 ms) and Retry-After: 0.
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(response.attempts, 2);
    let recorded = recorded.lock().unwrap();
    assert_eq!(header(&recorded[0], "x-typesafe-retry-count"), None);
    assert_eq!(header(&recorded[1], "x-typesafe-retry-count"), Some("1"));
    assert_eq!(recorded[1].body, TRIAGE_REQUEST);
}

#[tokio::test]
async fn long_retry_after_gives_up_at_once() {
    let (url, recorded) = serve(vec![
        Canned::json(429, r#"{"detail":"slow down"}"#).with_header("Retry-After", "61"),
        ok(triage_response()),
    ])
    .await;
    let started = Instant::now();
    let error = client(&url)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(error.status(), Some(429), "{error}");
    assert_eq!(error.retry_after(), Some(Duration::from_secs(61)));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn retry_after_past_the_budget_gives_up_at_once() {
    let (url, recorded) = serve(vec![
        Canned::json(503, r#"{"detail":"busy"}"#).with_header("Retry-After", "5"),
        ok(triage_response()),
    ])
    .await;
    let started = Instant::now();
    let error = client(&url)
        .with_retry_budget(Duration::from_secs(2))
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(error.status(), Some(503));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn server_errors_are_retried() {
    let (url, recorded) = serve(vec![
        Canned::json(503, r#"{"detail":"busy"}"#),
        Canned::json(529, r#"{"detail":"overloaded"}"#).with_header("retry-after-ms", "10"),
        ok(triage_response()),
    ])
    .await;
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();
    assert_eq!(response.attempts, 3);
    let recorded = recorded.lock().unwrap();
    assert_eq!(header(&recorded[2], "x-typesafe-retry-count"), Some("2"));
}

#[tokio::test]
async fn a_post_closed_before_any_reply_is_resent() {
    let (url, recorded) = serve(vec![
        Canned::json(200, "").closing_unanswered(),
        ok(triage_response()),
    ])
    .await;
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();
    assert_eq!(response.attempts, 2);
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn conflict_is_retried() {
    let (url, recorded) = serve(vec![
        Canned::json(409, r#"{"detail":"busy"}"#),
        ok(triage_response()),
    ])
    .await;
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();
    assert_eq!(response.attempts, 2);
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn error_status_with_a_cut_body_keeps_its_status_and_is_retried() {
    let cut = || {
        Canned::json(503, r#"{"detail":"#)
            .with_header("x-typesafe-request-id", "req_cut")
            .with_header("Retry-After", "1")
            .with_cut_body()
    };
    let (url, _) = serve(vec![cut()]).await;
    let error = client(&url)
        .with_max_retries(0)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(503), "{error:?}");
    assert!(error.is_retryable());
    assert_eq!(error.request_id(), Some("req_cut"));
    assert_eq!(error.retry_after(), Some(Duration::from_secs(1)));
    assert_eq!(error.message(), "HTTP 503 with no error message");
    assert!(error.source().is_some());

    let (url, recorded) = serve(vec![cut(), ok(triage_response())]).await;
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();
    assert_eq!(response.attempts, 2);
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn stalled_body_times_out_per_attempt() {
    let (url, recorded) = serve(vec![
        ok(triage_response()).with_stall(Duration::from_secs(3)),
    ])
    .await;
    let started = Instant::now();
    let error = client(&url)
        .with_timeout(Duration::from_millis(300))
        .with_max_retries(0)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Timeout), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unprocessable_detail_never_includes_input() {
    let body = json!({"detail": [
        {"type": "missing", "loc": ["body", "questions", "route", "criteria"],
         "msg": "Field required", "input": {"note": SECRET}},
        {"type": "too_short", "loc": ["body", "questions", 0], "msg": "Too short",
         "input": SECRET}
    ]});
    let (url, recorded) = serve(vec![Canned::json(422, body.to_string())]).await;
    let error = client(&url)
        .ask(
            json!({"note": SECRET}),
            &Questions::new().noul("q", "Is it?"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(422));
    let text = error.to_string();
    assert!(
        text.contains(
            "Jev API error: body.questions.route.criteria: Field required; \
             body.questions.0: Too short (HTTP 422"
        ),
        "{text}"
    );
    assert!(!text.contains(SECRET), "{text}");
    assert!(!format!("{error:?}").contains(SECRET));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unauthorized_json_is_an_authentication_error() {
    let body = r#"{"detail":{"error_type":"authentication_error","message":"Invalid API key"}}"#;
    let (url, recorded) = serve(vec![
        Canned::json(401, body).with_header("x-typesafe-request-id", "req_401"),
    ])
    .await;
    let error = client(&url)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(401), "{error}");
    // TypeSafe documents only `max_tokens_exceeded`, so this code is unknown.
    assert_eq!(error_code(&error), Some(JevErrorCode::Unknown));
    assert_eq!(error.request_id(), Some("req_401"));
    assert_eq!(
        error.to_string(),
        "Jev API error: Invalid API key (HTTP 401, request req_401)"
    );
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn forbidden_html_is_an_edge_block() {
    let html = "<html><body>Attention Required! | Cloudflare</body></html>";
    let (url, _) = serve(vec![
        Canned::json(403, html)
            .with_content_type("text/html")
            .with_header("cf-ray", "8f2a-IAD"),
    ])
    .await;
    let error = client(&url)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(403), "{error}");
    assert_eq!(error_code(&error), Some(JevErrorCode::EdgeBlocked));
    assert_eq!(error.request_id(), Some("8f2a-IAD"));
    let text = error.to_string();
    assert!(text.contains("cf-ray 8f2a-IAD"), "{text}");
    assert!(
        !text.contains("html") && !text.contains("Attention"),
        "{text}"
    );
}

#[tokio::test]
async fn bad_request_details_are_mapped() {
    let long = "x".repeat(1000);
    let (url, _) = serve(vec![
        Canned::json(
            400,
            r#"{"detail":{"error_type":"max_tokens_exceeded","message":"Prompt exceeds 8192 tokens"}}"#,
        ),
        Canned::json(
            400,
            r#"{"detail":{"error_type":"api_usage_error","message":"Unknown model jev-9"}}"#,
        ),
        Canned::json(400, json!({"detail": long}).to_string()),
        Canned::json(400, "plain text body"),
    ])
    .await;
    let jev = client(&url);
    let questions = Questions::new().noul("q", "Is it?");

    let error = jev.ask("s", &questions).await.unwrap_err();
    assert_eq!(error.status(), Some(400), "{error}");
    assert_eq!(error_code(&error), Some(JevErrorCode::MaxTokensExceeded));
    assert_eq!(
        error.to_string(),
        "Jev API error: input too long: Prompt exceeds 8192 tokens \
         (HTTP 400 max_tokens_exceeded)"
    );

    let error = jev.ask("s", &questions).await.unwrap_err();
    assert_eq!(error_code(&error), Some(JevErrorCode::Unknown));
    assert!(error.to_string().contains("Unknown model jev-9"), "{error}");

    let error = jev.ask("s", &questions).await.unwrap_err();
    assert_eq!(error.status(), Some(400), "{error:?}");
    assert_eq!(error.message(), "x".repeat(200));

    let error = jev.ask("s", &questions).await.unwrap_err();
    assert_eq!(error.status(), Some(400), "{error:?}");
    assert_eq!(error.message(), "HTTP 400 with no error message");
    assert_eq!(error_code(&error), None);
}

#[tokio::test]
async fn key_and_state_never_leak() {
    let (url, _) = serve(vec![
        ok(triage_response()),
        Canned::json(
            401,
            r#"{"detail":{"error_type":"authentication_error","message":"Invalid API key"}}"#,
        ),
        Canned::json(200, format!("not json {SECRET}")),
    ])
    .await;
    let jev = client(&url);
    assert!(!format!("{jev:?}").contains(KEY));
    let questions = Questions::new().noul("q", "Is it?");

    let response = jev.ask(json!({"note": SECRET}), &questions).await.unwrap();
    assert!(!format!("{response:?}").contains(SECRET));

    for _ in 0..2 {
        let error = jev
            .ask(json!({"note": SECRET}), &questions)
            .await
            .unwrap_err();
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(!text.contains(KEY), "{text}");
            assert!(!text.contains(SECRET), "{text}");
        }
    }
}

#[tokio::test]
async fn unparseable_success_reports_byte_count() {
    let body = format!("{{\"model\": \"{SECRET}\"");
    let (url, _) = serve(vec![
        Canned::json(200, body.clone()).with_header("x-typesafe-request-id", "req_9"),
    ])
    .await;
    let error = client(&url)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Decode), "{error:?}");
    assert_eq!(
        error.to_string(),
        format!(
            "Failed to parse Jev response ({} bytes): unexpected end of input at line 1 \
             column {} (request req_9)",
            body.len(),
            body.len()
        )
    );
}

#[test]
fn typed_extraction_gates_on_thresholds() {
    let response = parsed(json!({
        "model": "jev-1.13.0",
        "answers": {
            "yes": {"type": "noul", "noul": 0.94},
            "no": {"type": "noul", "noul": 0.05},
            "coin": {"type": "noul", "noul": 0.5},
            "route": {
                "type": "choice",
                "choice": "billing",
                "confidence": 0.81,
                "probabilities": {"billing": 0.87, "technical": 0.1, "other": 0.03}
            },
            "sales": {
                "type": "choice",
                "choice": "sales",
                "confidence": 0.9,
                "probabilities": {"sales": 0.9, "other": 0.1}
            },
            "urgency": {
                "type": "score",
                "score": 0.8,
                "confidence": 0.3,
                "legend": {},
                "probabilities": {"0": 0.4, "1": 0.4, "2": 0.2}
            },
            "too_high": {
                "type": "score",
                "score": 3.0,
                "confidence": 1.0,
                "probabilities": {"0": 0.0, "1": 0.0, "2": 0.0, "3": 1.0}
            }
        }
    }));

    assert_eq!(
        response.yes_no("yes", 0.8).unwrap(),
        Outcome::Answered(true)
    );
    match response.yes_no("yes", 0.9).unwrap() {
        Outcome::Unsure { best, confidence } => {
            assert!(best);
            assert!((confidence - 0.88).abs() < 1e-9);
        }
        other => panic!("expected unsure, got {other:?}"),
    }
    assert_eq!(
        response.yes_no("no", 0.8).unwrap(),
        Outcome::Answered(false)
    );
    assert_eq!(
        response.yes_no("coin", 0.1).unwrap(),
        Outcome::Unsure {
            best: true,
            confidence: 0.0
        }
    );
    assert_eq!(response.yes_no("coin", 0.0).unwrap().answered(), Some(true));

    assert_eq!(
        response.choose::<Team>("route", 0.81).unwrap(),
        Outcome::Answered(Team::Billing)
    );
    assert_eq!(
        response.choose::<Team>("route", 0.9).unwrap(),
        Outcome::Unsure {
            best: Team::Billing,
            confidence: 0.81
        }
    );
    assert_eq!(
        response.choose::<Team>("route", 0.9).unwrap().answered(),
        None
    );
    let error = response.choose::<Team>("sales", 0.5).unwrap_err();
    assert!(error.to_string().contains("\"sales\""), "{error}");

    assert_eq!(response.score("urgency").unwrap().mode(), Some(0));
    assert_eq!(
        response.rate::<Urgency>("urgency", 0.3).unwrap(),
        Outcome::Answered(Urgency::NoDeadline)
    );
    assert_eq!(
        response.rate::<Urgency>("urgency", 0.5).unwrap(),
        Outcome::Unsure {
            best: Urgency::NoDeadline,
            confidence: 0.3
        }
    );
    let error = response.rate::<Urgency>("too_high", 0.5).unwrap_err();
    assert!(error.to_string().contains("\"too_high\""), "{error}");
}

#[tokio::test]
async fn shortcuts_ask_one_question() {
    let answer = |value: Value| ok(json!({"model": "jev-1.13.0", "answers": {"answer": value}}));
    let (url, recorded) = serve(vec![
        answer(json!({
            "type": "choice", "choice": "technical", "confidence": 0.95,
            "probabilities": {"billing": 0.02, "technical": 0.97, "other": 0.01}
        })),
        answer(json!({"type": "noul", "noul": 0.2})),
        answer(json!({
            "type": "score", "score": 1.0, "confidence": 0.5,
            "probabilities": {"0": 0.2, "1": 0.6, "2": 0.2}
        })),
    ])
    .await;
    let jev = client(&url);
    assert_eq!(
        jev.choose::<Team>("The page crashes", "Which team?", 0.9)
            .await
            .unwrap(),
        Outcome::Answered(Team::Technical)
    );
    match jev
        .yes_no("The page crashes", "Refund requested?", 0.8)
        .await
        .unwrap()
    {
        Outcome::Unsure { best, confidence } => {
            assert!(!best);
            assert!((confidence - 0.6).abs() < 1e-9);
        }
        other => panic!("expected unsure, got {other:?}"),
    }
    assert_eq!(
        jev.rate::<Urgency>("The page crashes", "How urgent?", 0.5)
            .await
            .unwrap(),
        Outcome::Answered(Urgency::Week)
    );
    let recorded = recorded.lock().unwrap();
    let body: Value = serde_json::from_str(&recorded[0].body).unwrap();
    assert_eq!(
        body["questions"]["answer"]["criteria"],
        json!({
            "billing": "Charges, payments, and refunds",
            "technical": "Software faults",
            "other": "Anything else"
        })
    );
    let body: Value = serde_json::from_str(&recorded[2].body).unwrap();
    assert_eq!(
        body["questions"]["answer"]["criteria"],
        json!(["No deadline", "Within a week", "Today"])
    );
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let vars: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name| vars.get(name).cloned()
}

#[test]
fn env_config_requires_key_and_model() {
    let error = JevClient::from_env_with(env(&[])).unwrap_err();
    assert_eq!(validation_param(&error), Some("TYPESAFE_API_KEY"));
    let error = JevClient::from_env_with(env(&[
        ("TYPESAFE_API_KEY", "   "),
        ("TYPESAFE_DEFAULT_MODEL", "jev-1.13.0"),
    ]))
    .unwrap_err();
    assert_eq!(validation_param(&error), Some("TYPESAFE_API_KEY"));
    let error = JevClient::from_env_with(env(&[("TYPESAFE_API_KEY", KEY)])).unwrap_err();
    assert_eq!(validation_param(&error), Some("TYPESAFE_DEFAULT_MODEL"));
    let error = JevClient::from_env_with(env(&[
        ("TYPESAFE_API_KEY", KEY),
        ("TYPESAFE_DEFAULT_MODEL", ""),
    ]))
    .unwrap_err();
    assert_eq!(validation_param(&error), Some("TYPESAFE_DEFAULT_MODEL"));
    assert!(!error.to_string().contains(KEY));

    let jev = JevClient::from_env_with(env(&[
        ("TYPESAFE_API_KEY", KEY),
        ("TYPESAFE_DEFAULT_MODEL", "jev-1.13.0"),
        ("TYPESAFE_BASE_URL", " "),
    ]))
    .unwrap();
    assert_eq!(jev.model(), "jev-1.13.0");
    assert_eq!(jev.base_url, DEFAULT_BASE_URL);

    let jev = JevClient::from_env_with(env(&[
        ("TYPESAFE_API_KEY", KEY),
        ("TYPESAFE_DEFAULT_MODEL", "jev-1.13.0"),
        ("TYPESAFE_BASE_URL", "http://127.0.0.1:8080/"),
    ]))
    .unwrap();
    assert_eq!(jev.base_url, "http://127.0.0.1:8080");

    let error = JevClient::from_env_with(env(&[
        ("TYPESAFE_API_KEY", KEY),
        ("TYPESAFE_DEFAULT_MODEL", "jev-1.13.0"),
        ("TYPESAFE_BASE_URL", "http://api.typesafe.ai"),
    ]))
    .unwrap_err();
    assert_eq!(validation_param(&error), Some("TYPESAFE_BASE_URL"));
}

#[test]
fn constructor_checks_key_and_model() {
    for key in ["", "   ", "ts key", "ts-kéy", "ts-key\u{7f}"] {
        let error = JevClient::try_new(key, "jev-1.13.0").unwrap_err();
        assert_eq!(validation_param(&error), Some("api_key"), "{key:?}");
    }
    let error = JevClient::try_new(KEY, " ").unwrap_err();
    assert_eq!(validation_param(&error), Some("model"));
    let jev = JevClient::try_new(&format!("  {KEY}\n"), "jev-1.13.0").unwrap();
    assert_eq!(jev.model(), "jev-1.13.0");
}

#[test]
fn base_url_needs_https_or_loopback_http() {
    let jev = || JevClient::try_new(KEY, "jev-1.13.0").unwrap();
    for url in [
        "https://api.typesafe.ai/",
        "https://staging.example.com/jev",
        "http://127.0.0.1:9000",
        "http://localhost:9000/",
        "http://[::1]:9000",
    ] {
        let client = jev().with_base_url(url).unwrap();
        assert!(!client.base_url.ends_with('/'), "{url}");
    }
    assert_eq!(
        jev()
            .with_base_url("https://staging.example.com/jev/")
            .unwrap()
            .base_url,
        "https://staging.example.com/jev"
    );
    for url in [
        "http://api.typesafe.ai",
        "http://10.0.0.1",
        "ftp://127.0.0.1",
        "not a url",
        "https://user:pass@api.typesafe.ai",
        "https://api.typesafe.ai?x=1",
    ] {
        let error = jev().with_base_url(url).unwrap_err();
        assert_eq!(validation_param(&error), Some("base_url"), "{url}");
    }
}

#[test]
fn loopback_base_url_skips_proxies() {
    let jev = JevClient::try_new(KEY, "jev-1.13.0").unwrap();
    assert!(uses_proxies(&jev.http));
    let jev = jev.with_base_url("http://127.0.0.1:1").unwrap();
    assert!(!uses_proxies(&jev.http));
    let jev = JevClient::from_env_with(env(&[
        ("TYPESAFE_API_KEY", KEY),
        ("TYPESAFE_DEFAULT_MODEL", "jev-1.13.0"),
        ("TYPESAFE_BASE_URL", "http://127.0.0.1:1"),
    ]))
    .unwrap();
    assert!(!uses_proxies(&jev.http));
}

fn tool_use(input: Value) -> ToolUseBlock {
    ToolUseBlock::new("toolu_1", "jev", input)
}

fn tool_input() -> Value {
    json!({
        "state": {"message": MESSAGE},
        "questions": [
            {"name": "route", "type": "choice",
             "instructions": "Which team should handle the customer message?",
             "options": [
                {"label": "billing", "description": "Charges, payments, and refunds"},
                {"label": "technical", "description": "Software faults"},
                {"label": "other", "description": "Anything else"}
             ]},
            {"name": "urgency", "type": "score",
             "instructions": "How soon does the customer ask for action?",
             "levels": ["No deadline", "Within a week", "Today"]},
            {"name": "refund_requested", "type": "noul",
             "instructions": "Does the customer explicitly request a refund?"}
        ]
    })
}

fn result_text(result: &ToolResultBlock) -> &str {
    match &result.content {
        Some(ToolResultBlockContent::String(text)) => text,
        Some(ToolResultBlockContent::Array(items)) => match items.as_slice() {
            [Content::Text(text)] => &text.text,
            other => panic!("expected one text block, got {other:?}"),
        },
        None => panic!("tool result has no content"),
    }
}

fn tool(url: &str) -> JevTool {
    JevTool::new(client(url).with_max_retries(0))
}

#[test]
fn tool_definition_is_named_jev_and_closed() {
    let tool = JevTool::new(client("https://api.typesafe.ai"));
    assert_eq!(tool.name(), "jev");
    let ToolUnionParam::CustomTool(param) = tool.definition() else {
        panic!("expected a custom tool");
    };
    assert_eq!(param.name, "jev");
    assert_eq!(param.input_schema["additionalProperties"], json!(false));
    for variant in param.input_schema["properties"]["questions"]["items"]["oneOf"]
        .as_array()
        .unwrap()
    {
        assert_eq!(variant["additionalProperties"], json!(false));
    }
    let description = param.description.unwrap();
    assert!(
        description.contains("not whether it is correct"),
        "{description}"
    );
    assert!(description.contains("listed first"), "{description}");
}

#[tokio::test]
async fn tool_turns_input_into_compact_json() {
    let (url, recorded) = serve(vec![ok(triage_response())]).await;
    let tool = tool(&url);
    let tool: &dyn ClientTool = &tool;
    let result = tool.call(&tool_use(tool_input())).await.unwrap();
    assert_eq!(result.tool_use_id, "toolu_1");
    assert_eq!(result.is_error, None);
    assert_eq!(recorded.lock().unwrap()[0].body, TRIAGE_REQUEST);
    let text = result_text(&result);
    assert!(!text.contains('\n'));
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        json!({
            "model": "jev-1.13.0",
            "answers": {
                "route": {"type": "choice", "choice": "billing", "confidence": 1.0,
                          "probabilities": {"billing": 1.0, "technical": 0.0, "other": 0.0}},
                "urgency": {"type": "score", "score": 2.0, "level": "Today",
                            "confidence": 1.0, "probabilities": [0.0, 0.0, 1.0]},
                "refund_requested": {"type": "noul", "probability_yes": 0.99}
            }
        })
    );
}

#[tokio::test]
async fn tool_reports_fixable_problems_as_error_results() {
    let (url, recorded) = serve(vec![
        Canned::json(429, r#"{"detail":"slow down"}"#),
        Canned::json(500, r#"{"detail":"boom"}"#),
        Canned::json(413, r#"{"detail":"too large"}"#),
        Canned::json(403, "<html>blocked</html>").with_content_type("text/html"),
    ])
    .await;
    let tool = tool(&url);

    let bad_inputs = [
        json!({"questions": []}),
        json!({"state": "s", "questions": [
            {"name": "q", "type": "noul", "instructions": "?", "extra": 1}
        ]}),
        json!({"state": "s", "questions": [{"name": "q", "type": "vote", "instructions": "?"}]}),
        json!({"state": "s", "questions": [], "extra": true}),
        json!({"state": "s", "questions": []}),
        json!({"state": 5, "questions": [{"name": "q", "type": "noul", "instructions": "?"}]}),
        json!({"state": "s", "questions": [
            {"name": "c", "type": "choice", "instructions": "?", "options": [{"label": "only"}]}
        ]}),
    ];
    for input in bad_inputs {
        let result = tool.call(&tool_use(input.clone())).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{input}");
    }
    assert!(recorded.lock().unwrap().is_empty());

    let input =
        json!({"state": "s", "questions": [{"name": "q", "type": "noul", "instructions": "?"}]});
    for status in ["429", "500", "413", "403"] {
        let result = tool.call(&tool_use(input.clone())).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{status}");
        assert!(
            result_text(&result).contains(status),
            "{}",
            result_text(&result)
        );
    }
}

#[tokio::test]
async fn tool_reports_a_conflict_on_every_attempt_as_an_error_result() {
    let conflict = || Canned::json(409, r#"{"detail":"busy"}"#);
    let (url, recorded) = serve(vec![conflict(), conflict()]).await;
    let tool = JevTool::new(client(&url).with_max_retries(1));
    let result = tool.call(&tool_use(tool_input())).await.unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result_text(&result).contains("409"),
        "{}",
        result_text(&result)
    );
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn tool_returns_err_for_rejected_keys() {
    let (url, _) = serve(vec![
        Canned::json(
            401,
            r#"{"detail":{"error_type":"authentication_error","message":"Invalid API key"}}"#,
        ),
        Canned::json(
            403,
            r#"{"detail":{"error_type":"authentication_error","message":"Missing API key"}}"#,
        ),
    ])
    .await;
    let tool = tool(&url);
    let input =
        json!({"state": "s", "questions": [{"name": "q", "type": "noul", "instructions": "?"}]});
    let error = tool.call(&tool_use(input.clone())).await.unwrap_err();
    assert_eq!(error.status(), Some(401), "{error}");
    let error = tool.call(&tool_use(input)).await.unwrap_err();
    assert_eq!(error.status(), Some(403), "{error}");
}

#[tokio::test]
async fn attempt_timeout_is_capped_by_the_retry_budget() {
    let (url, recorded) = serve(vec![
        ok(triage_response()).with_stall(Duration::from_secs(3)),
    ])
    .await;
    let started = Instant::now();
    let error = client(&url)
        .with_timeout(Duration::from_secs(10))
        .with_retry_budget(Duration::from_millis(300))
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Timeout), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn blank_and_garbage_retry_after_fall_back_to_backoff() {
    let (url, recorded) = serve(vec![
        Canned::json(429, r#"{"detail":"slow down"}"#).with_header("Retry-After", ""),
        Canned::json(429, r#"{"detail":"slow down"}"#).with_header("Retry-After", "soon"),
        ok(triage_response()),
    ])
    .await;
    let started = Instant::now();
    let response = client(&url)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap();
    // Backoff floors are 500 ms, then 1 s.
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(response.attempts, 3);
    assert_eq!(recorded.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn connection_refused_is_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let started = Instant::now();
    let error = client(&url)
        .with_max_retries(1)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Connection), "{error}");
    // One backoff of at least 500 ms means a second attempt was made.
    assert!(started.elapsed() >= Duration::from_millis(500));
}

#[tokio::test]
async fn request_timeout_status_is_retried() {
    let (url, recorded) = serve(vec![
        Canned::json(408, r#"{"detail":"request timeout"}"#),
        ok(triage_response()),
    ])
    .await;
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();
    assert_eq!(response.attempts, 2);
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn body_timeout_keeps_the_request_id() {
    let stalled = || {
        ok(triage_response())
            .with_header("x-typesafe-request-id", "req_stall")
            .with_stall(Duration::from_secs(3))
    };
    let (url, _) = serve(vec![stalled()]).await;
    let error = client(&url)
        .with_timeout(Duration::from_millis(300))
        .with_max_retries(0)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Timeout), "{error}");
    assert_eq!(error.request_id(), Some("req_stall"));

    // A fresh server: the first one is still stalling on the first connection.
    let (url, _) = serve(vec![stalled()]).await;
    let tool = JevTool::new(
        client(&url)
            .with_timeout(Duration::from_millis(300))
            .with_max_retries(0),
    );
    let input =
        json!({"state": "s", "questions": [{"name": "q", "type": "noul", "instructions": "?"}]});
    let result = tool.call(&tool_use(input)).await.unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result_text(&result).contains("req_stall"),
        "{}",
        result_text(&result)
    );
}

#[test]
fn malformed_known_answer_is_reported_per_question() {
    let response = parsed(json!({
        "model": "jev-1.13.0",
        "answers": {
            "refund": {"type": "noul", "noul": 0.97},
            "bad_noul": {"type": "noul", "noul": SECRET},
            "bad_choice": {"type": "choice", "choice": "billing"},
            "bad_score": {"type": "score", "score": 1.0, "confidence": 0.5,
                          "probabilities": {"0": 0.5, "7": 0.5}},
            "null_legend": {"type": "score", "score": 1.0, "confidence": 0.5, "legend": null,
                            "probabilities": {"0": 0.2, "1": 0.8}},
            "other": {"type": "ranking", "ranking": [SECRET]}
        }
    }));
    assert_eq!(response.noul("refund").unwrap(), 0.97);
    assert!(response.score("null_legend").unwrap().legend.is_empty());
    for (name, kind, read) in [
        ("bad_noul", "noul", response.noul("bad_noul").map(|_| ())),
        (
            "bad_choice",
            "choice",
            response.choice("bad_choice").map(|_| ()),
        ),
        (
            "bad_score",
            "score",
            response.score("bad_score").map(|_| ()),
        ),
    ] {
        let error = read.unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("the {kind} answer for {name:?} could not be read")),
            "{error}"
        );
        assert!(!error.to_string().contains(SECRET), "{error}");
    }
    assert!(matches!(
        &response.answers[1].answer,
        Answer::Malformed {
            kind: AnswerKind::Noul,
            ..
        }
    ));
    let error = response.noul("other").unwrap_err();
    assert!(error.to_string().contains("does not know"), "{error}");
    let debug = format!("{response:?}");
    assert!(!debug.contains(SECRET), "{debug}");
    assert!(debug.contains("raw_bytes"), "{debug}");
}

#[tokio::test]
async fn typesafe_request_id_wins_over_cf_ray() {
    let (url, _) = serve(vec![
        ok(triage_response())
            .with_header("cf-ray", "8f2a-IAD")
            .with_header("x-typesafe-request-id", "req_both"),
    ])
    .await;
    let response = client(&url)
        .ask(json!({"message": MESSAGE}), &triage_questions())
        .await
        .unwrap();
    assert_eq!(response.request_id.as_deref(), Some("req_both"));
}

#[tokio::test]
async fn tool_error_text_never_contains_the_state() {
    let (url, _) = serve(vec![
        Canned::json(500, r#"{"detail":"boom"}"#),
        ok(triage_response()).with_stall(Duration::from_secs(3)),
    ])
    .await;
    let tool = JevTool::new(
        client(&url)
            .with_timeout(Duration::from_millis(300))
            .with_max_retries(0),
    );
    let noul = json!([{"name": "q", "type": "noul", "instructions": "?"}]);
    let inputs = [
        // Validation failure before sending.
        json!({"state": {"note": SECRET}, "questions": []}),
        // Input that does not decode.
        json!({"state": SECRET, "questions": noul, "extra": SECRET.len()}),
        // HTTP 500, then a body timeout.
        json!({"state": {"note": SECRET}, "questions": noul}),
        json!({"state": {"note": SECRET}, "questions": noul}),
    ];
    for input in inputs {
        let result = tool.call(&tool_use(input.clone())).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{input}");
        let text = result_text(&result);
        assert!(!text.contains(SECRET), "{text}");
    }
}

#[test]
fn tool_debug_hides_the_key() {
    let tool = JevTool::new(client("https://api.typesafe.ai"));
    let debug = format!("{tool:?}");
    assert!(!debug.contains(KEY), "{debug}");
    assert!(debug.contains("[REDACTED]"), "{debug}");
}

#[test]
fn tool_definition_pins_option_objects_and_state_types() {
    let tool = JevTool::new(client("https://api.typesafe.ai"));
    let ToolUnionParam::CustomTool(param) = tool.definition() else {
        panic!("expected a custom tool");
    };
    let schema = &param.input_schema;
    assert_eq!(
        schema["properties"]["state"]["type"],
        json!(["string", "object", "array"])
    );
    let choice = &schema["properties"]["questions"]["items"]["oneOf"][1];
    assert_eq!(choice["properties"]["type"]["enum"], json!(["choice"]));
    let option = &choice["properties"]["options"]["items"];
    assert_eq!(option["type"], json!("object"));
    assert_eq!(option["additionalProperties"], json!(false));
    assert_eq!(option["required"], json!(["label"]));
}

fn noul_input() -> Value {
    json!({"state": "s", "questions": [{"name": "q", "type": "noul", "instructions": "?"}]})
}

#[tokio::test]
async fn body_over_the_cap_is_request_too_large() {
    let (url, recorded) = serve(vec![]).await;
    let limit = 32 * 1024 * 1024;
    let error = client(&url)
        .ask(
            Value::String("x".repeat(limit)),
            &Questions::new().noul("q", "Is it?"),
        )
        .await
        .unwrap_err();
    match error.kind() {
        ErrorKind::RequestTooLarge {
            size,
            limit: actual,
            ..
        } => {
            assert!(*size > limit, "{size}");
            assert_eq!(*actual, limit);
        }
        other => panic!("expected RequestTooLarge, got {other:?}"),
    }
    assert!(!error.is_retryable());
    assert!(
        error.to_string().contains("over the 32 MiB limit"),
        "{error}"
    );
    assert!(recorded.lock().unwrap().is_empty());
}

#[test]
fn failed_request_serialization_keeps_the_serde_error() {
    struct Unserializable;
    impl serde::Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(SECRET))
        }
    }
    let error = super::client::serialize_body(&Unserializable).unwrap_err();
    assert!(
        matches!(error.kind(), ErrorKind::Validation { .. }),
        "{error:?}"
    );
    // The cause is only in `source()`, never copied into the message.
    assert_eq!(error.to_string(), "Failed to serialize the Jev request");
    let source = error.source().expect("the serde error is the source");
    assert!(source.to_string().contains(SECRET), "{source}");
}

#[test]
fn fatal_split_by_kind_and_status() {
    let api = |status: u16, code: Option<JevErrorCode>| {
        Error::http_status(
            Service::Jev,
            status,
            Some("m".to_string()),
            None,
            code.map(ErrorCode::Jev),
            None,
            None,
        )
    };
    for status in [400, 408, 409, 413, 422, 429, 500, 502, 503, 529, 599] {
        assert!(!super::tool::is_fatal(&api(status, None)), "{status}");
    }
    assert!(!super::tool::is_fatal(&api(
        403,
        Some(JevErrorCode::EdgeBlocked)
    )));
    for status in [401, 402, 403, 404, 405] {
        assert!(super::tool::is_fatal(&api(status, None)), "{status}");
    }
    assert!(super::tool::is_fatal(&api(
        403,
        Some(JevErrorCode::Unknown)
    )));
    let others = [
        Error::api(Service::Jev, None, None, "in stream", None),
        Error::new(ErrorKind::Timeout, "t"),
        Error::new(ErrorKind::Connection, "c"),
        Error::new(ErrorKind::Interrupted, "i").with_request_id(Some("req".to_string())),
        Error::new(ErrorKind::Decode, "d"),
        Error::new(
            ErrorKind::RequestTooLarge { size: 2, limit: 1 },
            "too large",
        ),
        Error::validation("v", None),
    ];
    for error in others {
        assert!(!super::tool::is_fatal(&error), "{error:?}");
    }
}

// Answers with headers and a short body, then closes, `connections` times at most, counting
// the connections it saw.
async fn serve_truncated(request_id: &str) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nx-typesafe-request-id: {request_id}\r\n\
         Content-Length: 1000\r\nConnection: close\r\n\r\n{{\"model\":"
    );
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            crate::test_support::read_request(&mut socket).await;
            tokio::io::AsyncWriteExt::write_all(&mut socket, head.as_bytes())
                .await
                .unwrap();
            // Dropping the socket closes it with 990 bytes missing.
        }
    });
    (url, seen)
}

#[tokio::test]
async fn body_read_failure_is_interrupted_and_not_resent() {
    let (url, seen) = serve_truncated("req_cut").await;
    let error = client(&url)
        .ask("s", &Questions::new().noul("q", "Is it?"))
        .await
        .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Interrupted), "{error:?}");
    assert_eq!(error.request_id(), Some("req_cut"));
    assert!(error.is_retryable());
    assert!(error.source().is_some());
    // A POST may already have been done and billed, so it is never resent automatically.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(seen.load(Ordering::SeqCst), 1);

    // For the tool a cut-off body is a problem the model may retry, not a developer error.
    let (url, _) = serve_truncated("req_cut").await;
    let result = tool(&url)
        .call(&tool_use(noul_input()))
        .await
        .expect("an interrupted call is not fatal");
    assert_eq!(result.is_error, Some(true));
    assert!(
        result_text(&result).contains("req_cut"),
        "{}",
        result_text(&result)
    );
}

#[test]
fn unparseable_success_keeps_category_line_and_column() {
    let body = format!("{{\n  \"model\": \"{SECRET}\",\n  \"answers\": 5}}");
    let error = JevResponse::parse(
        Bytes::new(),
        Bytes::from(body.clone()),
        Some("req_7".into()),
        1,
    )
    .unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Decode), "{error:?}");
    assert_eq!(error.request_id(), Some("req_7"));
    let text = error.to_string();
    assert!(
        text.contains("type or value mismatch at line 3 column"),
        "{text}"
    );
    assert!(
        text.contains(&format!("({} bytes)", body.len()))
            || text.contains(&format!("{} bytes", body.len())),
        "{text}"
    );
    assert!(!text.contains(SECRET), "{text}");
    assert!(!format!("{error:?}").contains(SECRET), "{error:?}");
    assert!(error.source().is_none());
}

#[test]
fn malformed_answer_logs_its_shape_not_its_values() {
    let (response, warnings) = warnings_on("topcoat_si::jev", || {
        parsed(json!({
            "model": "jev-1.13.0",
            "answers": {"bad": {"type": "noul", "noul": SECRET, "extra": 1}}
        }))
    });
    assert!(matches!(
        response.answers[0].answer,
        Answer::Malformed { .. }
    ));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let line = &warnings[0];
    assert!(line.contains("kind=noul"), "{line}");
    assert!(line.contains("raw_bytes="), "{line}");
    assert!(
        line.contains("object with keys type, noul, extra"),
        "{line}"
    );
    assert!(!line.contains(SECRET), "{line}");

    // A hostile object cannot flood the log.
    let wide: serde_json::Map<String, Value> = (0..100)
        .map(|i| (format!("{i}{}", "k".repeat(100)), json!(1)))
        .chain([("type".to_string(), json!("noul"))])
        .collect();
    let (_, warnings) = warnings_on("topcoat_si::jev", || {
        parsed(json!({"model": "m", "answers": {"bad": Value::Object(wide)}}))
    });
    assert!(warnings[0].len() < 400, "{}", warnings[0]);
}
