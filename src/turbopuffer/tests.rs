#![cfg(test)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::search::SearchMode;
use super::*;
use crate::ClientTool;
use crate::error::ErrorKind;
use crate::test_support::{Canned, Recorded, record_events, serve};
use crate::types::{
    ContentBlock, ToolResultBlock, ToolResultBlockContent, ToolUnionParam, ToolUseBlock,
};

/// The fields the SDK requires on every query response.
fn billing_and_performance(mut body: Value) -> Value {
    body["billing"] = json!({
        "billable_logical_bytes_queried": 0,
        "billable_logical_bytes_returned": 0
    });
    body["performance"] = json!({
        "approx_namespace_size": 0,
        "cache_hit_ratio": 1.0,
        "cache_temperature": "hot",
        "exhaustive_search_count": 0,
        "query_execution_ms": 1,
        "server_total_ms": 1
    });
    body
}

fn rows_response(rows: Value) -> Canned {
    Canned::json(
        200,
        billing_and_performance(json!({"rows": rows})).to_string(),
    )
}

fn hybrid_response(rows: Value) -> Canned {
    Canned::json(
        200,
        billing_and_performance(json!({"results": [{"rows": rows}]})).to_string(),
    )
}

fn error_response(status: u16, message: &str) -> Canned {
    Canned::json(
        status,
        json!({"status": "error", "error": message}).to_string(),
    )
}

fn client(base_url: &str) -> Client {
    Client::builder()
        .base_url(base_url)
        .api_key("k")
        .max_retries(0)
        .build()
        .unwrap()
}

fn search(base_url: &str) -> TurbopufferSearch {
    TurbopufferSearch::try_new(
        client(base_url).namespace("acme-help"),
        "search_help",
        "Acme's help articles",
    )
    .unwrap()
}

fn body_json(recorded: &Arc<Mutex<Vec<Recorded>>>, index: usize) -> Value {
    serde_json::from_str(&recorded.lock().unwrap()[index].body).unwrap()
}

fn string_content(result: &ToolResultBlock) -> &str {
    match &result.content {
        Some(ToolResultBlockContent::String(text)) => text,
        other => panic!("expected string content, got {other:?}"),
    }
}

fn tool_use(input: Value) -> ToolUseBlock {
    ToolUseBlock::new("toolu_1", "search_help", input)
}

// 1. The tool's definition and name.

#[test]
fn tool_definition() {
    let search = search("http://127.0.0.1:1").with_max_text_chars(100);
    assert_eq!(search.name(), "search_help");
    let ToolUnionParam::CustomTool(tool) = search.definition() else {
        panic!("expected a custom tool");
    };
    assert_eq!(tool.name, "search_help");
    assert_eq!(
        tool.description.as_deref(),
        Some(
            "Search Acme's help articles. Hybrid (the default) matches both meaning and exact \
             words; use keyword for exact names, codes and ids, vector for loose paraphrase. \
             Each result carries the document's text (cut after 100 characters) and its \
             attributes. At most 10 results per call."
        )
    );
    assert_eq!(
        serde_json::to_value(&tool.input_schema).unwrap(),
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What to search for."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 10,
                          "description": "Number of results."},
                "mode": {"type": "string", "enum": ["hybrid", "vector", "keyword"],
                         "description": "How to match. Defaults to hybrid."}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    );
}

#[test]
fn tool_names_must_be_python_identifiers() {
    let ns = client("http://127.0.0.1:1").namespace("acme-help");
    let too_long = "s".repeat(65);
    for name in ["search-help", "Search", "", "1search", too_long.as_str()] {
        let error = TurbopufferSearch::try_new(ns.clone(), name, "docs").unwrap_err();
        assert!(
            matches!(error.kind(), ErrorKind::Validation { .. }),
            "{name:?}"
        );
    }
    assert!(TurbopufferSearch::try_new(ns, "s".repeat(64), "docs").is_ok());
}

// 2. The fixed filter.

#[test]
fn with_filter_rejects_an_empty_and_or_or_anywhere() {
    let empty_and = Filter::and(Vec::<Filter>::new());
    let empty_or = Filter::or(Vec::<Filter>::new());
    let nested = Filter::not(Filter::or([
        Filter::eq("a", 1),
        Filter::and(Vec::<Filter>::new()),
    ]));
    for filter in [empty_and, empty_or, nested] {
        let error = search("http://127.0.0.1:1")
            .with_filter(filter.clone())
            .unwrap_err();
        assert!(
            matches!(error.kind(), ErrorKind::Validation { .. }),
            "{filter:?}: {error}"
        );
    }
    let valid = Filter::and([
        Filter::eq("kind", "policy"),
        Filter::not(Filter::or([Filter::eq("draft", true)])),
    ]);
    assert!(search("http://127.0.0.1:1").with_filter(valid).is_ok());
}

#[tokio::test]
async fn hybrid_search_sends_the_filter_on_both_subqueries() {
    let (url, recorded) = serve(vec![hybrid_response(json!([
        {"id": "refunds", "$dist": 0.032, "text": "Refunds take 5 days.",
         "embed_text": [0.1, 0.2], "title": "Refund policy", "kind": "policy"},
        {"id": 7, "$dist": 0.016}
    ]))])
    .await;
    let search = search(&url)
        .with_filter(Filter::eq("kind", "policy"))
        .unwrap();
    let hits = search
        .search_hits("money back".to_string(), Some(3), SearchMode::Hybrid)
        .await
        .unwrap()
        .unwrap();

    let subquery = |rank_by: Value| {
        json!({
            "exclude_attributes": ["embed_text"],
            "filters": ["kind", "Eq", "policy"],
            "limit": 3,
            "rank_by": rank_by
        })
    };
    assert_eq!(
        body_json(&recorded, 0),
        json!({
            "queries": [
                subquery(json!(["text", "ANN", ["Embed", "money back"]])),
                subquery(json!(["text", "BM25", "money back"]))
            ],
            "rerank_by": ["RRF"],
            "limit": 3
        })
    );
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, ::turbopuffer::types::Id::from("refunds"));
    assert_eq!(hits[0].text.as_deref(), Some("Refunds take 5 days."));
    assert!((hits[0].score - 0.032).abs() < 1e-12);
    let mut attributes = serde_json::Map::new();
    attributes.insert("kind".to_string(), json!("policy"));
    attributes.insert("title".to_string(), json!("Refund policy"));
    assert_eq!(hits[0].attributes, attributes);
    assert_eq!(hits[1].id, ::turbopuffer::types::Id::Int(7));
    assert_eq!(hits[1].text, None);
}

// 3. Limits, scores and text cutting.

#[tokio::test]
async fn limit_defaults_to_5_and_is_clamped_to_1_through_10() {
    let (url, recorded) = serve(vec![
        rows_response(json!([])),
        rows_response(json!([])),
        rows_response(json!([])),
        rows_response(json!([])),
    ])
    .await;
    let search = search(&url);
    for limit in [None, Some(0), Some(50)] {
        search
            .search_hits("x".to_string(), limit, SearchMode::Keyword)
            .await
            .unwrap()
            .unwrap();
    }
    let search = search.with_default_limit(99);
    search
        .search_hits("x".to_string(), None, SearchMode::Keyword)
        .await
        .unwrap()
        .unwrap();
    let limits: Vec<Value> = (0..4)
        .map(|index| body_json(&recorded, index)["limit"].clone())
        .collect();
    assert_eq!(limits, [json!(5), json!(1), json!(10), json!(10)]);
}

#[tokio::test]
async fn vector_scores_are_similarity_and_keyword_scores_are_bm25() {
    let (url, recorded) = serve(vec![
        rows_response(json!([{"id": "a", "$dist": 0.25}])),
        rows_response(json!([{"id": "b", "$dist": 7.5}])),
    ])
    .await;
    let search = search(&url);
    let vector = search
        .search_hits("q".to_string(), None, SearchMode::Vector)
        .await
        .unwrap()
        .unwrap();
    assert!((vector[0].score - 0.75).abs() < 1e-12);
    assert_eq!(
        body_json(&recorded, 0)["rank_by"],
        json!(["text", "ANN", ["Embed", "q"]])
    );
    let keyword = search
        .search_hits("q".to_string(), None, SearchMode::Keyword)
        .await
        .unwrap()
        .unwrap();
    assert!((keyword[0].score - 7.5).abs() < 1e-12);
    assert_eq!(
        body_json(&recorded, 1)["rank_by"],
        json!(["text", "BM25", "q"])
    );
}

#[tokio::test]
async fn tool_hits_become_citable_search_results() {
    let (url, recorded) = serve(vec![rows_response(json!([
        {"id": "refunds", "$dist": 0.1, "text": "abcdefgh", "title": "Refund policy",
         "url": "https://help.acme.com/refunds", "kind": "policy"},
        {"id": 7, "$dist": 0.2, "url": 5}
    ]))])
    .await;
    let search = search(&url)
        .with_max_text_chars(4)
        .with_filter(Filter::not_eq("draft", true))
        .unwrap();
    let result = search
        .call(&tool_use(
            json!({"query": "refund", "limit": 50, "mode": "vector"}),
        ))
        .await
        .unwrap();

    let sent = body_json(&recorded, 0);
    assert_eq!(sent["limit"], 10);
    assert_eq!(sent["filters"], json!(["draft", "NotEq", true]));
    assert_eq!(sent["exclude_attributes"], json!(["embed_text"]));
    assert_eq!(
        serde_json::to_value(ContentBlock::ToolResult(result)).unwrap(),
        json!({
            "type": "tool_result",
            "tool_use_id": "toolu_1",
            "content": [
                {
                    "type": "search_result",
                    "source": "https://help.acme.com/refunds",
                    "title": "Refund policy",
                    "content": [
                        {"type": "text", "text": "abcd [truncated]"},
                        {"type": "text", "text": "Attributes: {\"kind\":\"policy\"}"}
                    ],
                    "citations": {"enabled": true}
                },
                {
                    "type": "search_result",
                    "source": "turbopuffer://acme-help/7",
                    "title": "7",
                    "content": [{"type": "text", "text": "7"}],
                    "citations": {"enabled": true}
                }
            ]
        })
    );
}

// 4. A namespace that was never written.

#[tokio::test(flavor = "current_thread")]
async fn missing_namespace_answers_with_no_documents_and_a_warning() {
    let (url, _) = serve(vec![error_response(404, "namespace not found")]).await;
    let (_guard, events) = record_events("topcoat_si::turbopuffer::search");
    let result = search(&url)
        .call(&tool_use(json!({"query": "refund"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, None);
    assert_eq!(string_content(&result), "No documents match refund");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].level, tracing::Level::WARN);
    assert!(events[0].fields.contains("acme-help"), "{events:?}");
}

// 5. Errors, by who can fix them.

#[tokio::test]
async fn bad_input_is_an_error_result_and_sends_nothing() {
    let (url, recorded) = serve(vec![]).await;
    let search = search(&url);
    let bad = search
        .call(&tool_use(json!({"query": "x", "filters": []})))
        .await
        .unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert!(string_content(&bad).starts_with("Invalid search_help input"));
    let blank = search.call(&tool_use(json!({"query": " "}))).await.unwrap();
    assert_eq!(blank.is_error, Some(true));
    assert!(string_content(&blank).contains("must not be empty"));
    assert!(recorded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn transient_and_bad_request_statuses_are_error_results() {
    let statuses = [400, 408, 409, 429, 500, 503];
    let (url, recorded) = serve(
        statuses
            .iter()
            .map(|status| error_response(*status, "nope"))
            .collect(),
    )
    .await;
    let search = search(&url);
    for status in statuses {
        let result = search.call(&tool_use(json!({"query": "x"}))).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{status}");
        let text = string_content(&result);
        assert!(text.starts_with("Search failed: "), "{text}");
        assert!(text.contains(&format!("HTTP {status}")), "{text}");
    }
    assert_eq!(recorded.lock().unwrap().len(), statuses.len());
}

#[tokio::test]
async fn rejected_keys_are_returned_as_err() {
    let (url, _) = serve(vec![
        error_response(401, "bad key"),
        error_response(403, "forbidden"),
    ])
    .await;
    let search = search(&url);
    for status in [401, 403] {
        let error = search
            .call(&tool_use(json!({"query": "x"})))
            .await
            .unwrap_err();
        assert_eq!(error.status(), Some(status), "{error}");
        assert!(matches!(
            error.kind(),
            ErrorKind::Api {
                service: crate::error::Service::Turbopuffer,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn timeouts_and_connection_failures_are_error_results() {
    let (url, _) = serve(vec![
        Canned::json(200, "").closing_unanswered(),
        // Last, since the server answers one connection at a time.
        rows_response(json!([])).with_stall(Duration::from_millis(500)),
    ])
    .await;
    let client = Client::builder()
        .base_url(&url)
        .api_key("k")
        .max_retries(0)
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let search =
        TurbopufferSearch::try_new(client.namespace("acme-help"), "search_help", "help").unwrap();
    for _ in 0..2 {
        let result = search
            .call(&tool_use(json!({"query": "x", "mode": "keyword"})))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            string_content(&result).starts_with("Search failed: "),
            "{}",
            string_content(&result)
        );
    }
}

#[tokio::test]
async fn sdk_errors_map_onto_our_kinds() {
    let (url, _) = serve(vec![
        Canned::json(200, "").closing_unanswered(),
        Canned::json(200, "{\"rows\": [}"),
        // Last, since the server answers one connection at a time.
        rows_response(json!([])).with_stall(Duration::from_millis(500)),
    ])
    .await;
    let ns = Client::builder()
        .base_url(&url)
        .api_key("k")
        .max_retries(0)
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap()
        .namespace("acme-help");
    let mut kinds = Vec::new();
    for _ in 0..3 {
        let error: crate::Error = ns.query().send().await.unwrap_err().into();
        kinds.push(error.kind().clone());
    }
    assert_eq!(
        kinds,
        [ErrorKind::Connection, ErrorKind::Decode, ErrorKind::Timeout]
    );
}

#[tokio::test]
async fn rows_without_a_score_are_an_error_result() {
    let (url, _) = serve(vec![
        rows_response(json!([{"id": "a"}])),
        Canned::json(
            200,
            billing_and_performance(json!({"results": []})).to_string(),
        ),
    ])
    .await;
    let search = search(&url);
    for mode in ["vector", "hybrid"] {
        let result = search
            .call(&tool_use(json!({"query": "x", "mode": mode})))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{mode}");
        assert!(
            string_content(&result).starts_with("Search failed: "),
            "{}",
            string_content(&result)
        );
    }
}

#[tokio::test]
async fn error_messages_keep_only_the_json_error_capped_at_200_chars() {
    let long = "e".repeat(500);
    let (url, _) = serve(vec![
        Canned::json(502, "<html><body>secret upstream page</body></html>")
            .with_content_type("text/html"),
        error_response(400, &long),
    ])
    .await;
    let ns = client(&url).namespace("acme-help");

    let html: crate::Error = ns.query().send().await.unwrap_err().into();
    assert_eq!(html.status(), Some(502), "{html}");
    let message = html.to_string();
    assert!(!message.contains("secret"), "{message}");
    assert!(!message.contains("html"), "{message}");

    let capped: crate::Error = ns.query().send().await.unwrap_err().into();
    let message = capped.to_string();
    let expected = format!("turbopuffer API error: {} (HTTP 400)", "e".repeat(200));
    assert!(message.contains(&expected), "{message}");
    assert!(!message.contains(&"e".repeat(201)), "{message}");
}
