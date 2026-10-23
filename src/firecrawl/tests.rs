#![cfg(test)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use time::macros::date;

use super::*;
use crate::ClientTool;
use std::error::Error as _;

use crate::error::{ErrorCode, ErrorKind, Service};
use crate::test_support::{Canned, Recorded, serve, uses_proxies};
use crate::types::{
    Content, ContentBlock, SearchResultBlock, ToolResultBlock, ToolResultBlockContent,
    ToolUnionParam, ToolUseBlock,
};

fn ok(body: Value) -> Canned {
    Canned::json(200, body.to_string())
}

fn client(base_url: &str) -> FirecrawlClient {
    FirecrawlClient::try_new("fc-test-key")
        .unwrap()
        .with_base_url(base_url)
        .unwrap()
}

fn tool_use(input: Value) -> ToolUseBlock {
    ToolUseBlock::new("toolu_1", WEB_SEARCH_TOOL_NAME, input)
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

#[tokio::test]
async fn request_json_shape_and_auth_header() {
    let (url, recorded) = serve(vec![ok(json!({"success": true, "data": {"web": []}}))]).await;
    let request = SearchRequest::new("rust async")
        .with_limit(7)
        .with_sources(vec![SearchSource::Web, SearchSource::News])
        .with_categories(vec![SearchCategory::Research, SearchCategory::Pdf])
        .with_include_domains(vec!["docs.rs".to_string()])
        .with_time_range(TimeRange::Custom {
            from: date!(2026 - 01 - 02),
            to: date!(2026 - 03 - 04),
        })
        .with_sort_by_date(true)
        .with_location("Germany")
        .with_country("DE")
        .with_timeout(Duration::from_secs(30))
        .with_ignore_invalid_urls(true)
        .with_scrape_options(
            ScrapeOptions::new()
                .with_only_main_content(true)
                .with_max_age(Duration::from_secs(3600)),
        );
    client(&url).search(&request).await.unwrap();

    let recorded_request = &recorded.lock().unwrap()[0];
    let first_line = recorded_request.headers.lines().next().unwrap();
    assert!(first_line.starts_with("POST /v2/search "), "{first_line}");
    assert!(
        recorded_request
            .headers
            .lines()
            .any(|l| l.eq_ignore_ascii_case("authorization: Bearer fc-test-key"))
    );
    assert_eq!(
        serde_json::from_str::<Value>(&recorded_request.body).unwrap(),
        json!({
            "query": "rust async",
            "limit": 7,
            "sources": ["web", "news"],
            "categories": ["research", "pdf"],
            "includeDomains": ["docs.rs"],
            "tbs": "cdr:1,cd_min:01/02/2026,cd_max:03/04/2026,sbd:1",
            "location": "Germany",
            "country": "DE",
            "timeout": 30000,
            "ignoreInvalidURLs": true,
            "scrapeOptions": {"formats": ["markdown"], "onlyMainContent": true, "maxAge": 3600000}
        })
    );
}

#[tokio::test]
async fn response_becomes_search_result_blocks() {
    let (url, recorded) = serve(vec![ok(json!({
        "success": true,
        "creditsUsed": 4,
        "warning": "one page failed to scrape",
        "unexpected": {"nested": true},
        "data": {
            "research": [{"anything": 1}],
            "web": [
                {"url": "https://example.com/a", "title": "A", "description": "about a",
                 "markdown": "ééééééééééééé", "position": 1, "newField": "x"},
                {"url": "https://blocked.example.org/b", "title": "B"}
            ],
            "news": [
                {"url": "https://news.example.net/c", "snippet": "breaking", "date": "2 hours ago"}
            ]
        }
    }))])
    .await;
    let search = FirecrawlWebSearch::new(client(&url))
        .with_max_content_chars(5)
        .with_blocked_domains(vec!["example.org".to_string()]);
    let result = search
        .execute(&tool_use(
            json!({"query": "latest", "recency": "day", "sources": ["web", "news"]}),
        ))
        .await
        .unwrap();

    let sent = body_json(&recorded, 0);
    // The default 5 results are split across the two sources.
    assert_eq!(sent["limit"], 3);
    assert_eq!(sent["tbs"], "qdr:d");
    assert_eq!(sent["excludeDomains"], json!(["example.org"]));
    assert_eq!(
        sent["scrapeOptions"],
        json!({"formats": ["markdown"], "onlyMainContent": true})
    );

    // ToolResultBlock gets its "type" tag from the ContentBlock it is sent in.
    assert_eq!(
        serde_json::to_value(ContentBlock::ToolResult(result.clone())).unwrap(),
        json!({
            "type": "tool_result",
            "tool_use_id": "toolu_1",
            "content": [
                {
                    "type": "search_result",
                    "source": "https://example.com/a",
                    "title": "A",
                    "content": [
                        {"type": "text", "text": "about a"},
                        {"type": "text", "text": "ééééé [truncated]"}
                    ],
                    "citations": {"enabled": true}
                },
                {
                    "type": "search_result",
                    "source": "https://news.example.net/c",
                    "title": "https://news.example.net/c",
                    "content": [
                        {"type": "text", "text": "breaking"},
                        {"type": "text", "text": "Published: 2 hours ago"}
                    ],
                    "citations": {"enabled": true}
                },
                {"type": "text", "text": "Firecrawl warning: one page failed to scrape"}
            ]
        })
    );
    assert_eq!(result.tool_use_id, "toolu_1");
    assert_eq!(result.is_error, None);
    let Some(ToolResultBlockContent::Array(items)) = &result.content else {
        panic!("expected array content");
    };
    assert_eq!(items.len(), 3, "blocked result must be dropped: {items:?}");
    let Content::SearchResult(web) = &items[0] else {
        panic!("expected search result");
    };
    assert_eq!(web.source, "https://example.com/a");
    assert_eq!(web.title, "A");
    assert_eq!(web.citations.as_ref().map(|c| c.enabled), Some(true));
    let texts: Vec<&str> = web.content.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["about a", "ééééé [truncated]"]);
    let Content::SearchResult(news) = &items[1] else {
        panic!("expected search result");
    };
    assert_eq!(news.title, "https://news.example.net/c");
    let texts: Vec<&str> = news.content.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["breaking", "Published: 2 hours ago"]);
    let Content::Text(warning) = &items[2] else {
        panic!("expected warning text");
    };
    assert_eq!(warning.text, "Firecrawl warning: one page failed to scrape");
}

#[tokio::test]
async fn zero_results_is_plain_text() {
    let (url, _) = serve(vec![ok(json!({"success": true, "data": {}}))]).await;
    let result = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(json!({"query": "nothing here"})))
        .await
        .unwrap();
    assert_eq!(string_content(&result), "No results for nothing here");
    assert_eq!(result.is_error, None);
}

#[tokio::test]
async fn callable_as_dyn_client_tool() {
    let (url, recorded) = serve(vec![ok(json!({"success": true, "data": {}}))]).await;
    let search = FirecrawlWebSearch::new(client(&url));
    let tool: &dyn ClientTool = &search;
    assert_eq!(tool.name(), WEB_SEARCH_TOOL_NAME);
    let ToolUnionParam::CustomTool(definition) = tool.definition() else {
        panic!("expected a custom tool");
    };
    assert_eq!(definition.name, WEB_SEARCH_TOOL_NAME);
    let result = tool
        .call(&tool_use(json!({"query": "nothing here"})))
        .await
        .unwrap();
    assert_eq!(result.tool_use_id, "toolu_1");
    assert_eq!(string_content(&result), "No results for nothing here");
    assert_eq!(body_json(&recorded, 0)["query"], "nothing here");
}

#[tokio::test]
async fn rate_limit_with_retry_after_then_success() {
    let (url, recorded) = serve(vec![
        Canned::json(
            429,
            json!({"success": false, "error": "slow down"}).to_string(),
        )
        .with_header("Retry-After", "1"),
        ok(json!({"success": true, "data": {"web": [{"url": "https://example.com"}]}})),
    ])
    .await;
    let started = Instant::now();
    let response = client(&url)
        .search(&SearchRequest::new("retry"))
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(recorded.lock().unwrap().len(), 2);
    assert_eq!(response.data.unwrap().web.unwrap().len(), 1);
}

#[tokio::test]
async fn rate_limit_after_retries_returns_the_429() {
    let limited = || {
        Canned::json(
            429,
            json!({"success": false, "error": "slow down", "code": "RATE_LIMIT"}).to_string(),
        )
        .with_header("Retry-After", "0")
    };
    let (url, recorded) = serve(vec![limited(), limited()]).await;
    let err = client(&url)
        .with_max_retries(1)
        .search(&SearchRequest::new("retry"))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(429), "{err:?}");
    assert!(err.to_string().contains("slow down (HTTP 429 RATE_LIMIT)"));
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn error_code_is_kept_as_sent() {
    let (url, _) = serve(vec![Canned::json(
        429,
        json!({"success": false, "error": "slow down", "code": "RATE_LIMIT"}).to_string(),
    )])
    .await;
    let err = client(&url)
        .with_max_retries(0)
        .search(&SearchRequest::new("code"))
        .await
        .unwrap_err();
    let ErrorKind::Api { error_code, .. } = err.kind() else {
        panic!("expected an Api error, got {err:?}");
    };
    assert_eq!(
        error_code,
        &Some(ErrorCode::Firecrawl("RATE_LIMIT".to_string()))
    );
}

#[tokio::test]
async fn conflict_and_not_implemented_are_retried() {
    for status in [409, 501] {
        let (url, recorded) = serve(vec![
            Canned::json(
                status,
                json!({"success": false, "error": "try again"}).to_string(),
            ),
            ok(json!({"success": true, "data": {}})),
        ])
        .await;
        client(&url)
            .search(&SearchRequest::new("retry"))
            .await
            .unwrap_or_else(|err| panic!("{status}: {err:?}"));
        assert_eq!(recorded.lock().unwrap().len(), 2, "{status}");
    }
}

#[tokio::test]
async fn error_status_with_a_cut_body_keeps_its_status_and_is_retried() {
    let cut = || Canned::json(503, r#"{"success":false"#).with_cut_body();
    let (url, _) = serve(vec![cut()]).await;
    let err = client(&url)
        .with_max_retries(0)
        .search(&SearchRequest::new("cut"))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(503), "{err:?}");
    assert!(err.is_retryable());
    assert!(err.source().is_some());
    assert!(
        err.to_string().contains("HTTP 503 with no error message"),
        "{err}"
    );

    let (url, recorded) = serve(vec![cut(), ok(json!({"success": true, "data": {}}))]).await;
    client(&url)
        .search(&SearchRequest::new("cut"))
        .await
        .unwrap();
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn payment_required_is_err() {
    let (url, recorded) = serve(vec![
        Canned::json(
            402,
            json!({"success": false, "error": "Insufficient credits"}).to_string(),
        ),
        ok(json!({"success": true})),
    ])
    .await;
    let err = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(402));
    assert!(matches!(err.kind(), ErrorKind::Api { .. }));
    assert!(err.to_string().contains("Insufficient credits"));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unauthorized_is_err_without_the_key() {
    let (url, recorded) = serve(vec![
        Canned::json(
            401,
            json!({"success": false, "error": "Unauthorized"}).to_string(),
        ),
        ok(json!({"success": true})),
    ])
    .await;
    let err = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(401));
    assert!(!format!("{err:?}").contains("fc-test-key"));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn bad_request_is_error_tool_result() {
    let (url, recorded) = serve(vec![Canned::json(
        400,
        json!({"success": false, "error": "Invalid tbs", "code": "BAD_REQUEST"}).to_string(),
    )])
    .await;
    let result = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(string_content(&result).contains("Invalid tbs (HTTP 400 BAD_REQUEST)"));
    assert_eq!(recorded.lock().unwrap().len(), 1, "400 must not be retried");
}

#[tokio::test]
async fn validation_errors_are_raised_before_sending() {
    let client = client("http://127.0.0.1:9");
    let cases = [
        SearchRequest::new("  "),
        SearchRequest::new("x".repeat(501)),
        SearchRequest::new("q").with_limit(0),
        SearchRequest::new("q").with_limit(101),
        SearchRequest::new("q")
            .with_include_domains(vec!["a.com".to_string()])
            .with_exclude_domains(vec!["b.com".to_string()]),
        SearchRequest::new("q").with_categories(vec![SearchCategory::Gov, SearchCategory::Pdf]),
        SearchRequest::new("q")
            .with_categories(vec![SearchCategory::Developer, SearchCategory::Research]),
    ];
    for request in cases {
        let err = client.search(&request).await.unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::Validation { .. }),
            "{request:?}: {err:?}"
        );
    }
    assert!(SearchRequest::new("é".repeat(500)).validate().is_ok());
    assert!(
        SearchRequest::new("q")
            .with_categories(vec![SearchCategory::Research, SearchCategory::Pdf])
            .validate()
            .is_ok()
    );
}

#[tokio::test]
async fn invalid_model_input_is_error_tool_result() {
    let search = FirecrawlWebSearch::new(client("http://127.0.0.1:9"));
    for input in [
        json!({}),
        json!({"query": "q", "recency": "decade"}),
        json!({"query": "q", "unknown": 1}),
        json!({"query": "q", "limit": 50}),
        json!({"query": ""}),
        json!({"query": "q", "include_domains": ["https://docs.rs"]}),
        json!({"query": "q", "exclude_domains": ["docs.rs:443"]}),
        json!({"query": "q", "include_domains": ["docs.rs/path"]}),
        json!({"query": "q", "exclude_domains": ["..evil.com"]}),
        json!({"query": "q", "exclude_domains": ["example..com"]}),
        json!({"query": "q", "country": "  "}),
    ] {
        let result = search.execute(&tool_use(input.clone())).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{input}");
        assert!(!string_content(&result).is_empty());
    }
}

#[tokio::test]
async fn developer_domain_lists_win_over_the_model() {
    let (url, recorded) = serve(vec![ok(json!({"success": true, "data": {}}))]).await;
    let search = FirecrawlWebSearch::new(client(&url))
        .with_allowed_domains(vec!["rust-lang.org".to_string(), "docs.rs".to_string()])
        .with_blocked_domains(vec!["docs.rs".to_string()]);
    search
        .execute(&tool_use(json!({
            "query": "q",
            "include_domains": ["blog.rust-lang.org", "evil.com", "docs.rs"]
        })))
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 0)["includeDomains"],
        json!(["blog.rust-lang.org"])
    );
    assert!(body_json(&recorded, 0).get("excludeDomains").is_none());

    let refused = search
        .execute(&tool_use(
            json!({"query": "q", "include_domains": ["evil.com"]}),
        ))
        .await
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
}

#[test]
fn tool_definition_and_dispatch() {
    let search = FirecrawlWebSearch::new(FirecrawlClient::try_new("fc-test-key").unwrap());
    let ToolUnionParam::CustomTool(tool) = search.tool() else {
        panic!("expected a custom tool");
    };
    assert_eq!(tool.name, "web_search");
    assert_eq!(tool.input_schema["required"], json!(["query"]));
    assert_eq!(tool.input_schema["additionalProperties"], json!(false));
    assert!(search.handles(&tool_use(json!({}))));
    assert!(!search.handles(&ToolUseBlock::new("t", "other", json!({}))));
    assert!(!format!("{search:?}").contains("fc-test-key"));
}

#[test]
fn unknown_response_fields_are_ignored() {
    let parsed: SearchResponse = serde_json::from_value(json!({
        "success": true,
        "id": "abc",
        "future": [1, 2],
        "data": {
            "research": {"papers": []},
            "images": [{"url": "https://e.com", "imageUrl": "https://e.com/i.png", "imageWidth": 10, "extra": null}],
            "web": [{"url": "https://e.com", "metadata": {"sourceURL": "https://e.com", "statusCode": 200, "other": "x"}}]
        }
    }))
    .unwrap();
    let data = parsed.data.unwrap();
    assert_eq!(data.images.unwrap()[0].image_width, Some(10));
    let metadata = data.web.unwrap()[0].metadata.clone().unwrap();
    assert_eq!(metadata.source_url.as_deref(), Some("https://e.com"));
    assert_eq!(metadata.status_code, Some(200));
}

#[test]
fn empty_api_key_is_rejected() {
    assert!(matches!(
        FirecrawlClient::try_new(" ").unwrap_err().kind(),
        ErrorKind::Validation { .. }
    ));
}

#[test]
fn api_key_with_inner_whitespace_is_rejected() {
    for key in ["fc key", "fc\tkey"] {
        assert!(
            matches!(
                FirecrawlClient::try_new(key).unwrap_err().kind(),
                ErrorKind::Validation { param } if param.as_deref() == Some("api_key")
            ),
            "{key:?}"
        );
    }
}

#[test]
fn api_key_not_allowed_in_a_header_is_not_printable_ascii() {
    let error = FirecrawlClient::try_new("fc-key\nInjected: 1").unwrap_err();
    assert!(
        matches!(error.kind(), ErrorKind::Validation { param } if param.as_deref() == Some("api_key")),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "Firecrawl api_key must be printable ASCII without whitespace"
    );
    assert!(error.source().is_none());
}

#[test]
fn base_url_needs_https_or_loopback_http() {
    let firecrawl = || FirecrawlClient::try_new("fc-test-key").unwrap();
    for url in [
        "https://api.firecrawl.dev/",
        "http://127.0.0.1:9000",
        "http://localhost:9000/",
        "http://[::1]:9000",
    ] {
        let client = firecrawl().with_base_url(url).unwrap();
        assert!(!client.base_url.ends_with('/'), "{url}");
    }
    for url in [
        "http://api.firecrawl.dev",
        "http://10.0.0.1",
        "ftp://127.0.0.1",
        "not a url",
        "https://user:pass@api.firecrawl.dev",
        "https://api.firecrawl.dev?x=1",
    ] {
        let error = firecrawl().with_base_url(url).unwrap_err();
        assert!(
            matches!(error.kind(), ErrorKind::Validation { param, .. } if param.as_deref() == Some("base_url")),
            "{url}: {error}"
        );
    }
}

#[test]
fn loopback_base_url_skips_proxies() {
    let firecrawl = FirecrawlClient::try_new("fc-test-key").unwrap();
    assert!(uses_proxies(&firecrawl.http));
    let firecrawl = firecrawl.with_base_url("http://127.0.0.1:1").unwrap();
    assert!(!uses_proxies(&firecrawl.http));
}

#[tokio::test]
async fn retry_after_over_cap_fails_without_retrying() {
    let limited = || {
        Canned::json(
            429,
            json!({"success": false, "error": "slow down"}).to_string(),
        )
        .with_header("Retry-After", "120")
    };
    let (url, recorded) = serve(vec![limited(), limited()]).await;
    let started = Instant::now();
    let err = client(&url)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(429), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(recorded.lock().unwrap().len(), 1);

    let (url, recorded) = serve(vec![limited(), limited()]).await;
    let result = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn server_errors_and_timeouts_are_retried() {
    for status in [503, 408] {
        let (url, recorded) = serve(vec![
            Canned::json(
                status,
                json!({"success": false, "error": "try again"}).to_string(),
            )
            .with_header("Retry-After", "0"),
            ok(json!({"success": true, "data": {"web": [{"url": "https://example.com"}]}})),
        ])
        .await;
        let response = client(&url).search(&SearchRequest::new("q")).await.unwrap();
        assert_eq!(response.data.unwrap().web.unwrap().len(), 1, "{status}");
        assert_eq!(recorded.lock().unwrap().len(), 2, "{status}");
    }

    // Without Retry-After the client backs off exponentially before retrying.
    let (url, recorded) = serve(vec![
        Canned::json(500, json!({"success": false}).to_string()),
        ok(json!({"success": true, "data": {}})),
    ])
    .await;
    client(&url).search(&SearchRequest::new("q")).await.unwrap();
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_post_closed_before_any_reply_is_resent() {
    let (url, recorded) = serve(vec![
        Canned::json(200, "").closing_unanswered(),
        ok(json!({"success": true, "data": {}})),
    ])
    .await;
    client(&url).search(&SearchRequest::new("q")).await.unwrap();
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn domain_entries_are_normalized() {
    let response = json!({"success": true, "data": {"web": [
        {"url": "https://example.org./trailing-dot"},
        {"url": "https://a.wild.com/"},
        {"url": "https://dot.com/"},
        {"url": "https://xn--bcher-kva.de/"},
        {"url": "https://bücher.de/unicode"},
        {"url": "https://evilexample.com/"},
        {"url": "https://example.com/"},
        {"url": "https://kept.net/"}
    ]}});
    let (url, recorded) = serve(vec![ok(response)]).await;
    let search = FirecrawlWebSearch::new(client(&url))
        .with_default_limit(10)
        .with_blocked_domains(vec![
            "Example.ORG.".to_string(),
            "*.wild.com".to_string(),
            ".dot.com".to_string(),
            "bücher.de".to_string(),
        ]);
    let result = search
        .execute(&tool_use(
            json!({"query": "q", "exclude_domains": ["example.com"]}),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 0)["excludeDomains"],
        json!([
            "example.org",
            "wild.com",
            "dot.com",
            "xn--bcher-kva.de",
            "example.com"
        ])
    );
    let Some(ToolResultBlockContent::Array(items)) = &result.content else {
        panic!("expected results: {result:?}");
    };
    let sources: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            Content::SearchResult(block) => Some(block.source.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(sources, ["https://evilexample.com/", "https://kept.net/"]);
}

#[tokio::test]
async fn invalid_developer_config_is_err() {
    let configs = [
        FirecrawlWebSearch::new(client("http://127.0.0.1:9")).with_default_limit(0),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9")).with_default_limit(11),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9")).with_max_content_chars(0),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9")).with_country(" "),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
            .with_blocked_domains(vec!["https://evil.com".to_string()]),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
            .with_blocked_domains(vec!["..evil.com".to_string()]),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
            .with_blocked_domains(vec!["*..evil.com".to_string()]),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
            .with_blocked_domains(vec!["example..com".to_string()]),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
            .with_allowed_domains(vec!["example.com/docs".to_string()]),
        FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
            .with_allowed_domains(vec!["docs.example.com".to_string()])
            .with_blocked_domains(vec!["example.com".to_string()]),
    ];
    for search in configs {
        let err = search
            .execute(&tool_use(json!({"query": "q"})))
            .await
            .unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::Validation { .. }),
            "{search:?}: {err:?}"
        );
    }
}

#[tokio::test]
async fn exclude_domains_emptying_the_allowlist_says_so() {
    let search = FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
        .with_allowed_domains(vec!["docs.rs".to_string()]);
    let result = search
        .execute(&tool_use(
            json!({"query": "q", "exclude_domains": ["docs.rs"]}),
        ))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let message = string_content(&result);
    assert!(
        message.contains("exclude_domains remove every domain"),
        "{message}"
    );
    assert!(message.contains("docs.rs"), "{message}");
}

#[tokio::test]
async fn null_optional_inputs_are_absent() {
    let (url, recorded) = serve(vec![ok(json!({"success": true, "data": {}}))]).await;
    let result = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(json!({
            "query": "q",
            "limit": null,
            "recency": null,
            "sources": null,
            "include_domains": null,
            "exclude_domains": null,
            "country": null
        })))
        .await
        .unwrap();
    assert_eq!(result.is_error, None, "{result:?}");
    assert_eq!(body_json(&recorded, 0)["limit"], 5);
}

#[tokio::test]
async fn limit_caps_total_results_across_sources() {
    let web: Vec<Value> = (0..3)
        .map(|i| json!({"url": format!("https://web.com/{i}")}))
        .collect();
    let news: Vec<Value> = (0..3)
        .map(|i| json!({"url": format!("https://news.com/{i}")}))
        .collect();
    let (url, recorded) = serve(vec![ok(
        json!({"success": true, "data": {"web": web, "news": news}}),
    )])
    .await;
    let result = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(
            json!({"query": "q", "limit": 5, "sources": ["web", "news"]}),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(&recorded, 0)["limit"], 3);
    let Some(ToolResultBlockContent::Array(items)) = &result.content else {
        panic!("expected results: {result:?}");
    };
    assert_eq!(items.len(), 5);
    let sources: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            Content::SearchResult(block) => Some(block.source.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        sources,
        [
            "https://web.com/0",
            "https://web.com/1",
            "https://web.com/2",
            "https://news.com/0",
            "https://news.com/1"
        ]
    );
}

fn result_sources(result: &ToolResultBlock) -> Vec<String> {
    match &result.content {
        Some(ToolResultBlockContent::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Content::SearchResult(block) => Some(block.source.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn ip_entries_and_hostless_results() {
    let response = json!({"success": true, "data": {"web": [
        {"url": "http://10.0.0.1/blocked"},
        {"url": "http://10.0.0.2/kept"},
        {"url": "mailto:someone@ok.com"},
        {"url": "data:text/plain,hello"},
        {"url": "ftp://ok.com/file"},
        {"url": "not a url"},
        {"url": "https://ok.com/kept"}
    ]}});
    let (url, recorded) = serve(vec![ok(response)]).await;
    let result = FirecrawlWebSearch::new(client(&url))
        .with_default_limit(10)
        .with_blocked_domains(vec!["10.0.0.1".to_string()])
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 0)["excludeDomains"],
        json!(["10.0.0.1"])
    );
    assert_eq!(
        result_sources(&result),
        ["http://10.0.0.2/kept", "https://ok.com/kept"]
    );

    let response = json!({"success": true, "data": {"web": [
        {"url": "http://10.0.0.2/kept"},
        {"url": "http://10.0.0.3/dropped"}
    ]}});
    let (url, recorded) = serve(vec![ok(response)]).await;
    let result = FirecrawlWebSearch::new(client(&url))
        .with_allowed_domains(vec!["10.0.0.2".to_string()])
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 0)["includeDomains"],
        json!(["10.0.0.2"])
    );
    assert_eq!(result_sources(&result), ["http://10.0.0.2/kept"]);
}

#[tokio::test]
async fn blocked_subdomain_under_model_include_is_dropped() {
    let response = json!({"success": true, "data": {"web": [
        {"url": "https://blog.rust-lang.org/post"},
        {"url": "https://forge.rust-lang.org/page"},
        {"url": "https://www.rust-lang.org/"}
    ]}});
    let (url, recorded) = serve(vec![ok(response)]).await;
    let result = FirecrawlWebSearch::new(client(&url))
        .with_blocked_domains(vec!["blog.rust-lang.org".to_string()])
        .execute(&tool_use(json!({
            "query": "q",
            "include_domains": ["rust-lang.org"],
            "exclude_domains": ["forge.rust-lang.org"]
        })))
        .await
        .unwrap();
    let sent = body_json(&recorded, 0);
    assert_eq!(sent["includeDomains"], json!(["rust-lang.org"]));
    assert!(sent.get("excludeDomains").is_none());
    assert_eq!(result_sources(&result), ["https://www.rust-lang.org/"]);
}

#[tokio::test]
async fn snippets_only_without_scraping() {
    let (url, recorded) = serve(vec![ok(json!({"success": true, "data": {}}))]).await;
    let search = FirecrawlWebSearch::new(client(&url)).with_scrape_options(None);
    let ToolUnionParam::CustomTool(tool) = search.tool() else {
        panic!("expected a custom tool");
    };
    let description = tool.description.unwrap();
    assert!(!description.contains("markdown"), "{description}");
    assert_eq!(tool.input_schema["properties"]["limit"]["maximum"], 10);
    search
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap();
    assert!(body_json(&recorded, 0).get("scrapeOptions").is_none());

    let scraping = FirecrawlWebSearch::new(client("http://127.0.0.1:9"));
    let ToolUnionParam::CustomTool(tool) = scraping.tool() else {
        panic!("expected a custom tool");
    };
    let description = tool.description.unwrap();
    assert!(
        description.contains("main content as markdown"),
        "{description}"
    );

    let whole_page = FirecrawlWebSearch::new(client("http://127.0.0.1:9"))
        .with_scrape_options(Some(ScrapeOptions::new()));
    let ToolUnionParam::CustomTool(tool) = whole_page.tool() else {
        panic!("expected a custom tool");
    };
    let description = tool.description.unwrap();
    assert!(
        description.contains("page content as markdown"),
        "{description}"
    );
    assert!(!description.contains("main content"), "{description}");
}

#[tokio::test]
#[ignore = "hits the live Firecrawl API; needs FIRECRAWL_API_KEY"]
async fn live_search() {
    let client = FirecrawlClient::from_env().expect(
        "live_search needs FIRECRAWL_API_KEY: FIRECRAWL_API_KEY=... cargo test live_search -- --ignored",
    );
    let result = FirecrawlWebSearch::new(client)
        .with_default_limit(2)
        .execute(&tool_use(json!({"query": "Rust programming language"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, None, "{result:?}");
    let Some(ToolResultBlockContent::Array(items)) = &result.content else {
        panic!("expected results: {result:?}");
    };
    assert!(
        items
            .iter()
            .any(|item| matches!(item, Content::SearchResult(_)))
    );
}

fn fetch_use(input: Value) -> ToolUseBlock {
    ToolUseBlock::new("toolu_2", "fetch_page", input)
}

fn scraped(markdown: &str, metadata: Value) -> Canned {
    ok(json!({"success": true, "data": {"markdown": markdown, "metadata": metadata}}))
}

fn only_search_result(result: &ToolResultBlock) -> &SearchResultBlock {
    match &result.content {
        Some(ToolResultBlockContent::Array(items)) => match items.as_slice() {
            [Content::SearchResult(block)] => block,
            other => panic!("expected exactly one search_result, got {other:?}"),
        },
        other => panic!("expected array content, got {other:?}"),
    }
}

#[tokio::test]
async fn scrape_request_wire_body() {
    let (url, recorded) = serve(vec![scraped("# Hi", json!({}))]).await;
    let request = ScrapeRequest::new("https://example.com/page").with_options(
        ScrapeOptions::new()
            .with_only_main_content(true)
            .with_max_age(Duration::from_secs(3600))
            .with_timeout(Duration::from_secs(30))
            .with_pdf_max_pages(20),
    );
    let response = client(&url).scrape(&request).await.unwrap();
    assert_eq!(response.data.markdown.as_deref(), Some("# Hi"));

    let recorded_request = &recorded.lock().unwrap()[0];
    let first_line = recorded_request.headers.lines().next().unwrap();
    assert!(first_line.starts_with("POST /v2/scrape "), "{first_line}");
    assert_eq!(
        serde_json::from_str::<Value>(&recorded_request.body).unwrap(),
        json!({
            "url": "https://example.com/page",
            "formats": ["markdown"],
            "onlyMainContent": true,
            "maxAge": 3600000,
            "timeout": 30000,
            "parsers": [{"type": "pdf", "maxPages": 20}, "image"]
        })
    );
}

#[tokio::test]
async fn scrape_validation_errors_are_raised_before_sending() {
    let (url, recorded) = serve(vec![]).await;
    let client = client(&url);
    let options = ScrapeOptions::new;
    let cases = [
        ScrapeRequest::new(""),
        ScrapeRequest::new("example.com/page"),
        ScrapeRequest::new("/relative"),
        ScrapeRequest::new("ftp://example.com/file"),
        ScrapeRequest::new("https://example.com")
            .with_options(options().with_timeout(Duration::from_millis(999))),
        ScrapeRequest::new("https://example.com")
            .with_options(options().with_timeout(Duration::from_secs(301))),
        ScrapeRequest::new("https://example.com").with_options(options().with_pdf_max_pages(0)),
        ScrapeRequest::new("https://example.com")
            .with_options(options().with_pdf_max_pages(10_001)),
    ];
    for request in cases {
        let err = client.scrape(&request).await.unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::Validation { .. }),
            "{request:?}: {err:?}"
        );
    }
    assert!(recorded.lock().unwrap().is_empty());
    assert!(
        ScrapeRequest::new("http://example.com")
            .with_options(
                options()
                    .with_timeout(Duration::from_secs(300))
                    .with_pdf_max_pages(10_000)
            )
            .validate()
            .is_ok()
    );
}

#[test]
fn scrape_metadata_never_fails_the_response() {
    let parsed: ScrapeResponse = serde_json::from_value(json!({
        "success": true,
        "warning": "top-level warning",
        "data": {
            "markdown": "text",
            "metadata": {
                "title": ["First", "Second"],
                "description": 42,
                "sourceURL": "https://example.com/a",
                "url": "https://example.com/b",
                "statusCode": "200",
                "contentType": "application/pdf",
                "numPages": 20,
                "totalPages": -1,
                "error": null
            }
        }
    }))
    .unwrap();
    let metadata = parsed.data.metadata.unwrap();
    assert_eq!(metadata.title.as_deref(), Some("First"));
    assert_eq!(metadata.description, None);
    assert_eq!(
        metadata.source_url.as_deref(),
        Some("https://example.com/a")
    );
    assert_eq!(metadata.url.as_deref(), Some("https://example.com/b"));
    assert_eq!(metadata.status_code, None);
    assert_eq!(metadata.content_type.as_deref(), Some("application/pdf"));
    assert_eq!(metadata.num_pages, Some(20));
    assert_eq!(metadata.total_pages, None);
    assert_eq!(metadata.error, None);
    assert_eq!(parsed.data.warning.as_deref(), Some("top-level warning"));

    let parsed: ScrapeResponse = serde_json::from_value(json!({
        "success": true,
        "warning": "top",
        "data": {"warning": "inner", "metadata": "not an object"}
    }))
    .unwrap();
    assert_eq!(parsed.data.warning.as_deref(), Some("inner"));
    assert_eq!(parsed.data.metadata, Some(ResultMetadata::default()));
}

#[tokio::test]
async fn fetch_page_returns_one_chunked_search_result() {
    let intro = "word ".repeat(250).trim_end().to_string();
    let second = "more ".repeat(200).trim_end().to_string();
    let long = "x".repeat(4000);
    let markdown = format!("{intro}\n\n{second}\n# Heading\nUnder the heading.\n\n{long}");
    let (url, recorded) = serve(vec![ok(json!({
        "success": true,
        "data": {
            "markdown": markdown,
            "warning": "slow page",
            "metadata": {
                "title": ["Report"],
                "sourceURL": "https://example.com/report.pdf",
                "url": "https://cdn.example.com/report.pdf",
                "statusCode": 200,
                "numPages": 20,
                "totalPages": 80
            }
        }
    }))])
    .await;
    let fetch = FirecrawlFetchPage::new(client(&url)).with_max_content_chars(6000);
    let tool: &dyn ClientTool = &fetch;
    assert_eq!(tool.name(), "fetch_page");
    let ToolUnionParam::CustomTool(definition) = tool.definition() else {
        panic!("expected a custom tool");
    };
    assert_eq!(definition.name, "fetch_page");
    assert_eq!(definition.input_schema["required"], json!(["url"]));
    assert_eq!(
        definition.input_schema["additionalProperties"],
        json!(false)
    );

    let result = tool
        .call(&fetch_use(json!({"url": "https://example.com/report.pdf"})))
        .await
        .unwrap();
    assert_eq!(result.tool_use_id, "toolu_2");
    assert_eq!(result.is_error, None);
    assert_eq!(
        body_json(&recorded, 0),
        json!({
            "url": "https://example.com/report.pdf",
            "formats": ["markdown"],
            "onlyMainContent": true,
            "maxAge": 3600000,
            "timeout": 30000,
            "parsers": [{"type": "pdf", "maxPages": 20}, "image"]
        })
    );

    let block = only_search_result(&result);
    assert_eq!(block.source, "https://cdn.example.com/report.pdf");
    assert_eq!(block.title, "Report");
    assert!(block.citations.is_some());
    let texts: Vec<&str> = block.content.iter().map(|t| t.text.as_str()).collect();
    // The first two paragraphs do not fit one block together; the heading starts a block; the
    // rest of the cap is cut into pieces of at most 1,500 characters.
    assert_eq!(texts[0], intro);
    assert_eq!(texts[1], second);
    assert_eq!(texts[2], "# Heading\nUnder the heading.");
    let before_long = format!("{intro}\n\n{second}\n# Heading\nUnder the heading.\n\n");
    let kept = 6000 - before_long.len();
    assert_eq!(texts[3], "x".repeat(1500));
    assert_eq!(texts[4], "x".repeat(1500));
    assert_eq!(texts[5], format!("{} [truncated]", "x".repeat(kept - 3000)));
    assert_eq!(texts[6], "Only pages 1 to 20 of 80 were read.");
    assert_eq!(texts[7], "Firecrawl warning: slow page");
    assert_eq!(texts.len(), 8);
    assert!(
        texts
            .iter()
            .all(|t| t.chars().count() <= 1500 + " [truncated]".len())
    );
}

#[tokio::test]
async fn fetch_page_error_status_is_error_result() {
    let (url, _) = serve(vec![scraped(
        "Not Found",
        json!({"statusCode": 404, "url": "https://example.com/missing"}),
    )])
    .await;
    let result = FirecrawlFetchPage::new(client(&url))
        .call(&fetch_use(json!({"url": "https://example.com/missing"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(string_content(&result).contains("404"), "{result:?}");
}

#[tokio::test]
async fn fetch_page_refuses_blocked_domains_and_redirects() {
    let (url, recorded) = serve(vec![scraped(
        "secret",
        json!({"statusCode": 200, "url": "https://evil.example.org/landing"}),
    )])
    .await;
    let fetch =
        FirecrawlFetchPage::new(client(&url)).with_blocked_domains(vec!["example.org".to_string()]);

    let refused = fetch
        .call(&fetch_use(json!({"url": "https://www.example.org/"})))
        .await
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
    assert!(recorded.lock().unwrap().is_empty());

    let redirected = fetch
        .call(&fetch_use(json!({"url": "https://example.com/start"})))
        .await
        .unwrap();
    assert_eq!(redirected.is_error, Some(true));
    assert!(string_content(&redirected).contains("redirected"));
    assert!(!string_content(&redirected).contains("secret"));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn fetch_page_payment_required_is_err() {
    let (url, recorded) = serve(vec![Canned::json(
        402,
        json!({"success": false, "error": "Insufficient credits"}).to_string(),
    )])
    .await;
    let err = FirecrawlFetchPage::new(client(&url))
        .call(&fetch_use(json!({"url": "https://example.com"})))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(402));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn fetch_page_blank_markdown_is_plain_text() {
    let (url, _) = serve(vec![scraped("  \n\n ", json!({"statusCode": 200}))]).await;
    let result = FirecrawlFetchPage::new(client(&url))
        .call(&fetch_use(json!({"url": "https://example.com/empty"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, None);
    assert_eq!(
        string_content(&result),
        "https://example.com/empty has no readable text."
    );
}

#[tokio::test]
async fn fetch_page_bad_input_and_failures_are_error_results() {
    let (url, recorded) = serve(vec![Canned::json(
        500,
        json!({"success": false, "error": "boom"}).to_string(),
    )])
    .await;
    let fetch = FirecrawlFetchPage::new(client(&url).with_max_retries(0));
    for input in [
        json!({}),
        json!({"url": "https://example.com", "extra": 1}),
        json!({"url": "not a url"}),
    ] {
        let result = fetch.call(&fetch_use(input.clone())).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{input}");
    }
    assert!(recorded.lock().unwrap().is_empty());
    let failed = fetch
        .call(&fetch_use(json!({"url": "https://example.com"})))
        .await
        .unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert!(string_content(&failed).starts_with("Could not fetch https://example.com: "));
    assert!(string_content(&failed).contains("boom"));
}

#[tokio::test]
async fn fetch_page_ignores_landed_urls_that_are_not_absolute() {
    let (url, recorded) = serve(vec![
        scraped("Body one.", json!({"statusCode": 200, "url": ""})),
        scraped(
            "Body two.",
            json!({"statusCode": 200, "url": "/landing", "sourceURL": "relative/page"}),
        ),
        scraped(
            "Body three.",
            json!({"statusCode": 200, "url": "mailto:someone@example.com"}),
        ),
    ])
    .await;
    let open = FirecrawlFetchPage::new(client(&url));
    let allowed =
        FirecrawlFetchPage::new(client(&url)).with_allowed_domains(vec!["example.com".to_string()]);
    for (fetch, text) in [
        (&open, "Body one."),
        (&open, "Body two."),
        (&allowed, "Body three."),
    ] {
        let result = fetch
            .call(&fetch_use(json!({"url": "https://example.com/start"})))
            .await
            .unwrap();
        assert_eq!(result.is_error, None, "{result:?}");
        let block = only_search_result(&result);
        assert_eq!(block.source, "https://example.com/start");
        assert_eq!(block.content[0].text, text);
    }
    assert_eq!(recorded.lock().unwrap().len(), 3);
}

#[test]
fn chunks_split_only_at_atx_headings_outside_fences() {
    let markdown = [
        "Intro.",
        "",
        "#hashtag is not a heading",
        "",
        "####### seven hashes is not a heading",
        "",
        "    # indented four spaces is code",
        "",
        "```bash",
        "# a shell comment",
        "",
        "# another comment",
        "~~~",
        "# still inside the backtick fence",
        "```",
        "",
        "~~~~",
        "# inside a tilde fence",
        "~~~~",
        "",
        "# Real heading",
        "Under it.",
        "",
        "###\tTabbed heading",
        "Under that.",
    ]
    .join("\n");
    assert_eq!(
        fetch_page::chunks(&markdown),
        vec![
            [
                "Intro.",
                "#hashtag is not a heading",
                "####### seven hashes is not a heading",
                "    # indented four spaces is code",
                "```bash\n# a shell comment",
                "# another comment\n~~~\n# still inside the backtick fence\n```",
                "~~~~\n# inside a tilde fence\n~~~~",
            ]
            .join("\n\n"),
            "# Real heading\nUnder it.".to_string(),
            "###\tTabbed heading\nUnder that.".to_string(),
        ]
    );
}

#[test]
fn chunks_never_leave_empty_pieces_or_trailing_blank_lines() {
    let indented = format!("  {}", "x".repeat(2000));
    let trailing = format!("{}   ", "y ".repeat(1000));
    let markdown = format!("Intro.\n\n{indented}\n\n{trailing}");
    let blocks = fetch_page::chunks(&markdown);
    for block in &blocks {
        assert!(!block.trim().is_empty(), "{blocks:?}");
        assert_eq!(block.trim_end(), block, "{blocks:?}");
        assert!(block.chars().count() <= 1500, "{blocks:?}");
    }
    assert_eq!(blocks[0], "Intro.");
    assert_eq!(blocks[1], format!("  {}", "x".repeat(1498)));
    assert_eq!(blocks[2], "x".repeat(502));
}

fn not_found() -> Canned {
    Canned::json(
        404,
        json!({"success": false, "error": "Resource not found", "code": "NOT_FOUND"}).to_string(),
    )
}

#[tokio::test]
async fn not_found_is_api_with_exact_display() {
    let (url, recorded) = serve(vec![not_found(), not_found()]).await;
    let err = client(&url)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err.kind(),
            ErrorKind::Api {
                service: Service::Firecrawl,
                status: Some(404),
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "Firecrawl API error: Resource not found (HTTP 404 NOT_FOUND)"
    );
    assert!(!err.is_retryable());
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn error_without_a_body_message_uses_the_shared_fallback() {
    let (url, _) = serve(vec![Canned::json(404, "<html>nope</html>")]).await;
    let err = client(&url)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Firecrawl API error: HTTP 404 with no error message (HTTP 404)"
    );
}

#[tokio::test]
async fn request_timeout_status_is_api_and_retried() {
    let timed_out = || Canned::json(408, json!({"success": false, "error": "slow"}).to_string());
    let (url, recorded) = serve(vec![timed_out(), ok(json!({"success": true}))]).await;
    client(&url).search(&SearchRequest::new("q")).await.unwrap();
    assert_eq!(recorded.lock().unwrap().len(), 2);

    let (url, _) = serve(vec![timed_out()]).await;
    let err = client(&url)
        .with_max_retries(0)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err.kind(),
            ErrorKind::Api {
                status: Some(408),
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.is_retryable());
}

#[tokio::test]
async fn unparseable_success_leaks_neither_text_nor_source() {
    let secret = "sk-secret-value-123";
    let body = json!({"success": secret}).to_string();
    let (url, _) = serve(vec![Canned::json(200, body.clone())]).await;
    let err = client(&url)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::Decode), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!(
            "Failed to parse Firecrawl response ({} bytes): type or value mismatch at line 1 \
             column {}",
            body.len(),
            body.len() - 1
        )
    );
    assert!(!format!("{err:?}").contains(secret), "{err:?}");
    assert!(err.source().is_none());
}

#[tokio::test]
async fn success_false_on_2xx_is_api_without_status() {
    let (url, _) = serve(vec![Canned::json(
        200,
        json!({"success": false, "error": "x", "code": "SCRAPE_FAILED"}).to_string(),
    )])
    .await;
    let err = client(&url)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err.kind(),
            ErrorKind::Api { service: Service::Firecrawl, status: None, error_code: Some(ErrorCode::Firecrawl(code)), .. }
                if code == "SCRAPE_FAILED"
        ),
        "{err:?}"
    );
    assert_eq!(err.status(), None);
    assert!(!err.is_retryable());
    assert_eq!(
        err.to_string(),
        "Firecrawl API error: x (success false, SCRAPE_FAILED)"
    );
}

#[tokio::test]
async fn fetch_page_unauthorized_is_err_and_not_found_is_an_error_result() {
    let (url, recorded) = serve(vec![Canned::json(
        401,
        json!({"success": false, "error": "Unauthorized"}).to_string(),
    )])
    .await;
    let err = FirecrawlFetchPage::new(client(&url))
        .call(&fetch_use(json!({"url": "https://example.com"})))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(401));
    assert_eq!(recorded.lock().unwrap().len(), 1);

    let (url, _) = serve(vec![not_found()]).await;
    let result = FirecrawlFetchPage::new(client(&url))
        .call(&fetch_use(json!({"url": "https://example.com"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));

    let (url, _) = serve(vec![not_found()]).await;
    let result = FirecrawlWebSearch::new(client(&url))
        .execute(&tool_use(json!({"query": "q"})))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
}

#[derive(Default)]
struct MemoryCache(Mutex<std::collections::HashMap<String, String>>);

impl FirecrawlCache for MemoryCache {
    fn get<'a>(&'a self, key: &'a str) -> futures::future::BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.0.lock().unwrap().get(key).cloned() })
    }

    fn put<'a>(
        &'a self,
        key: &'a str,
        body: String,
        _ttl: Duration,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().insert(key.to_string(), body);
        })
    }
}

fn cached_client(base_url: &str) -> (FirecrawlClient, Arc<MemoryCache>) {
    let cache = Arc::new(MemoryCache::default());
    let client = client(base_url).with_cache(cache.clone(), Duration::from_secs(60));
    (client, cache)
}

#[tokio::test]
async fn an_identical_scrape_is_served_from_the_cache() {
    let (url, recorded) = serve(vec![scraped("# Hi", json!({"title": "Hi"}))]).await;
    let (client, cache) = cached_client(&url);
    let request = ScrapeRequest::new("https://example.com/page");
    let first = client.scrape(&request).await.unwrap();
    let mut second = client.scrape(&request).await.unwrap();
    // The hit says it cost nothing; everything else is as cached.
    let metadata = second.data.metadata.as_mut().unwrap();
    assert_eq!(metadata.credits_used, Some(0));
    metadata.credits_used = None;
    assert_eq!(first, second);
    assert_eq!(recorded.lock().unwrap().len(), 1);
    let keys: Vec<String> = cache.0.lock().unwrap().keys().cloned().collect();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].starts_with("scrape:"), "{}", keys[0]);
}

#[tokio::test]
async fn an_identical_search_is_served_from_the_cache() {
    let (url, recorded) = serve(vec![ok(
        json!({"success": true, "data": {"web": [{"url": "https://a.test", "title": "A"}]}}),
    )])
    .await;
    let (client, _) = cached_client(&url);
    let request = SearchRequest::new("rust async");
    let first = client.search(&request).await.unwrap();
    let second = client.search(&request).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_different_url_misses_the_cache() {
    let (url, recorded) = serve(vec![scraped("one", json!({})), scraped("two", json!({}))]).await;
    let (client, _) = cached_client(&url);
    let one = client
        .scrape(&ScrapeRequest::new("https://example.com/one"))
        .await
        .unwrap();
    let two = client
        .scrape(&ScrapeRequest::new("https://example.com/two"))
        .await
        .unwrap();
    assert_eq!(one.data.markdown.as_deref(), Some("one"));
    assert_eq!(two.data.markdown.as_deref(), Some("two"));
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_failed_request_is_not_cached() {
    let (url, recorded) = serve(vec![
        Canned::json(400, json!({"success": false, "error": "bad"}).to_string()),
        scraped("fine", json!({})),
    ])
    .await;
    let (client, cache) = cached_client(&url);
    let request = ScrapeRequest::new("https://example.com/page");
    assert!(client.scrape(&request).await.is_err());
    assert!(cache.0.lock().unwrap().is_empty());
    let retry = client.scrape(&request).await.unwrap();
    assert_eq!(retry.data.markdown.as_deref(), Some("fine"));
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_page_with_an_error_status_is_not_cached() {
    let page = || scraped("gone", json!({"statusCode": 503}));
    let (url, recorded) = serve(vec![page(), page()]).await;
    let (client, cache) = cached_client(&url);
    let request = ScrapeRequest::new("https://example.com/page");
    client.scrape(&request).await.unwrap();
    client.scrape(&request).await.unwrap();
    assert!(cache.0.lock().unwrap().is_empty());
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_max_age_of_zero_bypasses_the_cache() {
    let (url, recorded) = serve(vec![scraped("old", json!({})), scraped("new", json!({}))]).await;
    let (client, _) = cached_client(&url);
    let warm = ScrapeRequest::new("https://example.com/page");
    client.scrape(&warm).await.unwrap();
    let mut fresh = ScrapeRequest::new("https://example.com/page");
    fresh.options.max_age = Some(Duration::ZERO);
    let page = client.scrape(&fresh).await.unwrap();
    assert_eq!(page.data.markdown.as_deref(), Some("new"));
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_cached_search_reports_zero_credits() {
    let (url, _) = serve(vec![ok(
        json!({"success": true, "creditsUsed": 2, "data": {"web": []}}),
    )])
    .await;
    let (client, _) = cached_client(&url);
    let request = SearchRequest::new("rust async");
    assert_eq!(client.search(&request).await.unwrap().credits_used, Some(2));
    assert_eq!(client.search(&request).await.unwrap().credits_used, Some(0));
}

#[tokio::test]
async fn search_sends_country_and_highlights_off() {
    let (url, recorded) = serve(vec![ok(json!({"success": true, "data": {"web": [
        {"url": "https://a.gov/1", "description": "the site's own description"}
    ]}}))])
    .await;
    let response = client(&url)
        .search(
            &SearchRequest::new("zoning variance")
                .with_country("US")
                .with_highlights(false),
        )
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 0),
        json!({"query": "zoning variance", "country": "US", "highlights": false})
    );
    assert_eq!(
        response.data.unwrap().web.unwrap()[0]
            .description
            .as_deref(),
        Some("the site's own description")
    );
}

#[tokio::test]
async fn flat_search_data_is_read_as_web_results() {
    let (url, _) = serve(vec![
        ok(json!({"success": true, "creditsUsed": 2, "data": [
            {"url": "https://a.gov/1", "title": "One", "position": 1, "markdown": "# One"},
            {"url": "https://a.gov/2", "title": "Two", "position": 2}
        ]})),
        ok(json!({"success": true, "data": []})),
    ])
    .await;
    let client = client(&url);
    let response = client.search(&SearchRequest::new("q")).await.unwrap();
    assert_eq!(response.credits_used, Some(2));
    let data = response.data.unwrap();
    assert_eq!(data.news, None);
    let web = data.web.unwrap();
    assert_eq!(web.len(), 2);
    assert_eq!(web[0].url.as_deref(), Some("https://a.gov/1"));
    assert_eq!(web[0].markdown.as_deref(), Some("# One"));
    assert_eq!(web[1].position, Some(2));

    let empty = client.search(&SearchRequest::new("q")).await.unwrap();
    assert_eq!(empty.data.unwrap().web, Some(Vec::new()));
}

#[tokio::test]
async fn gov_search_request_and_response() {
    let (url, recorded) = serve(vec![ok(json!({
        "success": true,
        "data": {"web": [
            {"url": "https://library.municode.com/tn/nashville/codes/code_of_ordinances",
             "title": "Title 17 - Zoning", "description": "variance from the setback",
             "position": 1, "newField": true},
            {"url": "https://www.ecfr.gov/x"}
        ]}
    }))])
    .await;
    let request = GovSearchRequest::new("setback variance")
        .with_k(25)
        .with_jurisdiction("us-tn")
        .with_court("tennctapp")
        .with_court_type(CourtType::StateAttorneyGeneral)
        .with_cites("Tenn. Code Ann. § 13-7-109")
        .with_domain("nashville.gov");
    let response = client(&url).gov_search(&request).await.unwrap();

    let recorded_request = &recorded.lock().unwrap()[0];
    let first_line = recorded_request.headers.lines().next().unwrap();
    assert!(
        first_line.starts_with("POST /v2/search/gov "),
        "{first_line}"
    );
    assert!(
        recorded_request
            .headers
            .lines()
            .any(|l| l.eq_ignore_ascii_case("authorization: Bearer fc-test-key"))
    );
    assert_eq!(
        serde_json::from_str::<Value>(&recorded_request.body).unwrap(),
        json!({
            "query": "setback variance",
            "k": 25,
            "jurisdiction": "us-tn",
            "court": "tennctapp",
            "courtType": "state-attorney-general",
            "cites": "Tenn. Code Ann. § 13-7-109",
            "domain": "nashville.gov"
        })
    );
    assert!(response.success);
    let web = &response.data.web;
    assert_eq!(web.len(), 2);
    assert_eq!(web[0].title.as_deref(), Some("Title 17 - Zoning"));
    assert_eq!(
        web[0].description.as_deref(),
        Some("variance from the setback")
    );
    assert_eq!(web[0].position, Some(1));
    assert_eq!(web[1].url.as_deref(), Some("https://www.ecfr.gov/x"));
    assert_eq!(web[1].title, None);
}

#[tokio::test]
async fn gov_search_minimal_body_and_empty_data() {
    let (url, recorded) = serve(vec![ok(json!({"success": true}))]).await;
    let response = client(&url)
        .gov_search(&GovSearchRequest::new("q"))
        .await
        .unwrap();
    assert_eq!(body_json(&recorded, 0), json!({"query": "q"}));
    assert!(response.data.web.is_empty());
}

#[tokio::test]
async fn gov_search_upstream_errors_are_retried() {
    for status in [502, 504] {
        let (url, recorded) = serve(vec![
            Canned::json(
                status,
                json!({"success": false, "error": "upstream"}).to_string(),
            ),
            ok(json!({"success": true, "data": {"web": [{"url": "https://a.gov"}]}})),
        ])
        .await;
        let response = client(&url)
            .gov_search(&GovSearchRequest::new("q"))
            .await
            .unwrap_or_else(|err| panic!("{status}: {err:?}"));
        assert_eq!(response.data.web.len(), 1, "{status}");
        assert_eq!(recorded.lock().unwrap().len(), 2, "{status}");
    }

    let (url, _) = serve(vec![Canned::json(
        502,
        json!({"success": false, "error": "upstream"}).to_string(),
    )])
    .await;
    let err = client(&url)
        .with_max_retries(0)
        .gov_search(&GovSearchRequest::new("q"))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(502));
    assert!(err.is_retryable());
}

#[tokio::test]
async fn gov_search_errors_keep_their_status_and_retryability() {
    for (status, code, retryable) in [
        (400, "BAD_REQUEST", false),
        (401, "UNAUTHORIZED", false),
        (429, "RATE_LIMIT", true),
    ] {
        let (url, _) = serve(vec![Canned::json(
            status,
            json!({"success": false, "error": "no", "code": code}).to_string(),
        )])
        .await;
        let err = client(&url)
            .with_max_retries(0)
            .gov_search(&GovSearchRequest::new("q"))
            .await
            .unwrap_err();
        assert_eq!(err.status(), Some(status));
        assert_eq!(err.is_retryable(), retryable, "{status}");
        assert_eq!(
            err.to_string(),
            format!("Firecrawl API error: no (HTTP {status} {code})")
        );
    }
}

#[tokio::test]
async fn gov_search_jurisdiction_is_sent_in_lowercase() {
    let (url, recorded) = serve(vec![ok(json!({"success": true}))]).await;
    client(&url)
        .gov_search(&GovSearchRequest::new("q").with_jurisdiction("US-TN"))
        .await
        .unwrap();
    assert_eq!(
        body_json(&recorded, 0),
        json!({"query": "q", "jurisdiction": "us-tn"})
    );
}

#[tokio::test]
async fn gov_search_validation_errors_are_raised_before_sending() {
    let client = client("http://127.0.0.1:9");
    let cases = [
        GovSearchRequest::new(" "),
        GovSearchRequest::new("q").with_k(0),
        GovSearchRequest::new("q").with_k(101),
        GovSearchRequest::new("q").with_jurisdiction("us-nashville"),
        GovSearchRequest::new("q").with_jurisdiction("ca"),
        GovSearchRequest::new("q").with_jurisdiction("us-"),
        GovSearchRequest::new("q").with_court(""),
        GovSearchRequest::new("q").with_cites(" "),
        GovSearchRequest::new("q").with_domain(""),
    ];
    for request in cases {
        let err = client.gov_search(&request).await.unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::Validation { .. }),
            "{request:?}: {err:?}"
        );
    }
    for jurisdiction in ["us", "US", "us-tn", "US-TN", "us-pr"] {
        assert!(
            GovSearchRequest::new("q")
                .with_jurisdiction(jurisdiction)
                .with_k(100)
                .validate()
                .is_ok(),
            "{jurisdiction}"
        );
    }
}

#[tokio::test]
async fn map_with_search_request_and_response() {
    let (url, recorded) = serve(vec![ok(json!({
        "success": true,
        "links": [
            {"url": "https://www.nashville.gov/departments/planning/agendas",
             "title": "Planning Commission Agendas", "description": "Meeting agendas"},
            {"url": "https://www.nashville.gov/minutes"}
        ]
    }))])
    .await;
    let request = MapRequest::new(" https://www.nashville.gov ")
        .with_search("planning commission agenda")
        .with_limit(100)
        .with_sitemap(SitemapMode::Only)
        .with_location(MapLocation::new("US").with_languages(vec!["en-US".to_string()]));
    let response = client(&url).map(&request).await.unwrap();

    let first_line = recorded.lock().unwrap()[0]
        .headers
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert!(first_line.starts_with("POST /v2/map "), "{first_line}");
    assert_eq!(
        body_json(&recorded, 0),
        json!({
            "url": "https://www.nashville.gov",
            "search": "planning commission agenda",
            "limit": 100,
            "sitemap": "only",
            "location": {"country": "US", "languages": ["en-US"]}
        })
    );
    assert_eq!(response.links.len(), 2);
    assert_eq!(
        response.links[0].title.as_deref(),
        Some("Planning Commission Agendas")
    );
    assert_eq!(
        response.links[0].description.as_deref(),
        Some("Meeting agendas")
    );
    assert_eq!(
        response.links[1].url.as_deref(),
        Some("https://www.nashville.gov/minutes")
    );
    assert_eq!(response.links[1].title, None);
}

#[tokio::test]
async fn map_minimal_body_and_errors() {
    let (url, recorded) = serve(vec![
        ok(json!({"success": true})),
        Canned::json(
            402,
            json!({"success": false, "error": "Insufficient credits"}).to_string(),
        ),
    ])
    .await;
    let client = client(&url);
    let response = client
        .map(&MapRequest::new("https://example.com").with_location(MapLocation::new("US")))
        .await
        .unwrap();
    assert!(response.links.is_empty());
    assert_eq!(
        body_json(&recorded, 0),
        json!({"url": "https://example.com", "location": {"country": "US"}})
    );

    let err = client
        .map(&MapRequest::new("https://example.com"))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(402));
    assert!(err.to_string().contains("Insufficient credits"));
    assert_eq!(recorded.lock().unwrap().len(), 2, "402 must not be retried");
}

#[tokio::test]
async fn map_validation_errors_are_raised_before_sending() {
    let client = client("http://127.0.0.1:9");
    let cases = [
        MapRequest::new("example.com"),
        MapRequest::new("ftp://example.com"),
        MapRequest::new("https://example.com").with_limit(0),
        MapRequest::new("https://example.com").with_limit(100_001),
        MapRequest::new("https://example.com").with_location(MapLocation::new("us")),
        MapRequest::new("https://example.com").with_location(MapLocation::new("USA")),
    ];
    for request in cases {
        let err = client.map(&request).await.unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::Validation { .. }),
            "{request:?}: {err:?}"
        );
    }
    assert!(
        MapRequest::new("https://example.com")
            .with_limit(100_000)
            .validate()
            .is_ok()
    );
}

#[tokio::test]
async fn credit_usage_is_a_get_and_reads_the_balance() {
    let (url, recorded) = serve(vec![ok(json!({
        "success": true,
        "data": {
            "remainingCredits": 98123,
            "planCredits": 100000,
            "billingPeriodStart": "2026-10-01T00:00:00Z",
            "billingPeriodEnd": null
        }
    }))])
    .await;
    let usage = client(&url).credit_usage().await.unwrap();

    let recorded_request = &recorded.lock().unwrap()[0];
    let first_line = recorded_request.headers.lines().next().unwrap();
    assert!(
        first_line.starts_with("GET /v2/team/credit-usage "),
        "{first_line}"
    );
    assert!(
        recorded_request
            .headers
            .lines()
            .any(|l| l.eq_ignore_ascii_case("authorization: Bearer fc-test-key"))
    );
    assert!(recorded_request.body.is_empty());
    assert_eq!(usage.remaining_credits, 98123.0);
    assert_eq!(usage.plan_credits, Some(100_000.0));
    assert_eq!(
        usage.billing_period_start.as_deref(),
        Some("2026-10-01T00:00:00Z")
    );
    assert_eq!(usage.billing_period_end, None);
}

#[tokio::test]
async fn credit_usage_failures() {
    let (url, recorded) = serve(vec![
        Canned::json(500, json!({"success": false, "error": "boom"}).to_string()),
        ok(json!({"success": true, "data": {"remainingCredits": 5}})),
    ])
    .await;
    let usage = client(&url).credit_usage().await.unwrap();
    assert_eq!(usage.remaining_credits, 5.0);
    assert_eq!(usage.plan_credits, None);
    assert_eq!(recorded.lock().unwrap().len(), 2);

    let (url, _) = serve(vec![Canned::json(
        404,
        json!({"success": false, "error": "Could not find credit usage information"}).to_string(),
    )])
    .await;
    let err = client(&url).credit_usage().await.unwrap_err();
    assert_eq!(err.status(), Some(404));
    assert!(!err.is_retryable());

    let (url, _) = serve(vec![ok(json!({"success": false, "error": "no team"}))]).await;
    let err = client(&url).credit_usage().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "Firecrawl API error: no team (success false)"
    );

    let (url, _) = serve(vec![ok(json!({"success": true}))]).await;
    let err = client(&url).credit_usage().await.unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::Decode), "{err:?}");
}

#[tokio::test]
async fn credit_usage_is_resent_after_a_cut_success_but_search_is_not() {
    let cut = || Canned::json(200, r#"{"success":true,"data""#).with_cut_body();
    let balance = || ok(json!({"success": true, "data": {"remainingCredits": 7}}));
    let (url, recorded) = serve(vec![cut(), balance()]).await;
    let usage = client(&url).credit_usage().await.unwrap();
    assert_eq!(usage.remaining_credits, 7.0);
    assert_eq!(recorded.lock().unwrap().len(), 2);

    let (url, recorded) = serve(vec![cut(), balance()]).await;
    let err = client(&url)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::Interrupted), "{err:?}");
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn gov_search_and_map_are_cached_but_credit_usage_is_not() {
    let (url, recorded) = serve(vec![
        ok(json!({"success": true, "data": {"web": [{"url": "https://a.gov", "position": 1}]}})),
        ok(json!({"success": true, "links": [{"url": "https://a.gov/agendas"}]})),
        ok(json!({"success": true, "data": {"remainingCredits": 9}})),
        ok(json!({"success": true, "data": {"remainingCredits": 8}})),
    ])
    .await;
    let (client, cache) = cached_client(&url);
    let gov = GovSearchRequest::new("setback").with_jurisdiction("us-tn");
    let first = client.gov_search(&gov).await.unwrap();
    assert_eq!(client.gov_search(&gov).await.unwrap(), first);

    let map = MapRequest::new("https://a.gov").with_search("agenda");
    let first = client.map(&map).await.unwrap();
    assert_eq!(first.credits_used, None);
    // The hit says it cost nothing; everything else is as cached.
    let mut second = client.map(&map).await.unwrap();
    assert_eq!(second.credits_used, Some(0));
    second.credits_used = None;
    assert_eq!(second, first);

    assert_eq!(client.credit_usage().await.unwrap().remaining_credits, 9.0);
    assert_eq!(client.credit_usage().await.unwrap().remaining_credits, 8.0);
    assert_eq!(recorded.lock().unwrap().len(), 4);

    let mut keys: Vec<String> = cache.0.lock().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys[0].starts_with("gov_search:"), "{}", keys[0]);
    assert!(keys[1].starts_with("map:"), "{}", keys[1]);
}

#[tokio::test]
async fn a_flat_search_answer_comes_back_the_same_from_the_cache() {
    let (url, recorded) = serve(vec![ok(json!({"success": true, "data": [
        {"url": "https://a.gov/1", "description": "a passage", "position": 1}
    ]}))])
    .await;
    let (client, _) = cached_client(&url);
    let request = SearchRequest::new("q");
    let first = client.search(&request).await.unwrap();
    let second = client.search(&request).await.unwrap();
    assert_eq!(second, first);
    assert_eq!(
        second.data.unwrap().web.unwrap()[0].description.as_deref(),
        Some("a passage")
    );
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_different_gov_query_or_map_search_misses_the_cache() {
    let gov_answer = || ok(json!({"success": true, "data": {"web": []}}));
    let map_answer = || ok(json!({"success": true, "links": []}));
    let (url, recorded) = serve(vec![
        gov_answer(),
        gov_answer(),
        gov_answer(),
        map_answer(),
        map_answer(),
    ])
    .await;
    let (client, cache) = cached_client(&url);
    let gov = GovSearchRequest::new("setback").with_jurisdiction("us-tn");
    client.gov_search(&gov).await.unwrap();
    client.gov_search(&gov.clone().with_k(20)).await.unwrap();
    client
        .gov_search(&gov.with_jurisdiction("us-ga"))
        .await
        .unwrap();
    let map = MapRequest::new("https://a.gov").with_search("agenda");
    client.map(&map).await.unwrap();
    client.map(&map.with_search("minutes")).await.unwrap();
    assert_eq!(recorded.lock().unwrap().len(), 5);
    assert_eq!(cache.0.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn credit_usage_reads_fractional_balances() {
    let (url, _) = serve(vec![ok(json!({
        "success": true,
        "data": {"remainingCredits": 1000.0, "planCredits": 99.5}
    }))])
    .await;
    let usage = client(&url).credit_usage().await.unwrap();
    assert_eq!(usage.remaining_credits, 1000.0);
    assert_eq!(usage.plan_credits, Some(99.5));
}

#[test]
fn pdf_options_and_links_go_into_the_wire_body() {
    let wire = |options: ScrapeOptions| {
        let request = ScrapeRequest::new("https://example.com/a.pdf").with_options(options);
        serde_json::to_value(request.wire()).unwrap()
    };
    assert_eq!(
        wire(
            ScrapeOptions::new()
                .with_pdf_mode(PdfMode::Ocr)
                .with_pdf_max_pages(40)
                .with_pdf_pages(true)
                .with_pdf_page_markers(true)
                .with_links(true)
        ),
        json!({
            "url": "https://example.com/a.pdf",
            "formats": ["markdown", "links"],
            "parsers": [{
                "type": "pdf",
                "mode": "ocr",
                "maxPages": 40,
                "pages": true,
                "pageMarkers": true
            }, "image"]
        })
    );
    for (mode, name) in [(PdfMode::Fast, "fast"), (PdfMode::Auto, "auto")] {
        assert_eq!(
            wire(ScrapeOptions::new().with_pdf_mode(mode))["parsers"],
            json!([{"type": "pdf", "mode": name}, "image"])
        );
    }
    assert_eq!(
        wire(ScrapeOptions::new().with_pdf_page_markers(false))["parsers"],
        json!([{"type": "pdf", "pageMarkers": false}, "image"])
    );
    let plain = wire(ScrapeOptions::new().with_links(false));
    assert_eq!(plain["formats"], json!(["markdown"]));
    assert!(plain.get("parsers").is_none(), "{plain}");
}

#[tokio::test]
async fn scrape_reads_links_and_pdf_pages() {
    let (url, _) = serve(vec![ok(json!({"success": true, "data": {
        "markdown": "one\n\n---\n\n<!-- page 2 -->\n\ntwo",
        "links": ["https://example.com/a", "https://example.com/b"],
        "pages": [
            {"pageNumber": 1, "markdown": "one"},
            {"pageNumber": 2, "markdown": "two"}
        ],
        "metadata": {"contentType": "application/pdf", "numPages": 2, "creditsUsed": 3}
    }}))])
    .await;
    let options = ScrapeOptions::new()
        .with_pdf_pages(true)
        .with_pdf_page_markers(true)
        .with_links(true);
    let request = ScrapeRequest::new("https://example.com/a.pdf").with_options(options.clone());
    let data = client(&url).scrape(&request).await.unwrap().data;
    assert_eq!(
        data.links,
        Some(vec![
            "https://example.com/a".to_string(),
            "https://example.com/b".to_string()
        ])
    );
    let pages = data.pages.as_deref().unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!((pages[1].page, pages[1].markdown.as_str()), (2, "two"));
    assert_eq!(data.metadata.as_ref().unwrap().credits_used, Some(3));
    assert_eq!(data.credits(&options), 3);
}

#[test]
fn credits_are_estimated_from_the_pages_read() {
    let data = |metadata: Value| -> ScrapeData {
        serde_json::from_value(json!({"markdown": "x", "metadata": metadata})).unwrap()
    };
    let options = ScrapeOptions::new();
    assert_eq!(
        data(json!({"contentType": "text/html"})).credits(&options),
        1
    );
    assert_eq!(
        data(json!({"contentType": "application/pdf", "numPages": 40, "totalPages": 300}))
            .credits(&options),
        40
    );
    assert_eq!(data(json!({"numPages": 1})).credits(&options), 1);
    assert_eq!(data(json!({"numPages": 0})).credits(&options), 1);
    assert_eq!(ScrapeData::default().credits(&options), 1);
    assert_eq!(
        data(json!({"numPages": 40})).credits(&ScrapeOptions::new().with_links(true)),
        40
    );
    // Firecrawl's own figure wins over the estimate.
    assert_eq!(
        data(json!({"numPages": 40, "creditsUsed": 7})).credits(&options),
        7
    );
}

#[tokio::test]
async fn a_cached_scrape_reports_zero_credits() {
    let (url, recorded) = serve(vec![scraped(
        "pdf",
        json!({"contentType": "application/pdf", "numPages": 40, "creditsUsed": 40}),
    )])
    .await;
    let (client, _) = cached_client(&url);
    let request = ScrapeRequest::new("https://example.com/a.pdf");
    let fresh = client.scrape(&request).await.unwrap();
    assert_eq!(fresh.data.credits(&request.options), 40);
    let cached = client.scrape(&request).await.unwrap();
    assert_eq!(cached.data.credits(&request.options), 0);
    assert_eq!(cached.data.markdown.as_deref(), Some("pdf"));
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn new_scrape_options_miss_the_cache() {
    let (url, recorded) = serve(vec![
        scraped("plain", json!({})),
        ok(json!({"success": true, "data": {"markdown": "linked", "links": ["https://a.test"]}})),
        scraped("ocr", json!({})),
        scraped("marked", json!({})),
    ])
    .await;
    let (client, cache) = cached_client(&url);
    let page = "https://example.com/page";
    let scrape = |options: ScrapeOptions| {
        let request = ScrapeRequest::new(page).with_options(options);
        let client = client.clone();
        async move { client.scrape(&request).await.unwrap().data }
    };
    assert_eq!(scrape(ScrapeOptions::new()).await.links, None);
    let linked = scrape(ScrapeOptions::new().with_links(true)).await;
    assert_eq!(linked.links, Some(vec!["https://a.test".to_string()]));
    scrape(ScrapeOptions::new().with_pdf_mode(PdfMode::Ocr)).await;
    scrape(ScrapeOptions::new().with_pdf_page_markers(true)).await;
    assert_eq!(recorded.lock().unwrap().len(), 4);
    assert_eq!(cache.0.lock().unwrap().len(), 4);
    // Each is now a hit under its own options.
    let again = scrape(ScrapeOptions::new().with_links(true)).await;
    assert_eq!(again.markdown.as_deref(), Some("linked"));
    assert_eq!(recorded.lock().unwrap().len(), 4);
}

#[test]
fn scrape_http_timeout_is_the_scrape_timeout_plus_a_margin() {
    use super::client::scrape_http_timeout;
    let secs = Duration::from_secs;
    assert_eq!(scrape_http_timeout(None), None);
    assert_eq!(scrape_http_timeout(Some(secs(30))), Some(secs(120)));
    assert_eq!(scrape_http_timeout(Some(secs(90))), Some(secs(120)));
    assert_eq!(scrape_http_timeout(Some(secs(91))), Some(secs(121)));
    assert_eq!(scrape_http_timeout(Some(secs(300))), Some(secs(330)));
}

/// Serves `count` scrapes on concurrent connections, each answered after `delay`, and returns
/// the most requests that were open at once.
async fn serve_concurrently(
    count: usize,
    delay: Duration,
) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let open = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let peak = most.clone();
    tokio::spawn(async move {
        for _ in 0..count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (open, most) = (open.clone(), most.clone());
            tokio::spawn(async move {
                crate::test_support::read_request(&mut socket).await;
                let now = open.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                // Counted as closed before the reply, so the next request cannot be counted early.
                open.fetch_sub(1, Ordering::SeqCst);
                let body = json!({"success": true, "data": {"markdown": "x"}}).to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            });
        }
    });
    (format!("http://{address}"), peak)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_concurrency_caps_the_open_requests() {
    let (url, peak) = serve_concurrently(12, Duration::from_millis(100)).await;
    let client = client(&url).with_max_concurrency(5);
    let scrapes = (0..12).map(|i| {
        let client = client.clone();
        async move {
            client
                .scrape(&ScrapeRequest::new(format!("https://example.com/{i}")))
                .await
        }
    });
    for response in futures::future::join_all(scrapes).await {
        assert_eq!(response.unwrap().data.markdown.as_deref(), Some("x"));
    }
    let peak = peak.load(std::sync::atomic::Ordering::SeqCst);
    assert!((2..=5).contains(&peak), "{peak} requests were open at once");
}

#[tokio::test]
async fn a_cache_hit_does_not_wait_for_a_slot() {
    let (url, recorded) = serve(vec![
        scraped("cached", json!({})),
        scraped("slow", json!({})).with_stall(Duration::from_secs(2)),
    ])
    .await;
    let (client, _) = cached_client(&url);
    let client = client.with_max_concurrency(1);
    let cached = ScrapeRequest::new("https://example.com/cached");
    client.scrape(&cached).await.unwrap();
    let slow = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .scrape(&ScrapeRequest::new("https://example.com/slow"))
                .await
        })
    };
    while recorded.lock().unwrap().len() < 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let hit = tokio::time::timeout(Duration::from_millis(500), client.scrape(&cached))
        .await
        .expect("a cache hit waited for the only slot")
        .unwrap();
    assert_eq!(hit.data.markdown.as_deref(), Some("cached"));
    assert!(!slow.is_finished());
    assert_eq!(
        slow.await.unwrap().unwrap().data.markdown.as_deref(),
        Some("slow")
    );
}

#[tokio::test]
async fn every_firecrawl_call_waits_for_a_slot() {
    let (url, recorded) = serve(vec![
        scraped("slow", json!({})).with_stall(Duration::from_secs(1)),
        ok(json!({"success": true, "data": {"remainingCredits": 5}})),
    ])
    .await;
    let client = client(&url).with_max_concurrency(1);
    let slow = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .scrape(&ScrapeRequest::new("https://example.com/slow"))
                .await
        })
    };
    while recorded.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let waited = tokio::time::timeout(Duration::from_millis(300), client.credit_usage()).await;
    assert!(waited.is_err(), "credit usage did not wait for the slot");
    slow.await.unwrap().unwrap();
    client.credit_usage().await.unwrap();
}

#[tokio::test]
async fn a_cache_hit_without_a_credits_figure_reports_zero() {
    let (url, recorded) = serve(vec![
        scraped("pdf", json!({"numPages": 40})),
        ok(json!({"success": true, "data": {"markdown": "bare"}})),
    ])
    .await;
    let (client, _) = cached_client(&url);
    let options = ScrapeOptions::new();
    for page in ["https://example.com/a.pdf", "https://example.com/bare"] {
        let request = ScrapeRequest::new(page);
        let fresh = client.scrape(&request).await.unwrap();
        assert_eq!(
            fresh.data.metadata.as_ref().and_then(|m| m.credits_used),
            None
        );
        assert!(fresh.data.credits(&options) > 0);
        let hit = client.scrape(&request).await.unwrap();
        assert_eq!(hit.data.credits(&options), 0, "{page}");
    }
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

#[test]
fn the_image_parser_is_sent_with_every_pdf_entry() {
    let options = [
        ScrapeOptions::new().with_pdf_max_pages(5),
        ScrapeOptions::new().with_pdf_mode(PdfMode::Fast),
        ScrapeOptions::new().with_pdf_pages(false),
        ScrapeOptions::new().with_pdf_page_markers(true),
    ];
    for options in options {
        let parsers = serde_json::to_value(options.wire()).unwrap()["parsers"].clone();
        let parsers = parsers.as_array().unwrap();
        assert_eq!(parsers.len(), 2, "{options:?}");
        assert_eq!(parsers[0]["type"], "pdf");
        assert_eq!(parsers[1], "image");
    }
    let search =
        SearchRequest::new("q").with_scrape_options(ScrapeOptions::new().with_pdf_max_pages(5));
    assert_eq!(
        serde_json::to_value(search.wire()).unwrap()["scrapeOptions"]["parsers"][1],
        "image"
    );
}

#[test]
fn a_pdf_page_without_a_number_does_not_fail_the_response() {
    let data: ScrapeData = serde_json::from_value(json!({
        "pages": [{"markdown": "first"}, {"pageNumber": 2, "markdown": "second"}]
    }))
    .unwrap();
    let pages = data.pages.unwrap();
    assert_eq!((pages[0].page, pages[0].markdown.as_str()), (0, "first"));
    assert_eq!(pages[1].page, 2);
}
