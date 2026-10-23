#![cfg(test)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::ClientTool;
use crate::error::{Error, ErrorKind};
use crate::test_support::{Canned, Recorded, serve};
use crate::types::{
    ContentBlock, ToolResultBlock, ToolResultBlockContent, ToolUnionParam, ToolUseBlock,
};

const MODEL: &str = "voyage/voyage-4-lite";

/// The checks these tests make on an [`Error`], by kind and status.
trait Checks {
    fn is_validation(&self) -> bool;
    fn is_not_found(&self) -> bool;
}

impl Checks for Error {
    fn is_validation(&self) -> bool {
        matches!(self.kind(), ErrorKind::Validation { .. })
    }

    fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }
}

fn ok(body: Value) -> Canned {
    Canned::json(200, body.to_string())
}

fn client(base_url: &str) -> TurbopufferClient {
    TurbopufferClient::try_new("tpuf-test-key", "gcp-us-central1")
        .unwrap()
        .with_base_url(base_url)
        .unwrap()
}

fn namespace(base_url: &str) -> Namespace {
    client(base_url).namespace("acme-help", MODEL).unwrap()
}

fn body_json(recorded: &Arc<Mutex<Vec<Recorded>>>, index: usize) -> Value {
    serde_json::from_str(&recorded.lock().unwrap()[index].body).unwrap()
}

fn request_line(recorded: &Arc<Mutex<Vec<Recorded>>>, index: usize) -> String {
    let recorded = recorded.lock().unwrap();
    recorded[index].headers.lines().next().unwrap().to_string()
}

fn string_content(result: &ToolResultBlock) -> &str {
    match &result.content {
        Some(ToolResultBlockContent::String(text)) => text,
        other => panic!("expected string content, got {other:?}"),
    }
}

fn hybrid_response(rows: Value) -> Canned {
    ok(json!({"results": [{"rows": rows}], "performance": {"embedding_tokens": 4}}))
}

fn search(base_url: &str) -> TurbopufferSearch {
    TurbopufferSearch::try_new(namespace(base_url), "search_help", "Acme's help articles").unwrap()
}

fn tool_use(input: Value) -> ToolUseBlock {
    ToolUseBlock::new("toolu_1", "search_help", input)
}

// 1. Upsert request and summary.

#[tokio::test]
async fn upsert_sends_schema_rows_and_auth() {
    let (url, recorded) = serve(vec![ok(json!({
        "status": "OK",
        "rows_upserted": 2,
        "rows_affected": 2,
        "performance": {"embedding_tokens": 31, "server_total_ms": 12}
    }))])
    .await;
    let summary = namespace(&url)
        .upsert([
            Document::new("refunds", "Refunds take 5 days.")
                .with_attribute("title", "Refund policy")
                .with_attribute("url", "https://help.acme.com/refunds"),
            Document::new(7_u64, "Bracket").with_attribute("in_stock", true),
        ])
        .await
        .unwrap();
    assert_eq!(
        summary,
        UpsertSummary {
            rows: 2,
            embedding_tokens: 31
        }
    );

    assert!(
        request_line(&recorded, 0).starts_with("POST /v2/namespaces/acme-help "),
        "{}",
        request_line(&recorded, 0)
    );
    assert!(
        recorded.lock().unwrap()[0]
            .headers
            .lines()
            .any(|l| l.eq_ignore_ascii_case("authorization: Bearer tpuf-test-key"))
    );
    assert_eq!(
        body_json(&recorded, 0),
        json!({
            "upsert_rows": [
                {"id": "refunds", "text": "Refunds take 5 days.", "title": "Refund policy",
                 "url": "https://help.acme.com/refunds"},
                {"id": 7, "text": "Bracket", "in_stock": true}
            ],
            "distance_metric": "cosine_distance",
            "schema": {"text": {"type": "string", "embed": MODEL, "full_text_search": true}}
        })
    );
}

// 2. Batches of 256.

#[tokio::test]
async fn upsert_sends_batches_of_256() {
    let (url, recorded) = serve(vec![
        ok(json!({"rows_upserted": 256, "performance": {"embedding_tokens": 1000}})),
        ok(json!({"rows_upserted": 44, "performance": {"embedding_tokens": 200}})),
    ])
    .await;
    let documents = (0..300_u64).map(|id| Document::new(id, format!("doc {id}")));
    let summary = namespace(&url).upsert(documents).await.unwrap();
    assert_eq!(
        summary,
        UpsertSummary {
            rows: 300,
            embedding_tokens: 1200
        }
    );
    let first = body_json(&recorded, 0);
    let second = body_json(&recorded, 1);
    assert_eq!(first["upsert_rows"].as_array().unwrap().len(), 256);
    assert_eq!(second["upsert_rows"].as_array().unwrap().len(), 44);
    assert_eq!(second["upsert_rows"][0]["id"], 256);
    assert_eq!(second["schema"], first["schema"]);
}

// 3. A bare status parses.

#[tokio::test]
async fn bare_status_ok_parses_as_zero_counts() {
    let (url, _) = serve(vec![
        ok(json!({"status": "OK"})),
        ok(json!({"status": "OK"})),
    ])
    .await;
    let ns = namespace(&url);
    let summary = ns.upsert([Document::new("a", "text")]).await.unwrap();
    assert_eq!(summary, UpsertSummary::default());
    assert_eq!(ns.delete(["a"]).await.unwrap(), 0);
}

// 4. Validation sends nothing.

#[tokio::test]
async fn invalid_input_sends_no_request() {
    let (url, recorded) = serve(vec![]).await;
    let ns = namespace(&url);
    let upsert_error = |documents: Vec<Document>| {
        let ns = ns.clone();
        async move { ns.upsert(documents).await.unwrap_err() }
    };

    let duplicate = upsert_error(vec![Document::new("a", "x"), Document::new("a", "y")]).await;
    assert!(
        duplicate.to_string().contains("appears twice"),
        "{duplicate}"
    );
    let long_id = upsert_error(vec![Document::new("x".repeat(65), "x")]).await;
    assert!(long_id.is_validation(), "{long_id}");
    for name in ["id", "text", "embed_text", "vector", "$x", ""] {
        let error = upsert_error(vec![Document::new("a", "x").with_attribute(name, 1)]).await;
        assert!(error.is_validation(), "{name}: {error}");
    }
    let long_name = upsert_error(vec![
        Document::new("a", "x").with_attribute("n".repeat(129), 1),
    ])
    .await;
    assert!(long_name.is_validation(), "{long_name}");

    let c = client(&url);
    let too_long = "n".repeat(129);
    for name in ["", "a/b", "a b", ".", "..", too_long.as_str()] {
        assert!(
            c.namespace(name, MODEL).unwrap_err().is_validation(),
            "{name:?}"
        );
    }
    assert!(
        c.namespace("ok-name_1.2", "  ")
            .unwrap_err()
            .is_validation()
    );

    let query_error = |query: Query| {
        let ns = ns.clone();
        async move { ns.query(&query).await.unwrap_err() }
    };
    assert!(query_error(Query::new("  ")).await.is_validation());
    let long = "word ".repeat(1800);
    assert!(long.len() > 8_192);
    assert!(query_error(Query::new(long.clone())).await.is_validation());
    assert!(
        query_error(Query::new(long).with_mode(SearchMode::Keyword))
            .await
            .is_validation()
    );
    assert!(
        query_error(Query::new("x").with_limit(0))
            .await
            .is_validation()
    );
    assert!(
        query_error(Query::new("x").with_limit(10_001))
            .await
            .is_validation()
    );
    assert!(
        query_error(Query::new("x").with_filter(Filter::all([])))
            .await
            .is_validation()
    );
    let nested = Filter::not(Filter::any([Filter::all([])]));
    assert!(
        query_error(Query::new("x").with_filter(nested))
            .await
            .is_validation()
    );
    assert_eq!(ns.upsert([]).await.unwrap(), UpsertSummary::default());
    assert_eq!(ns.delete(Vec::<DocumentId>::new()).await.unwrap(), 0);
    assert!(recorded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn long_vector_query_and_64_byte_id_are_accepted() {
    let (url, recorded) = serve(vec![
        ok(json!({"rows": []})),
        ok(json!({"rows_upserted": 1})),
    ])
    .await;
    let ns = namespace(&url);
    let long = "word ".repeat(1800);
    let response = ns
        .query(&Query::new(long).with_mode(SearchMode::Vector))
        .await
        .unwrap();
    assert!(response.hits.is_empty());
    ns.upsert([Document::new("x".repeat(64), "x")])
        .await
        .unwrap();
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

// 5. Hybrid query.

#[tokio::test]
async fn hybrid_body_and_rows() {
    let (url, recorded) = serve(vec![hybrid_response(json!([
        {"id": "refunds", "$dist": 0.032, "text": "Refunds take 5 days.",
         "embed_text": [0.1, 0.2], "title": "Refund policy", "kind": "policy"},
        {"id": 7, "$dist": 0.016}
    ]))])
    .await;
    let response = namespace(&url)
        .query(
            &Query::new("money back")
                .with_limit(3)
                .with_filter(Filter::equals("kind", "policy")),
        )
        .await
        .unwrap();

    assert!(request_line(&recorded, 0).starts_with("POST /v2/namespaces/acme-help/query "));
    let subquery = |rank_by: Value| {
        json!({
            "rank_by": rank_by,
            "limit": 3,
            "filters": ["kind", "Eq", "policy"],
            "exclude_attributes": ["embed_text"]
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
    assert_eq!(response.embedding_tokens, 4);
    assert_eq!(response.hits.len(), 2);
    let first = &response.hits[0];
    assert_eq!(first.id, DocumentId::from("refunds"));
    assert_eq!(first.text.as_deref(), Some("Refunds take 5 days."));
    assert!((first.score - 0.032).abs() < 1e-12);
    let mut attributes = serde_json::Map::new();
    attributes.insert("title".to_string(), json!("Refund policy"));
    attributes.insert("kind".to_string(), json!("policy"));
    assert_eq!(first.attributes, attributes);
    assert_eq!(response.hits[1].id, DocumentId::Number(7));
    assert_eq!(response.hits[1].text, None);
}

#[tokio::test]
async fn hybrid_without_results_is_a_decode_error() {
    let (url, _) = serve(vec![
        ok(json!({"rows": []})),
        ok(json!({"results": []})),
        ok(json!({"results": [{"rows": []}]})),
    ])
    .await;
    let ns = namespace(&url);
    for _ in 0..2 {
        let error = ns.query(&Query::new("x")).await.unwrap_err();
        assert!(matches!(error.kind(), ErrorKind::Decode), "{error}");
    }
    assert!(ns.query(&Query::new("x")).await.unwrap().hits.is_empty());
}

// 6. Vector and keyword queries.

#[tokio::test]
async fn vector_and_keyword_bodies_and_scores() {
    let (url, recorded) = serve(vec![
        ok(json!({"rows": [{"id": "a", "$dist": 0.25}], "performance": {"embedding_tokens": 3}})),
        ok(json!({"rows": [{"id": "b", "$dist": 7.5}]})),
    ])
    .await;
    let ns = namespace(&url);
    let vector = ns
        .query(&Query::new("q").with_mode(SearchMode::Vector))
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 0),
        json!({
            "rank_by": ["text", "ANN", ["Embed", "q"]],
            "limit": 10,
            "exclude_attributes": ["embed_text"]
        })
    );
    assert!((vector.hits[0].score - 0.75).abs() < 1e-12);
    assert_eq!(vector.embedding_tokens, 3);

    let keyword = ns
        .query(&Query::new("q").with_mode(SearchMode::Keyword).with_limit(2))
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 1),
        json!({
            "rank_by": ["text", "BM25", "q"],
            "limit": 2,
            "exclude_attributes": ["embed_text"]
        })
    );
    assert!((keyword.hits[0].score - 7.5).abs() < 1e-12);
    assert_eq!(keyword.embedding_tokens, 0);
}

// 7. Filters.

#[test]
fn filter_constructors_serialize_to_the_wire_form() {
    let filter = Filter::all([
        Filter::equals("kind", "product"),
        Filter::not_equals("draft", true),
        Filter::one_of("color", ["red", "blue"]),
        Filter::none_of("size", [1, 2]),
        Filter::less_than("price", 10),
        Filter::at_most("price", 9.5),
        Filter::greater_than("stock", 0),
        Filter::at_least("rating", 4),
        Filter::contains("tags", "sale"),
        Filter::contains_any("tags", ["new", "hot"]),
        Filter::any([Filter::equals("a", 1), Filter::not(Filter::equals("b", 2))]),
    ]);
    assert_eq!(
        filter.to_wire(),
        json!([
            "And",
            [
                ["kind", "Eq", "product"],
                ["draft", "NotEq", true],
                ["color", "In", ["red", "blue"]],
                ["size", "NotIn", [1, 2]],
                ["price", "Lt", 10],
                ["price", "Lte", 9.5],
                ["stock", "Gt", 0],
                ["rating", "Gte", 4],
                ["tags", "Contains", "sale"],
                ["tags", "ContainsAny", ["new", "hot"]],
                ["Or", [["a", "Eq", 1], ["Not", ["b", "Eq", 2]]]]
            ]
        ])
    );
}

// 8. Retries and error messages.

#[tokio::test]
async fn rate_limit_and_unavailable_are_retried() {
    let (url, recorded) = serve(vec![
        Canned::json(
            429,
            json!({"status": "error", "error": "slow down"}).to_string(),
        )
        .with_header("Retry-After", "0"),
        Canned::json(503, "{}"),
        ok(json!({"rows_upserted": 1})),
    ])
    .await;
    let summary = namespace(&url)
        .upsert([Document::new("a", "x")])
        .await
        .unwrap();
    assert_eq!(summary.rows, 1);
    assert_eq!(recorded.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn bad_request_is_not_retried_and_its_message_is_capped() {
    let long = "e".repeat(500);
    let (url, recorded) = serve(vec![Canned::json(
        400,
        json!({"status": "error", "error": long}).to_string(),
    )])
    .await;
    let error = namespace(&url)
        .upsert([Document::new("a", "x")])
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(400), "{error}");
    let message = error.to_string();
    let expected = format!("turbopuffer API error: {} (HTTP 400)", "e".repeat(200));
    assert!(message.contains(&expected), "{message}");
    assert!(!message.contains(&"e".repeat(201)), "{message}");
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn long_retry_after_fails_at_once() {
    let (url, recorded) = serve(vec![
        Canned::json(429, json!({"error": "later"}).to_string()).with_header("Retry-After", "60"),
    ])
    .await;
    let error = namespace(&url).query(&Query::new("x")).await.unwrap_err();
    assert_eq!(error.status(), Some(429), "{error}");
    assert_eq!(error.retry_after(), Some(Duration::from_secs(60)));
    assert!(
        error
            .to_string()
            .contains("turbopuffer API error: later (HTTP 429)")
    );
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn request_timeout_status_is_retried() {
    let (url, recorded) = serve(vec![Canned::json(408, "{}"), hybrid_response(json!([]))]).await;
    let response = namespace(&url).query(&Query::new("x")).await.unwrap();
    assert!(response.hits.is_empty());
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn conflict_is_retried() {
    let (url, recorded) = serve(vec![
        Canned::json(409, json!({"error": "index is still building"}).to_string()),
        hybrid_response(json!([])),
    ])
    .await;
    let response = namespace(&url).query(&Query::new("x")).await.unwrap();
    assert!(response.hits.is_empty());
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn query_is_resent_after_a_cut_success_but_delete_is_not() {
    let cut = || Canned::json(200, r#"{"results":[{"rows""#).with_cut_body();
    let (url, recorded) = serve(vec![cut(), hybrid_response(json!([]))]).await;
    let response = namespace(&url).query(&Query::new("x")).await.unwrap();
    assert!(response.hits.is_empty());
    assert_eq!(recorded.lock().unwrap().len(), 2);

    let (url, recorded) = serve(vec![cut(), ok(json!({"rows_deleted": 0}))]).await;
    let error = namespace(&url).delete(["a"]).await.unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Interrupted), "{error:?}");
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn query_is_resent_after_a_timeout_but_delete_is_not() {
    let slow = || ok(json!({"rows_deleted": 1})).with_stall(Duration::from_millis(400));
    let (url, recorded) = serve(vec![slow(), hybrid_response(json!([]))]).await;
    let ns = client(&url)
        .with_timeout(Duration::from_millis(300))
        .namespace("acme-help", MODEL)
        .unwrap();
    assert!(ns.query(&Query::new("x")).await.unwrap().hits.is_empty());
    assert_eq!(recorded.lock().unwrap().len(), 2);

    let (url, recorded) = serve(vec![slow(), ok(json!({"rows_deleted": 0}))]).await;
    let ns = client(&url)
        .with_timeout(Duration::from_millis(300))
        .namespace("acme-help", MODEL)
        .unwrap();
    let error = ns.delete(["a"]).await.unwrap_err();
    assert!(matches!(error.kind(), ErrorKind::Timeout), "{error:?}");
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

// 9. Deletes.

#[tokio::test]
async fn delete_and_delete_namespace() {
    let (url, recorded) = serve(vec![
        ok(json!({"rows_deleted": 2})),
        ok(json!({"status": "OK"})),
        Canned::json(
            404,
            json!({"status": "error", "error": "not found"}).to_string(),
        ),
    ])
    .await;
    let ns = namespace(&url);
    assert_eq!(
        ns.delete([DocumentId::from("a"), DocumentId::from(3_u64)])
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        body_json(&recorded, 0),
        json!({"deletes": ["a", 3], "distance_metric": "cosine_distance"})
    );
    ns.delete_namespace().await.unwrap();
    assert!(request_line(&recorded, 1).starts_with("DELETE /v2/namespaces/acme-help "));
    ns.delete_namespace().await.unwrap();
    assert_eq!(recorded.lock().unwrap().len(), 3);
}

// 10. Missing namespace.

#[tokio::test]
async fn query_on_missing_namespace_is_not_found() {
    let (url, _) = serve(vec![Canned::json(
        404,
        json!({"status": "error", "error": "namespace not found"}).to_string(),
    )])
    .await;
    let error = namespace(&url).query(&Query::new("x")).await.unwrap_err();
    assert!(error.is_not_found(), "{error}");
}

// 11. Environment.

#[test]
fn from_env_reads_key_region_and_base_url() {
    let env = |pairs: &[(&str, &str)]| {
        let vars: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        TurbopufferClient::from_env_with(move |name| vars.get(name).cloned())
    };
    let debug = |client: TurbopufferClient| {
        let debug = format!("{client:?}");
        assert!(!debug.contains("tpuf-key"), "{debug}");
        debug
    };

    let region = env(&[
        ("TURBOPUFFER_API_KEY", "tpuf-key"),
        ("TURBOPUFFER_REGION", "gcp-us-central1"),
    ])
    .unwrap();
    assert!(debug(region).contains("\"https://gcp-us-central1.turbopuffer.com\""));

    let custom = env(&[
        ("TURBOPUFFER_API_KEY", "tpuf-key"),
        ("TURBOPUFFER_BASE_URL", "https://tpuf.internal.example/"),
    ])
    .unwrap();
    assert!(debug(custom).contains("\"https://tpuf.internal.example\""));

    let both = env(&[
        ("TURBOPUFFER_API_KEY", "tpuf-key"),
        ("TURBOPUFFER_REGION", "gcp-us-central1"),
        ("TURBOPUFFER_BASE_URL", "https://tpuf.internal.example"),
    ])
    .unwrap();
    assert!(debug(both).contains("\"https://tpuf.internal.example\""));

    let blank_base = env(&[
        ("TURBOPUFFER_API_KEY", "tpuf-key"),
        ("TURBOPUFFER_REGION", "aws-us-east-1"),
        ("TURBOPUFFER_BASE_URL", "  "),
    ])
    .unwrap();
    assert!(debug(blank_base).contains("\"https://aws-us-east-1.turbopuffer.com\""));

    let missing_key = env(&[
        ("TURBOPUFFER_API_KEY", " "),
        ("TURBOPUFFER_REGION", "gcp-us-central1"),
    ])
    .unwrap_err();
    assert!(
        missing_key.to_string().contains("TURBOPUFFER_API_KEY"),
        "{missing_key}"
    );

    let no_host = env(&[
        ("TURBOPUFFER_API_KEY", "tpuf-key"),
        ("TURBOPUFFER_REGION", ""),
    ])
    .unwrap_err();
    assert!(
        no_host.to_string().contains("TURBOPUFFER_REGION"),
        "{no_host}"
    );

    assert!(
        TurbopufferClient::try_new("key", "GCP_us")
            .unwrap_err()
            .is_validation()
    );
    assert!(
        TurbopufferClient::try_new("key", "")
            .unwrap_err()
            .is_validation()
    );
    assert!(
        TurbopufferClient::try_new("bad\nkey", "gcp-us-central1")
            .unwrap_err()
            .is_validation()
    );
}

// 12. The tool.

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

#[tokio::test]
async fn tool_hits_become_citable_search_results() {
    let (url, recorded) = serve(vec![ok(json!({"rows": [
        {"id": "refunds", "$dist": 0.1, "text": "abcdefgh", "title": "Refund policy",
         "url": "https://help.acme.com/refunds", "kind": "policy"},
        {"id": 7, "$dist": 0.2, "url": 5}
    ]}))])
    .await;
    let search = search(&url)
        .with_max_text_chars(4)
        .with_filter(Filter::not_equals("draft", true))
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
    assert_eq!(sent["rank_by"], json!(["text", "ANN", ["Embed", "refund"]]));
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

#[tokio::test]
async fn tool_errors_by_who_can_fix_them() {
    let (url, recorded) = serve(vec![
        Canned::json(404, json!({"error": "namespace not found"}).to_string()),
        Canned::json(429, json!({"error": "slow down"}).to_string())
            .with_header("Retry-After", "60"),
        Canned::json(401, json!({"error": "bad key"}).to_string()),
    ])
    .await;
    let search = search(&url);

    let bad = search
        .call(&tool_use(json!({"query": "x", "filters": []})))
        .await
        .unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert!(string_content(&bad).starts_with("Invalid search_help input"));
    let blank = search.call(&tool_use(json!({"query": " "}))).await.unwrap();
    assert_eq!(blank.is_error, Some(true));
    assert!(recorded.lock().unwrap().is_empty());

    let missing = search
        .call(&tool_use(json!({"query": "refund"})))
        .await
        .unwrap();
    assert_eq!(missing.is_error, None);
    assert_eq!(string_content(&missing), "No documents match refund");

    let limited = search.call(&tool_use(json!({"query": "x"}))).await.unwrap();
    assert_eq!(limited.is_error, Some(true));
    assert!(string_content(&limited).contains("429"));

    let error = search
        .call(&tool_use(json!({"query": "x"})))
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(401), "{error}");
}

// 13. Tool names.

#[test]
fn tool_names_must_be_python_identifiers() {
    let ns = namespace("http://127.0.0.1:1");
    let too_long = "s".repeat(65);
    for name in ["search-help", "Search", "", "1search", too_long.as_str()] {
        assert!(
            TurbopufferSearch::try_new(ns.clone(), name, "docs")
                .unwrap_err()
                .is_validation(),
            "{name:?}"
        );
    }
    assert!(TurbopufferSearch::try_new(ns, "s".repeat(64), "docs").is_ok());
}

#[tokio::test]
async fn retries_stop_after_max_retries() {
    let unavailable = || Canned::json(503, "{}").with_header("Retry-After", "0");
    let (url, recorded) = serve(vec![
        unavailable(),
        unavailable(),
        unavailable(),
        unavailable(),
    ])
    .await;
    let error = client(&url)
        .with_max_retries(3)
        .namespace("acme-help", MODEL)
        .unwrap()
        .query(&Query::new("x"))
        .await
        .unwrap_err();
    assert_eq!(error.status(), Some(503), "{error}");
    assert_eq!(recorded.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn hybrid_embedding_tokens_fall_back_to_the_subqueries() {
    let (url, _) = serve(vec![ok(json!({"results": [
        {"rows": [{"id": "a", "$dist": 0.03}], "performance": {"embedding_tokens": 5}},
        {"rows": [], "performance": {"server_total_ms": 2}}
    ]}))])
    .await;
    let response = namespace(&url).query(&Query::new("x")).await.unwrap();
    assert_eq!(response.embedding_tokens, 5);
    assert_eq!(response.hits.len(), 1);
}

#[tokio::test]
async fn tool_reports_an_index_still_building_as_an_error_result() {
    let building = || {
        Canned::json(
            409,
            json!({"status": "error", "error": "index is still building"}).to_string(),
        )
    };
    let (url, recorded) = serve(vec![building(), building()]).await;
    let namespace = client(&url)
        .with_max_retries(1)
        .namespace("acme-help", MODEL)
        .unwrap();
    let result = TurbopufferSearch::try_new(namespace, "search_help", "Acme's help articles")
        .unwrap()
        .call(&tool_use(json!({"query": "x"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(string_content(&result).contains("409"));
    // The client retries a 409 first; only the last one reaches Claude.
    assert_eq!(recorded.lock().unwrap().len(), 2);
}
