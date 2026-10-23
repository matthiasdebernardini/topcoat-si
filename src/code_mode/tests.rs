#![cfg(test)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use monty_pool::PoolConfig;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use super::pack::untyped;
use super::tool::{CANCELLED, RESTARTED, TOOL_NAME, definition};
use super::*;
use crate::client_tool::ClientTool;
use crate::error::{Error, ErrorKind};
use crate::firecrawl::FirecrawlClient;
use crate::jev::JevClient;
use crate::test_support::{Canned, read_request, serve};
use crate::types::{KnownModel, MessageCreateParams, MessageParam, Model, ToolResultBlockContent};
use crate::types::{ToolResultBlock, ToolUnionParam, ToolUseBlock};
use crate::{Anthropic, FirecrawlFetchPage, FirecrawlWebSearch};

async fn test_pool() -> Arc<Pool> {
    let monty = std::env::var("MONTY_BIN").unwrap_or_else(|_| {
        panic!(
            "set MONTY_BIN to the monty binary from \
             `cargo install monty-runtime --version 1.2.0 --locked`"
        )
    });
    pool(monty, 2).await.unwrap()
}

/// Runs one snippet and returns the tool result text and its `is_error` flag.
async fn run(code: &CodeMode, snippet: &str) -> (String, bool) {
    let tool_use = ToolUseBlock::new("toolu_1", TOOL_NAME, json!({"code": snippet}));
    let result = code.call(&tool_use).await.unwrap();
    (text(&result), result.is_error == Some(true))
}

fn text(result: &ToolResultBlock) -> String {
    match &result.content {
        Some(ToolResultBlockContent::String(text)) => text.clone(),
        other => panic!("expected string content, got {other:?}"),
    }
}

fn used(code: &CodeMode) -> usize {
    code.host_calls.load(Ordering::SeqCst)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
}

/// A `search` fake answering one hit per query and counting its calls.
fn counted_search(calls: Arc<AtomicUsize>) -> impl HostFunction {
    host_fn(
        "search",
        "search(*, query: str, limit: int = 5) -> list[dict]",
        "fake search",
        move |args: SearchArgs| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(
                    json!([{"url": format!("https://example.com/{}", args.query), "limit": args.limit}]),
                )
            }
        },
    )
}

fn host(
    name: &'static str,
    function: impl Fn(Value) -> Result<Value, HostError> + Send + Sync + 'static,
) -> impl HostFunction {
    let function = Arc::new(function);
    host_fn(
        name,
        format!("{name}(**kwargs) -> object"),
        "test function",
        move |args: Value| {
            let function = function.clone();
            async move { function(args) }
        },
    )
}

fn fake_firecrawl() -> Pack {
    Pack::new("firecrawl", "fake")
        .with_host(host("search", |_| {
            Ok(json!([
                {"url": "https://example.com/a", "title": "A", "markdown": "about async"},
                {"url": "https://example.com/b", "title": "B", "markdown": "about cooking"},
                {"url": "https://example.com/c", "title": "C", "markdown": "unratable"}
            ]))
        }))
        .with_host(host("scrape", |args| {
            let url = args["url"].as_str().unwrap_or_default();
            if url.ends_with("missing") {
                return Err(HostError::Raise {
                    kind: PyError::RuntimeError,
                    message: "not found".to_string(),
                });
            }
            Ok(json!({"url": url, "title": "Page A"}))
        }))
}

/// Jev fakes: the last option or level at 0.9 confidence, unsure for example.com/b, and a
/// `RuntimeError` for example.com/c or the text "broken".
fn fake_jev() -> Pack {
    let broken = |args: &Value| {
        let state = &args["state"];
        (state["url"] == "https://example.com/c" || state["text"] == "broken").then(|| {
            HostError::Raise {
                kind: PyError::RuntimeError,
                message: "jev failed".to_string(),
            }
        })
    };
    Pack::new("jev", "fake")
        .with_host(host("choose", move |args| {
            if let Some(error) = broken(&args) {
                return Err(error);
            }
            let options = args["options"].as_array().cloned().unwrap_or_default();
            Ok(json!({"answer": options.last(), "confidence": 0.9}))
        }))
        .with_host(host("rate", move |args| {
            if let Some(error) = broken(&args) {
                return Err(error);
            }
            let levels = args["levels"].as_array().cloned().unwrap_or_default();
            if args["state"]["url"] == "https://example.com/b" {
                return Ok(json!({"answer": null, "confidence": 0.4}));
            }
            Ok(json!({"answer": levels.last(), "confidence": 0.9}))
        }))
}

fn firecrawl_client(url: &str) -> FirecrawlClient {
    FirecrawlClient::try_new("fc-test-key")
        .unwrap()
        .with_base_url(url)
        .unwrap()
        .with_max_retries(0)
}

fn slow() -> impl HostFunction {
    host_fn("slow", "slow() -> None", "sleeps", |_: Value| async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(Value::Null)
    })
}

/// Pauses the test runtime's clock until dropped, so a sleep ends at once when nothing else runs.
struct PausedClock;

impl PausedClock {
    fn new() -> Self {
        tokio::time::pause();
        Self
    }
}

impl Drop for PausedClock {
    fn drop(&mut self) {
        tokio::time::resume();
    }
}

/// A `nap` host function sleeping `secs` on a paused clock: the call timeouts see the full
/// sleep, the test does not wait for it.
fn nap(secs: u64) -> impl HostFunction {
    host_fn("nap", "nap() -> None", "sleeps", move |_: Value| async move {
        let _clock = PausedClock::new();
        tokio::time::sleep(Duration::from_secs(secs)).await;
        Ok(Value::Null)
    })
}

fn firecrawl_pack(url: &str, search: FirecrawlWebSearch, max_credits: u32) -> Pack {
    packs::firecrawl(
        search,
        FirecrawlFetchPage::new(firecrawl_client(url)),
        max_credits,
    )
}

fn ok(body: Value) -> Canned {
    Canned::json(200, body.to_string())
}

fn search_response() -> Canned {
    ok(json!({
        "success": true,
        "creditsUsed": 2,
        "data": {"web": [
            {"url": "https://example.com/a", "title": "A", "description": "first"},
            {"url": "https://example.com/b", "title": "B", "description": "second"}
        ]}
    }))
}

// 1. Every built-in pack passes check_pack.

#[tokio::test]
async fn reports_pack_examples_pass() {
    testing::check_pack(test_pool().await, packs::reports, Vec::new)
        .await
        .unwrap();
}

#[tokio::test]
async fn research_pack_examples_pass() {
    testing::check_pack(test_pool().await, packs::research, || {
        vec![fake_firecrawl(), fake_jev()]
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn triage_pack_examples_pass() {
    testing::check_pack(test_pool().await, packs::triage, || vec![fake_jev()])
        .await
        .unwrap();
}

#[tokio::test]
async fn firecrawl_pack_examples_pass() {
    let (url, _) = serve(vec![
        search_response(),
        ok(json!({
            "success": true,
            "data": {"markdown": "# A", "metadata": {
                "title": "Page A", "url": "https://example.com/a", "statusCode": 200
            }}
        })),
    ])
    .await;
    let search = FirecrawlWebSearch::new(firecrawl_client(&url));
    testing::check_pack(
        test_pool().await,
        || firecrawl_pack(&url, search.clone(), 100),
        Vec::new,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn jev_pack_examples_pass() {
    let answer = |value: Value| ok(json!({"model": "jev-1.13.0", "answers": {"answer": value}}));
    let (url, _) = serve(vec![
        answer(json!({"type": "noul", "noul": 0.99})),
        answer(json!({
            "type": "choice", "choice": "technical", "confidence": 0.95,
            "probabilities": {"billing": 0.05, "technical": 0.95}
        })),
        answer(json!({
            "type": "score", "score": 1.0, "confidence": 0.9,
            "probabilities": {"0": 0.05, "1": 0.95}
        })),
        ok(json!({"model": "jev-1.13.0", "answers": {"refund": {"type": "noul", "noul": 0.99}}})),
    ])
    .await;
    let client = JevClient::try_new("ts-key", "jev-1.13.0")
        .unwrap()
        .with_base_url(&url)
        .unwrap();
    testing::check_pack(test_pool().await, || packs::jev(client.clone()), Vec::new)
        .await
        .unwrap();
}

// 2. A run_tools run whose script gathers search and rate.

fn reply(stop_reason: &str, content: Value) -> Canned {
    ok(json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-5-5",
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {"input_tokens": 3, "output_tokens": 2}
    }))
}

/// A host function that sleeps, recording when it started and ended.
fn timed(name: &'static str, spans: Arc<Mutex<Vec<(Instant, Instant)>>>) -> impl HostFunction {
    host_fn(
        name,
        format!("{name}(**kwargs) -> str"),
        "timed fake",
        move |_: Value| {
            let spans = spans.clone();
            async move {
                let start = Instant::now();
                tokio::time::sleep(Duration::from_millis(300)).await;
                spans.lock().unwrap().push((start, Instant::now()));
                Ok(json!(name))
            }
        },
    )
}

#[tokio::test]
async fn run_tools_gathers_host_calls_concurrently() {
    let script = "hits, level = await asyncio.gather(search(query=\"x\"), \
                  rate(state=\"s\", question=\"q\", levels=[\"a\", \"b\"]))\n[hits, level]";
    let (base_url, recorded) = serve(vec![
        reply(
            "tool_use",
            json!([{"type": "tool_use", "id": "toolu_1", "name": "python", "input": {"code": script}}]),
        ),
        reply("end_turn", json!([{"type": "text", "text": "Done."}])),
    ])
    .await;
    let spans = Arc::new(Mutex::new(Vec::new()));
    let pack = Pack::new("fakes", "timed fakes")
        .with_host(timed("search", spans.clone()))
        .with_host(timed("rate", spans.clone()));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let client = Anthropic::try_new(Some("test_key".to_string()))
        .unwrap()
        .with_base_url(base_url)
        .unwrap()
        .with_max_retries(0);
    let params = MessageCreateParams::new(
        256,
        vec![MessageParam::user("question")],
        Model::Known(KnownModel::ClaudeSonnet55),
    );
    let run = client.run_tools(params, &[&code], 5).await.unwrap();
    assert_eq!(run.responses.len(), 2);

    let spans = spans.lock().unwrap().clone();
    assert_eq!(spans.len(), 2);
    let (first, second) = (spans[0], spans[1]);
    assert!(
        first.0 < second.1 && second.0 < first.1,
        "calls did not overlap"
    );

    let body: Value = serde_json::from_str(&recorded.lock().unwrap()[1].body).unwrap();
    let result = &body["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["content"], "=> ['search', 'rate']");
    assert!(
        body["tools"][0]["description"]
            .as_str()
            .unwrap()
            .contains("\n\nfakes: timed fakes\n  search(**kwargs) -> str: timed fake")
    );
    code.finish().await.unwrap();
}

// 3. Errors.

#[tokio::test]
async fn misspelled_function_raises_name_error_without_host_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let pack = Pack::new("fakes", "fakes").with_host(counted_search(calls.clone()));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let (text, is_error) = run(&code, "await serch(query=\"x\")").await;
    assert!(is_error);
    assert!(text.contains("NameError"), "{text}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(used(&code), 0);
}

#[tokio::test]
async fn unknown_keyword_raises_type_error_and_costs_nothing() {
    let calls = Arc::new(AtomicUsize::new(0));
    let pack = Pack::new("fakes", "fakes").with_host(counted_search(calls.clone()));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let (text, is_error) = run(&code, "await search(query=\"x\", limt=3)").await;
    assert!(is_error);
    assert!(
        text.contains("TypeError: search(): unknown field `limt`"),
        "{text}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(used(&code), 0);

    let (text, is_error) = run(&code, "await search(\"x\")").await;
    assert!(is_error);
    assert!(
        text.contains("TypeError: search() takes keyword arguments only"),
        "{text}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn raised_value_error_is_catchable() {
    let pack = Pack::new("fakes", "fakes").with_host(host("check", |_| {
        Err(HostError::Raise {
            kind: PyError::ValueError,
            message: "bad value".to_string(),
        })
    }));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "try:\n    await check()\nexcept ValueError as e:\n    caught = str(e)\ncaught";
    assert_eq!(
        run(&code, snippet).await,
        ("=> 'bad value'".to_string(), false)
    );
}

#[tokio::test]
async fn slow_host_function_raises_runtime_error_and_keeps_the_session() {
    let pack = Pack::new("fakes", "fakes").with_host(slow());
    let mut code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    code.host_timeout = Duration::from_millis(200);
    let (text, is_error) = run(&code, "kept = 7\nawait slow()").await;
    assert!(is_error);
    assert!(
        text.contains("RuntimeError: slow() timed out after 200ms"),
        "{text}"
    );
    assert!(!text.contains(RESTARTED), "{text}");
    assert_eq!(run(&code, "kept").await, ("=> 7".to_string(), false));
}

#[tokio::test]
async fn fatal_host_error_makes_run_tools_fail() {
    let (base_url, _) = serve(vec![reply(
        "tool_use",
        json!([{"type": "tool_use", "id": "toolu_1", "name": "python", "input": {"code": "await boom()"}}]),
    )])
    .await;
    let pack = Pack::new("fakes", "fakes").with_host(host("boom", |_| {
        Err(HostError::Fatal(Error::tool("rejected key", None)))
    }));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let client = Anthropic::try_new(Some("test_key".to_string()))
        .unwrap()
        .with_base_url(base_url)
        .unwrap()
        .with_max_retries(0);
    let params = MessageCreateParams::new(
        256,
        vec![MessageParam::user("question")],
        Model::Known(KnownModel::ClaudeSonnet55),
    );
    let error = client.run_tools(params, &[&code], 5).await.unwrap_err();
    assert_eq!(error.error.kind(), &ErrorKind::Tool, "{error}");
    assert!(code.session.lock().await.is_none());
}

#[tokio::test]
async fn budget_runs_out_at_call_51_for_sequential_awaits() {
    let pack = Pack::new("fakes", "fakes").with_host(host("ping", |_| Ok(json!(1))));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "n = 0\nmessage = None\nfor i in range(51):\n    try:\n        n += await ping()\n    \
                   except RuntimeError as e:\n        message = str(e)\n(n, message)";
    assert_eq!(
        run(&code, snippet).await,
        (
            "=> (50, 'host call budget of 50 used up for this conversation; finish with what \
             you have')"
                .to_string(),
            false
        )
    );
}

// Monty 1.2.0 never reports a stale call in `ResolveFutures`: awaiting it stalls its scheduler,
// which raises its own `RuntimeError`, so the driver's "call it again" answer is not reached.
#[tokio::test]
async fn call_from_an_earlier_snippet_raises_and_costs_nothing() {
    let pack = Pack::new("fakes", "fakes").with_host(host("ping", |_| Ok(json!(1))));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let (_, is_error) = run(&code, "pending = ping()").await;
    assert!(!is_error);
    let (text, is_error) = run(&code, "await pending").await;
    assert!(is_error);
    assert!(
        text.ends_with(
            "RuntimeError: Internal error in monty: asyncio scheduler stalled: no ready tasks \
             and no pending external calls"
        ),
        "{text}"
    );
    assert!(!text.contains(RESTARTED), "{text}");
    assert_eq!(used(&code), 0);
    assert_eq!(
        run(&code, "await ping()").await,
        ("=> 1".to_string(), false)
    );
}

// 4. Session.

#[tokio::test]
async fn variables_persist_across_calls_and_errors() {
    let code = CodeMode::try_new(test_pool().await, vec![], Limits::default()).unwrap();
    assert_eq!(run(&code, "x = 1").await, ("=> None".to_string(), false));
    assert_eq!(run(&code, "x + 1").await, ("=> 2".to_string(), false));
    let (text, is_error) = run(&code, "y = 5\nraise ValueError('boom')").await;
    assert!(is_error);
    assert!(text.contains("ValueError: boom"), "{text}");
    assert_eq!(run(&code, "y").await, ("=> 5".to_string(), false));
    assert_eq!(
        run(&code, "x = 'text'\nx.upper()").await,
        ("=> 'TEXT'".to_string(), false)
    );
    code.finish().await.unwrap();
}

#[tokio::test]
async fn memory_limit_restarts_the_session() {
    let limits = Limits {
        max_memory: 16 * 1024 * 1024,
        ..Limits::default()
    };
    let code = CodeMode::try_new(test_pool().await, vec![], limits).unwrap();
    assert_eq!(run(&code, "kept = 1").await, ("=> None".to_string(), false));
    let (text, is_error) = run(&code, "big = 'a' * (64 * 1024 * 1024)\nlen(big)").await;
    assert!(is_error);
    assert!(text.contains("MemoryError"), "{text}");
    assert!(text.ends_with(RESTARTED), "{text}");
    let (text, is_error) = run(&code, "kept").await;
    assert!(is_error);
    assert!(text.contains("NameError"), "{text}");
}

#[tokio::test]
async fn cancelled_call_leaves_a_fresh_working_session() {
    let pack = Pack::new("fakes", "fakes").with_host(slow());
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    assert_eq!(run(&code, "kept = 1").await, ("=> None".to_string(), false));
    let tool_use = ToolUseBlock::new("toolu_1", TOOL_NAME, json!({"code": "await slow()"}));
    let cancelled = tokio::time::timeout(Duration::from_millis(300), code.call(&tool_use)).await;
    assert!(cancelled.is_err());

    assert_eq!(
        run(&code, "1 + 1").await,
        (
            format!("The last python call was cancelled before it finished. {RESTARTED}\n=> 2"),
            false
        )
    );
    assert_eq!(run(&code, "2 + 2").await, ("=> 4".to_string(), false));
    let (text, _) = run(&code, "kept").await;
    assert!(text.contains("NameError"), "{text}");
}

// 5. Validation.

#[tokio::test]
async fn invalid_packs_fail_in_try_new() {
    let pool = test_pool().await;
    let ping = || host("ping", |_| Ok(json!(1)));
    let duplicate = CodeMode::try_new(
        pool.clone(),
        vec![
            Pack::new("a", "a").with_host(ping()),
            Pack::new("b", "b").with_python_fn("ping", "def ping()", "again"),
        ],
        Limits::default(),
    );
    assert!(
        duplicate
            .err()
            .unwrap()
            .to_string()
            .contains("two functions are named ping")
    );

    let missing = CodeMode::try_new(
        pool.clone(),
        vec![Pack::new("a", "a").requires(["jev"])],
        Limits::default(),
    );
    assert!(
        missing
            .err()
            .unwrap()
            .to_string()
            .contains("requires pack jev")
    );

    let help = CodeMode::try_new(
        pool,
        vec![Pack::new("a", "a").with_host(host("help", |_| Ok(json!(1))))],
        Limits::default(),
    );
    assert!(help.err().unwrap().to_string().contains("reserved"));
}

#[tokio::test]
async fn max_output_chars_under_the_result_reserve_is_rejected() {
    let mut config = PoolConfig::subprocess("monty");
    config.min_processes = 0;
    let idle = Arc::new(Pool::new(config).await.unwrap());
    let limits = Limits {
        max_output_chars: 1000,
        ..Limits::default()
    };
    let error = CodeMode::try_new(idle, vec![], limits).err().unwrap();
    assert!(
        matches!(
            error.kind(),
            ErrorKind::Validation { param: Some(param) } if param == "max_output_chars"
        ),
        "{error}"
    );
}

// 6. Values.

#[tokio::test]
async fn values_cross_both_ways() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let pack = Pack::new("fakes", "fakes").with_host(host("echo", move |args| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(args)
    }));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    assert_eq!(
        run(
            &code,
            "await echo(a=None, b=True, c=3, d=1.5, e='s', f=[1, (2, 3)], g={'k': [1]})"
        )
        .await,
        (
            "=> {'a': None, 'b': True, 'c': 3, 'd': 1.5, 'e': 's', 'f': [1, [2, 3]], 'g': \
             {'k': [1]}}"
                .to_string(),
            false
        )
    );
    for (snippet, expected) in [
        ("await echo(a=2**70)", "ValueError"),
        ("await echo(a=float('nan'))", "ValueError"),
        (
            "await echo(a={1: 2})",
            "TypeError: dict keys must be str, got int",
        ),
        ("await echo(a={1, 2})", "TypeError: cannot pass a set"),
    ] {
        let (text, is_error) = run(&code, snippet).await;
        assert!(is_error, "{snippet}");
        assert!(text.contains(expected), "{snippet}: {text}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(run(&code, "{1, 2}").await, ("=> {1, 2}".to_string(), false));
}

// 7. Firecrawl and Jev policy.

#[tokio::test]
async fn firecrawl_pack_keeps_the_tool_policy() {
    let (url, recorded) = serve(vec![
        search_response(),
        Canned::json(
            402,
            json!({"success": false, "error": "no credits"}).to_string(),
        ),
    ])
    .await;
    let search = FirecrawlWebSearch::new(firecrawl_client(&url))
        .with_blocked_domains(vec!["blocked.com".to_string()]);
    let pack = firecrawl_pack(&url, search, 100);
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();

    let (text, is_error) = run(
        &code,
        "await search(query='x', include_domains=['blocked.com'])",
    )
    .await;
    assert!(is_error);
    assert!(
        text.contains("RuntimeError: Every requested include_domains entry is blocked."),
        "{text}"
    );
    assert!(recorded.lock().unwrap().is_empty());

    let (text, is_error) = run(&code, "len(await search(query='x', limit=100))").await;
    assert_eq!((text.as_str(), is_error), ("=> 2", false));
    let body: Value = serde_json::from_str(&recorded.lock().unwrap()[0].body).unwrap();
    assert_eq!(body["limit"], 10);
    assert!(body.get("scrapeOptions").is_none(), "{body}");

    let tool_use = ToolUseBlock::new(
        "toolu_1",
        TOOL_NAME,
        json!({"code": "await search(query='x')"}),
    );
    let error = code.call(&tool_use).await.unwrap_err();
    assert_eq!(error.status(), Some(402));
}

#[tokio::test]
async fn firecrawl_credit_budget_stops_calls() {
    let (url, recorded) = serve(vec![search_response()]).await;
    let search = FirecrawlWebSearch::new(firecrawl_client(&url));
    let pack = firecrawl_pack(&url, search, 2);
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    assert_eq!(
        run(&code, "len(await search(query='x'))").await,
        ("=> 2".to_string(), false)
    );
    let (text, is_error) = run(&code, "await scrape(url='https://example.com/a')").await;
    assert!(is_error);
    assert!(
        text.contains(
            "RuntimeError: Firecrawl credit budget of 2 used up; finish with what you have"
        ),
        "{text}"
    );
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

fn jev_client(url: &str) -> JevClient {
    JevClient::try_new("ts-key", "jev-1.13.0")
        .unwrap()
        .with_base_url(url)
        .unwrap()
        .with_max_retries(0)
}

/// A yes answer to the shortcuts' one question, with usage and a request id.
fn jev_yes() -> Canned {
    ok(json!({
        "model": "jev-1.13.0",
        "answers": {"answer": {"type": "noul", "noul": 0.99}},
        "usage": {"input_tokens": 12, "output_tokens": 3}
    }))
    .with_header("x-typesafe-request-id", "req_1")
}

#[tokio::test]
async fn jev_request_budget_stops_calls() {
    let (url, recorded) = serve(vec![jev_yes()]).await;
    let budget = Arc::new(packs::JevBudget::new(1));
    let pack = packs::JevPack::new(jev_client(&url))
        .with_budget(budget.clone())
        .build();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "(await yes_no(state='s', question='q'))['answer']";
    assert_eq!(run(&code, snippet).await, ("=> True".to_string(), false));
    let refused = "RuntimeError: Jev request budget of 1 used up; finish with what you have";
    let (text, is_error) = run(&code, snippet).await;
    assert!(is_error);
    assert!(text.contains(refused), "{text}");
    assert_eq!(recorded.lock().unwrap().len(), 1);
    assert_eq!(budget.used(), packs::JevUsed::new(1));
    code.finish().await.unwrap();

    // A restored budget is already used up, so nothing is sent.
    let restored = Arc::new(packs::JevBudget::new(1).with_used(budget.used()));
    let pack = packs::JevPack::new(jev_client(&url))
        .with_budget(restored)
        .build();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let (text, is_error) = run(&code, snippet).await;
    assert!(is_error);
    assert!(text.contains(refused), "{text}");
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

/// Answers every request with [`jev_yes`] after 200 ms, recording the most requests in flight
/// at once.
async fn slow_jev() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let most = peak.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (in_flight, peak) = (in_flight.clone(), peak.clone());
            tokio::spawn(async move {
                read_request(&mut socket).await;
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                let body = json!({
                    "model": "jev-1.13.0",
                    "answers": {"answer": {"type": "noul", "noul": 0.99}}
                })
                .to_string();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            });
        }
    });
    (format!("http://{address}"), most)
}

#[tokio::test]
async fn jev_max_concurrency_bounds_requests_in_flight() {
    let (url, peak) = slow_jev().await;
    let pack = packs::JevPack::new(jev_client(&url))
        .with_max_concurrency(2)
        .build();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "len(await asyncio.gather(*[yes_no(state=str(i), question='q') for i in \
                   range(5)]))";
    assert_eq!(run(&code, snippet).await, ("=> 5".to_string(), false));
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}

#[derive(Default)]
struct JevEvents(Mutex<Vec<packs::JevEvent>>);

impl packs::JevObserver for JevEvents {
    fn event(&self, event: packs::JevEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn jev_observer_sees_usage_and_request_id() {
    let (url, _) = serve(vec![
        jev_yes(),
        Canned::json(400, json!({"detail": "bad state"}).to_string()),
    ])
    .await;
    let events = Arc::new(JevEvents::default());
    let pack = packs::JevPack::new(jev_client(&url))
        .with_observer(events.clone())
        .build();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "(await yes_no(state='s', question='q'))['answer']";
    assert_eq!(run(&code, snippet).await, ("=> True".to_string(), false));
    let (text, is_error) = run(&code, snippet).await;
    assert!(
        is_error && text.contains("RuntimeError: Jev failed"),
        "{text}"
    );

    let events = events.0.lock().unwrap();
    assert_eq!(events.len(), 4, "{events:?}");
    assert!(matches!(
        events[0],
        packs::JevEvent::Asking {
            function: "yes_no",
            questions: 1
        }
    ));
    let packs::JevEvent::Answered {
        function: "yes_no",
        usage,
        request_id,
        attempts,
        ..
    } = &events[1]
    else {
        panic!("expected Answered, got {:?}", events[1]);
    };
    assert_eq!(
        (usage.input_tokens, usage.output_tokens),
        (Some(12), Some(3))
    );
    assert_eq!(request_id.as_deref(), Some("req_1"));
    assert_eq!(*attempts, 1);
    assert!(matches!(
        events[2],
        packs::JevEvent::Asking {
            function: "yes_no",
            questions: 1
        }
    ));
    let packs::JevEvent::Failed {
        function: "yes_no",
        error,
    } = &events[3]
    else {
        panic!("expected Failed, got {:?}", events[3]);
    };
    assert_eq!(error.status(), Some(400));
}

#[tokio::test]
async fn jev_observer_sees_retried_attempts() {
    let (url, _) = serve(vec![
        Canned::json(500, json!({"detail": "busy"}).to_string()),
        jev_yes(),
    ])
    .await;
    let events = Arc::new(JevEvents::default());
    let client = jev_client(&url).with_max_retries(1);
    let pack = packs::JevPack::new(client)
        .with_observer(events.clone())
        .build();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "(await yes_no(state='s', question='q'))['answer']";
    assert_eq!(run(&code, snippet).await, ("=> True".to_string(), false));
    let events = events.0.lock().unwrap();
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(
        matches!(events[1], packs::JevEvent::Answered { attempts: 2, .. }),
        "{events:?}"
    );
}

#[tokio::test]
async fn jev_budget_holds_under_gather() {
    let (url, recorded) = serve(vec![jev_yes(), jev_yes()]).await;
    let budget = Arc::new(packs::JevBudget::new(2));
    let events = Arc::new(JevEvents::default());
    let pack = packs::JevPack::new(jev_client(&url))
        .with_budget(budget.clone())
        .with_observer(events.clone())
        .build();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "async def one(i):\n    try:\n        return (await yes_no(state=str(i), \
                   question='q'))['answer']\n    except RuntimeError as e:\n        return \
                   str(e)\nresults = await asyncio.gather(*[one(i) for i in range(5)])\n\
                   (len([r for r in results if r is True]), len([r for r in results if \
                   isinstance(r, str) and 'budget of 2 used up' in r]))";
    assert_eq!(run(&code, snippet).await, ("=> (2, 3)".to_string(), false));
    assert_eq!(recorded.lock().unwrap().len(), 2);
    assert_eq!(budget.used(), packs::JevUsed::new(2));
    let events = events.0.lock().unwrap();
    let refused = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                packs::JevEvent::Refused { function: "yes_no", reason }
                    if reason.contains("budget of 2 used up")
            )
        })
        .count();
    assert_eq!(refused, 3, "{events:?}");
}

#[tokio::test]
async fn jev_budget_counts_only_sent_requests() {
    let (url, recorded) = serve(vec![Canned::json(
        500,
        json!({"detail": "down"}).to_string(),
    )])
    .await;
    let budget = Arc::new(packs::JevBudget::new(5));
    let events = Arc::new(JevEvents::default());
    let pack = packs::JevPack::new(jev_client(&url))
        .with_budget(budget.clone())
        .with_observer(events.clone())
        .build();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();

    // Invalid questions send nothing and cost nothing.
    let (text, is_error) = run(&code, "await choose(state='s', question='q', options=[])").await;
    assert!(
        is_error && text.contains("ValueError: Invalid jev call"),
        "{text}"
    );
    assert_eq!(budget.used(), packs::JevUsed::new(0));
    assert!(recorded.lock().unwrap().is_empty());
    {
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(
            events[0],
            packs::JevEvent::Failed {
                function: "choose",
                ..
            }
        ));
    }

    // A request that was sent counts even though it failed.
    let (text, is_error) = run(&code, "await yes_no(state='s', question='q')").await;
    assert!(
        is_error && text.contains("RuntimeError: Jev failed"),
        "{text}"
    );
    assert_eq!(budget.used(), packs::JevUsed::new(1));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

// 8. Output.

#[tokio::test]
async fn long_prints_are_capped_and_the_traceback_kept() {
    let code = CodeMode::try_new(test_pool().await, vec![], Limits::default()).unwrap();
    let snippet = "for i in range(300):\n    print('x' * 99)\nraise ValueError('end')";
    let (text, is_error) = run(&code, snippet).await;
    assert!(is_error);
    assert!(text.starts_with(&"x".repeat(99)), "{}", &text[..200]);
    assert!(
        text.contains("[print output truncated: 15000 of 30000 chars shown]"),
        "{text}"
    );
    let traceback = text.split("chars shown]\n").nth(1).unwrap();
    assert!(traceback.contains("line 3"), "{traceback}");
    assert!(
        traceback.trim_end().ends_with("ValueError: end"),
        "{traceback}"
    );
}

#[tokio::test]
async fn help_lists_signature_and_examples() {
    let pack = packs::reports();
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let (text, _) = run(&code, "help('count_by')").await;
    assert!(
        text.starts_with("=> 'def count_by(rows: list[dict], key: str) -> list[dict]\\n"),
        "{text}"
    );
    assert!(text.contains("Example:"), "{text}");
    let (text, _) = run(&code, "help('nope')").await;
    assert!(text.contains("table, to_csv, count_by"), "{text}");
    assert_eq!(used(&code), 0);
}

#[test]
fn listing_drops_types_and_keeps_defaults() {
    assert_eq!(
        untyped(
            "search(*, query: str, limit: int = 5, include: list[str] | None = None, \
             levels: list[str] = [\"a, b\", \"c\"]) -> list[dict]"
        ),
        "search(*, query, limit=5, include=None, levels=[\"a, b\", \"c\"]) -> list[dict]"
    );
}

#[test]
fn description_lists_packs_in_order() {
    let ToolUnionParam::CustomTool(tool) = definition(&[packs::reports()]) else {
        panic!("expected a custom tool");
    };
    let description = tool.description.unwrap();
    assert!(description.starts_with("Run Python (a sandboxed subset of Python 3.14)"));
    assert!(description.ends_with(
        "reports: tables, CSV and counts from lists of dicts\n  table(rows, columns=None) -> \
         str: markdown table; columns default to the first row's keys, so an \"error\" \
         column appears only when the first row has one\n  to_csv(rows, columns=None) -> str: \
         CSV text; columns default to the first row's keys, as for table\n  \
         count_by(rows, key) -> list[dict]: [{key: value, \"count\": n}] sorted by count, \
         descending"
    ));
}

// Reviewer follow-ups: limits, discards, value sizes and policy.

#[tokio::test]
async fn sandbox_timeout_discards_the_session() {
    let limits = Limits {
        max_feed_duration: Duration::from_millis(200),
        ..Limits::default()
    };
    let code = CodeMode::try_new(test_pool().await, vec![], limits).unwrap();
    assert_eq!(run(&code, "kept = 1").await, ("=> None".to_string(), false));
    let (text, is_error) = run(&code, "while True:\n    pass").await;
    assert!(is_error);
    assert!(text.contains("TimeoutError"), "{text}");
    assert!(text.ends_with(RESTARTED), "{text}");
    let (text, _) = run(&code, "kept").await;
    assert!(text.contains("NameError"), "{text}");
}

#[tokio::test]
async fn python_call_timeout_discards_the_session() {
    let pack = Pack::new("fakes", "fakes").with_host(slow());
    let mut code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    code.call_timeout = Duration::from_millis(300);
    assert_eq!(run(&code, "kept = 1").await, ("=> None".to_string(), false));
    let (text, is_error) = run(&code, "await slow()").await;
    assert!(is_error);
    assert_eq!(
        text,
        format!("The python call ran longer than 300ms and was stopped.\n{RESTARTED}")
    );
    let (text, _) = run(&code, "kept").await;
    assert!(text.contains("NameError"), "{text}");
}

#[tokio::test]
async fn suspension_limit_discards_the_session() {
    let code = CodeMode::try_new(test_pool().await, vec![], Limits::default()).unwrap();
    assert_eq!(run(&code, "kept = 1").await, ("=> None".to_string(), false));
    // help() is free, but every call is a suspension; the pool allows 1,000 per session.
    let (text, is_error) = run(&code, "for i in range(1100):\n    help('x')").await;
    assert!(is_error);
    assert!(text.contains("suspension limit 1000 exceeded"), "{text}");
    assert!(text.ends_with(RESTARTED), "{text}");
    let (text, _) = run(&code, "kept").await;
    assert!(text.contains("NameError"), "{text}");
}

#[tokio::test]
async fn bad_args_in_a_gather_are_refunded_and_the_rest_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let pack = Pack::new("fakes", "fakes").with_host(counted_search(calls.clone()));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let snippet = "await asyncio.gather(search(query='a'), search(query='b', limt=1), \
                   search(query='c'))";
    let (text, is_error) = run(&code, snippet).await;
    assert!(is_error);
    assert!(
        text.contains("TypeError: search(): unknown field `limt`"),
        "{text}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(used(&code), 2);
}

#[tokio::test]
async fn first_fatal_error_drops_the_rest_of_its_gather() {
    let pack = Pack::new("fakes", "fakes")
        .with_host(slow())
        .with_host(host("boom", |_| {
            Err(HostError::Fatal(Error::tool("rejected key", None)))
        }));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let tool_use = ToolUseBlock::new(
        "toolu_1",
        TOOL_NAME,
        json!({"code": "await asyncio.gather(slow(), boom())"}),
    );
    let started = Instant::now();
    let error = code.call(&tool_use).await.unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Tool, "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn cancel_notice_survives_a_full_pool() {
    let monty = std::env::var("MONTY_BIN").unwrap();
    let mut config = PoolConfig::subprocess(monty);
    config.max_processes = 1;
    config.checkout_timeout = Some(Duration::from_millis(200));
    let full = Arc::new(Pool::new(config).await.unwrap());
    let holder = CodeMode::try_new(full.clone(), vec![], Limits::default()).unwrap();
    assert_eq!(run(&holder, "1").await, ("=> 1".to_string(), false));

    // A session left mid-turn by a cancelled call, on a worker from another pool.
    let other = CodeMode::try_new(test_pool().await, vec![], Limits::default()).unwrap();
    assert_eq!(run(&other, "1").await, ("=> 1".to_string(), false));
    let mut stale = other.session.lock().await.take().unwrap();
    stale.mid_turn = true;
    let code = CodeMode::try_new(full, vec![], Limits::default()).unwrap();
    *code.session.lock().await = Some(stale);

    assert_eq!(
        run(&code, "1").await,
        (
            format!("{CANCELLED} {RESTARTED}\nNo sandbox worker free; try again."),
            true
        )
    );
}

#[tokio::test]
async fn limits_past_the_python_call_timeout_are_rejected() {
    let limits = Limits {
        max_feed_duration: Duration::from_secs(200),
        max_turn_duration: Duration::from_secs(200),
        ..Limits::default()
    };
    let error = CodeMode::try_new(test_pool().await, vec![], limits)
        .err()
        .unwrap();
    assert!(
        matches!(error.kind(), ErrorKind::Validation { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn host_call_timeout_is_configurable() {
    let pool = test_pool().await;
    let pack = || Pack::new("fakes", "fakes").with_host(nap(50));
    let limits = Limits {
        host_call_timeout: Duration::from_secs(60),
        ..Limits::default()
    };
    let code = CodeMode::try_new(pool.clone(), vec![pack()], limits).unwrap();
    assert_eq!(run(&code, "await nap()").await, ("=> None".to_string(), false));

    let code = CodeMode::try_new(pool, vec![pack()], Limits::default()).unwrap();
    let (text, is_error) = run(&code, "await nap()").await;
    assert!(is_error);
    assert!(
        text.contains("RuntimeError: nap() timed out after 45s"),
        "{text}"
    );
}

#[tokio::test]
async fn python_call_timeout_is_configurable() {
    let pack = Pack::new("fakes", "fakes").with_host(nap(65));
    let limits = Limits {
        host_call_timeout: Duration::from_secs(70),
        python_call_timeout: Duration::from_secs(200),
        ..Limits::default()
    };
    let code = CodeMode::try_new(test_pool().await, vec![pack], limits).unwrap();
    // Two 65 s host calls: 130 s in one python call, past the 120 s default.
    assert_eq!(
        run(&code, "await nap()\nawait nap()").await,
        ("=> None".to_string(), false)
    );
}

#[tokio::test]
async fn limits_past_a_configured_python_call_timeout_are_rejected() {
    let pool = test_pool().await;
    let limits = |python_call_timeout| Limits {
        max_feed_duration: Duration::from_secs(200),
        max_turn_duration: Duration::from_secs(200),
        python_call_timeout,
        ..Limits::default()
    };
    let error = CodeMode::try_new(pool.clone(), vec![], limits(Duration::from_secs(120)))
        .err()
        .unwrap();
    assert!(
        matches!(error.kind(), ErrorKind::Validation { .. }),
        "{error}"
    );
    let limits = Limits {
        max_feed_duration: Duration::from_secs(100),
        max_turn_duration: Duration::from_secs(100),
        python_call_timeout: Duration::from_secs(100),
        ..Limits::default()
    };
    assert!(CodeMode::try_new(pool.clone(), vec![], limits).is_err());
    let limits = Limits {
        max_feed_duration: Duration::from_secs(100),
        max_turn_duration: Duration::from_secs(100),
        python_call_timeout: Duration::from_secs(105),
        ..Limits::default()
    };
    assert!(CodeMode::try_new(pool, vec![], limits).is_ok());
}

fn description(code: &CodeMode) -> String {
    let ToolUnionParam::CustomTool(tool) = code.definition() else {
        panic!("expected a custom tool");
    };
    tool.description.unwrap()
}

#[tokio::test]
async fn definition_is_the_same_for_the_same_packs() {
    let pool = test_pool().await;
    let build = || {
        CodeMode::try_new(
            pool.clone(),
            vec![packs::reports(), fake_firecrawl()],
            Limits::default(),
        )
        .unwrap()
    };
    let first = serde_json::to_string(&build().definition()).unwrap();
    assert_eq!(first, serde_json::to_string(&build().definition()).unwrap());
    let fresh = serde_json::to_string(&build().with_fresh_sessions().definition()).unwrap();
    assert_eq!(
        fresh,
        serde_json::to_string(&build().with_fresh_sessions().definition()).unwrap()
    );
}

#[tokio::test]
async fn fresh_sessions_add_only_one_sentence() {
    let pool = test_pool().await;
    let build = || CodeMode::try_new(pool.clone(), vec![packs::reports()], Limits::default());
    let kept = description(&build().unwrap());
    let fresh = description(&build().unwrap().with_fresh_sessions());
    let opening = "Run Python (a sandboxed subset of Python 3.14) in a session that keeps \
                   variables between calls,\nincluding ones assigned before an error.";
    let rest = kept.strip_prefix(opening).unwrap();
    assert_eq!(
        fresh,
        format!(
            "{opening} Each turn's python calls run in a fresh session; variables do not carry \
             over to later turns.{rest}"
        )
    );
}

#[tokio::test]
async fn limits_past_the_pool_request_timeout_are_rejected() {
    let limits = Limits {
        max_feed_duration: Duration::from_secs(150),
        max_turn_duration: Duration::from_secs(150),
        python_call_timeout: Duration::from_secs(600),
        ..Limits::default()
    };
    let error = CodeMode::try_new(test_pool().await, vec![], limits)
        .err()
        .unwrap();
    assert!(
        matches!(error.kind(), ErrorKind::Validation { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("round trip"), "{error}");
}

// A list holding one list twice, 30 deep: tiny in the sandbox, 2^30 items expanded.
const DOUBLING: &str = "x = [1]\nfor i in range(30):\n    x = [x, x]\n";

#[tokio::test]
async fn shared_references_stay_bounded() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let pack = Pack::new("fakes", "fakes").with_host(host("echo", move |args| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(args)
    }));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let started = Instant::now();

    let (text, is_error) = run(&code, &format!("{DOUBLING}await echo(a=x)")).await;
    assert!(is_error);
    assert!(
        text.contains("ValueError: value too large: more than 100000 items"),
        "{text}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let (text, is_error) = run(&code, &format!("{DOUBLING}x")).await;
    assert!(!is_error);
    assert!(text.starts_with("=> [[[[["), "{}", &text[..50]);
    assert!(text.ends_with(
        "\n[result truncated: first 5000 chars shown; return less or summarize in Python]"
    ));
    assert!(text.chars().count() < 5_200);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn item_and_depth_limits() {
    let pack = Pack::new("fakes", "fakes").with_host(host("size", |args| {
        Ok(json!(args["a"].as_array().map_or(0, Vec::len)))
    }));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    assert_eq!(
        run(&code, "await size(a=list(range(100000)))").await,
        ("=> 100000".to_string(), false)
    );
    let (text, is_error) = run(&code, "await size(a=list(range(100001)))").await;
    assert!(is_error);
    assert!(text.contains("more than 100000 items"), "{text}");
    let (text, is_error) = run(
        &code,
        "y = 1\nfor i in range(70):\n    y = [y]\nawait size(a=y)",
    )
    .await;
    assert!(is_error);
    assert!(
        text.contains("ValueError: value nested deeper than 64 levels"),
        "{text}"
    );
}

#[tokio::test]
async fn firecrawl_pack_refuses_blocked_results_and_redirects() {
    let (url, _) = serve(vec![
        ok(json!({
            "success": true,
            "data": {"web": [
                {"url": "https://example.com/a", "title": "A"},
                {"url": "https://blocked.com/x", "title": "X"}
            ]}
        })),
        ok(json!({
            "success": true,
            "data": {"markdown": "# X", "metadata": {
                "title": "X", "url": "https://blocked.com/x", "statusCode": 200
            }}
        })),
    ])
    .await;
    let blocked = vec!["blocked.com".to_string()];
    let pack = packs::firecrawl(
        FirecrawlWebSearch::new(firecrawl_client(&url)).with_blocked_domains(blocked.clone()),
        FirecrawlFetchPage::new(firecrawl_client(&url)).with_blocked_domains(blocked),
        100,
    );
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    assert_eq!(
        run(&code, "[hit['url'] for hit in await search(query='x')]").await,
        ("=> ['https://example.com/a']".to_string(), false)
    );
    let (text, is_error) = run(&code, "await scrape(url='https://example.com/x')").await;
    assert!(is_error);
    assert!(
        text.contains("it redirected to https://blocked.com/x"),
        "{text}"
    );
}

// Final round: scrape options, broken packs, bad input and pack validation.

#[tokio::test]
async fn search_with_scrape_sends_options_and_cuts_markdown() {
    let (url, recorded) = serve(vec![ok(json!({
        "success": true,
        "data": {"web": [{"url": "https://example.com/a", "title": "A", "markdown": "abcdefghij"}]}
    }))])
    .await;
    let search = FirecrawlWebSearch::new(firecrawl_client(&url)).with_max_content_chars(5);
    let code = CodeMode::try_new(
        test_pool().await,
        vec![firecrawl_pack(&url, search, 100)],
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        run(
            &code,
            "(await search(query='x', scrape=True))[0]['markdown']"
        )
        .await,
        ("=> 'abcde [truncated]'".to_string(), false)
    );
    let body: Value = serde_json::from_str(&recorded.lock().unwrap()[0].body).unwrap();
    assert_eq!(body["scrapeOptions"]["onlyMainContent"], true, "{body}");
}

#[tokio::test]
async fn broken_pack_source_is_an_error() {
    let pack = Pack::new("broken", "broken").with_source("raise ValueError('bad pack')");
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let tool_use = ToolUseBlock::new("toolu_1", TOOL_NAME, json!({"code": "1"}));
    let error = code.call(&tool_use).await.unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Sandbox), "{error}");
    let source = std::error::Error::source(&error).map(ToString::to_string);
    assert!(
        source
            .as_deref()
            .is_some_and(|source| source.contains("ValueError: bad pack")),
        "{source:?}"
    );
}

#[tokio::test]
async fn input_without_code_is_an_error_result() {
    let code = CodeMode::try_new(test_pool().await, vec![], Limits::default()).unwrap();
    for input in [
        json!({"cod": "1"}),
        json!({"code": 1}),
        json!({"code": "1", "x": 2}),
    ] {
        let tool_use = ToolUseBlock::new("toolu_1", TOOL_NAME, input.clone());
        let result = code.call(&tool_use).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{input}");
        assert!(
            text(&result).starts_with("Invalid python input:"),
            "{input}"
        );
    }
}

#[tokio::test]
async fn duplicate_packs_and_stray_examples_fail_in_try_new() {
    let pool = test_pool().await;
    let twice = CodeMode::try_new(
        pool.clone(),
        vec![packs::reports(), Pack::new("reports", "again")],
        Limits::default(),
    );
    assert!(
        twice
            .err()
            .unwrap()
            .to_string()
            .contains("two packs are named reports")
    );

    let example = Example {
        code: "1",
        expect: "=> 1",
    };
    let stray = CodeMode::try_new(
        pool,
        vec![Pack::new("a", "a").with_example("nope", example)],
        Limits::default(),
    );
    assert!(
        stray
            .err()
            .unwrap()
            .to_string()
            .contains("pack a has an example for nope, which it does not define")
    );
}

#[tokio::test]
async fn fatal_host_error_surfaces_unchanged() {
    #[derive(Debug)]
    struct Cause;
    impl std::fmt::Display for Cause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the tool's own cause")
        }
    }
    impl std::error::Error for Cause {}

    let original = Error::tool("rejected key", Some(Box::new(Cause)));
    let fatal = original.clone();
    let pack = Pack::new("fakes", "fakes")
        .with_host(host("boom", move |_| Err(HostError::Fatal(fatal.clone()))));
    let code = CodeMode::try_new(test_pool().await, vec![pack], Limits::default()).unwrap();
    let tool_use = ToolUseBlock::new("toolu_1", TOOL_NAME, json!({"code": "await boom()"}));
    let error = code.call(&tool_use).await.unwrap_err();
    // Not wrapped: same kind, same words, and the tool author's error is still the source.
    assert_eq!(error.kind(), &ErrorKind::Tool);
    assert_eq!(error.to_string(), original.to_string());
    let source = std::error::Error::source(&error).expect("the tool's error is the source");
    assert_eq!(source.to_string(), "the tool's own cause");
    assert!(std::error::Error::source(source).is_none());
}

#[tokio::test]
async fn pool_start_failure_keeps_the_pool_error_as_source() {
    let Err(error) = pool("/nonexistent/monty-binary", 1).await else {
        panic!("a missing binary should fail the pool");
    };
    assert!(matches!(error.kind(), ErrorKind::Sandbox), "{error}");
    assert_eq!(error.to_string(), "could not start the sandbox pool");
    let source = std::error::Error::source(&error).expect("the pool error is the source");
    assert!(!source.to_string().is_empty());
}
