#[cfg(test)]
mod tests {
    use opentelemetry::trace::{Span, Status, Tracer, TracerProvider};
    use opentelemetry_otlp::{Protocol, WithExportConfig};
    use std::io::{Read, Write};
    use topcoat_si::Error;

    #[test]
    fn sentry_captures_error_source_chain() {
        let error = Error::connection(
            "connection failed",
            Some(Box::new(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "peer reset connection",
            ))),
        );
        let events = sentry::test::with_captured_events(|| {
            sentry::capture_error(&error);
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].exception.len(), 2);
        assert_eq!(
            events[0].exception[0].value.as_deref(),
            Some("peer reset connection")
        );
        println!("Sentry captured source chain: {:?}", events[0].exception);
    }

    #[test]
    fn sentry_does_not_automatically_attach_api_metadata() {
        let error = Error::api(
            429,
            Some("rate_limit_error".into()),
            "retry later".into(),
            Some("req_demo".into()),
        );
        let events = sentry::test::with_captured_events(|| {
            sentry::capture_error(&error);
        });
        assert_eq!(events.len(), 1);
        assert!(events[0].tags.is_empty());
        assert!(!events[0].contexts.contains_key("anthropic"));
        assert!(!events[0].contexts.contains_key("http"));
        assert!(events[0].extra.is_empty());
        println!(
            "Sentry event requires explicit API metadata enrichment: {:?}",
            events[0].exception
        );
    }

    #[test]
    fn manually_recorded_error_exports_over_otlp_http() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/traces", listener.local_addr().unwrap());
        let receiver = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = socket.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&chunk[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let len: usize = headers
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|s| s.trim().parse().unwrap())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + len {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                        return serde_json::from_slice::<serde_json::Value>(
                            &request[end + 4..end + 4 + len],
                        )
                        .unwrap();
                    }
                }
            }
        });
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .with_endpoint(endpoint)
            .build()
            .unwrap();
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_simple_exporter(exporter)
            .build();
        let tracer = provider.tracer("topcoat-si-review");
        let mut span = tracer.start("anthropic.messages");
        let error = Error::api(
            429,
            Some("rate_limit_error".into()),
            "retry later".into(),
            Some("req_demo".into()),
        );
        span.record_error(&error);
        span.set_status(Status::error(error.to_string()));
        span.end();
        provider.force_flush().unwrap();
        provider.shutdown().unwrap();
        let wire = receiver.join().unwrap();
        let span = &wire["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span["name"], "anthropic.messages");
        assert_eq!(span["status"]["code"], 2);
        assert_eq!(span["events"][0]["name"], "exception");
        println!("OTLP HTTP export captured exception and error status: {span}");
    }
}

#[cfg(test)]
mod telemetry {
    use futures::StreamExt;
    use opentelemetry::trace::{Status, TracerProvider};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use topcoat_si::*;
    use tracing::{Instrument, subscriber::with_default};
    use tracing_subscriber::{filter::LevelFilter, prelude::*};

    fn provider() -> (InMemorySpanExporter, SdkTracerProvider) {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (exporter, provider)
    }
    fn subscriber(provider: &SdkTracerProvider) -> impl tracing::Subscriber + Send + Sync {
        tracing_subscriber::registry()
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(provider.tracer("review"))
                    .with_filter(LevelFilter::INFO),
            )
            .with(
                sentry_tracing::layer()
                    .span_filter(|_| false)
                    .event_filter(|metadata| match *metadata.level() {
                        tracing::Level::ERROR => sentry_tracing::EventFilter::Event,
                        tracing::Level::WARN => sentry_tracing::EventFilter::Breadcrumb,
                        _ => sentry_tracing::EventFilter::Ignore,
                    }),
            )
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    async fn serve(responses: Vec<String>) -> (Anthropic, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = [0; 32768];
                socket.read(&mut data).await.unwrap();
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (
            Anthropic::new(Some("synthetic-secret".into()))
                .unwrap()
                .with_base_url(format!("http://{address}"))
                .unwrap()
                .with_max_retries(1),
            task,
        )
    }
    fn params() -> MessageCreateParams {
        MessageCreateParams::simple("synthetic-private-prompt", KnownModel::ClaudeHaiku55)
    }
    fn response(id: &str, stream: bool) -> String {
        let message = Message::new(
            id.into(),
            vec![],
            Model::Custom("test".into()),
            Usage::new(3, 2),
        )
        .with_stop_reason(StopReason::EndTurn);
        let body = if stream {
            format!(
                "event: message_start\ndata: {}\n\nevent: message_stop\ndata: {{}}\n\n",
                serde_json::json!({"message":message})
            )
        } else {
            serde_json::to_string(&message).unwrap()
        };
        format!(
            "HTTP/1.1 200 OK\r\nrequest-id: {id}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }
    fn attr(span: &opentelemetry_sdk::trace::SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .rfind(|a| a.key.as_str() == key)
            .map(|a| a.value.to_string())
    }

    #[test]
    fn sdk_emits_spans_without_caller_instrumentation() {
        let (exporter, provider) = provider();
        with_default(subscriber(&provider), || {
            runtime().block_on(async {
                let (client, server) = serve(vec![response("req_ok", false)]).await;
                client.send(params()).await.unwrap();
                server.await.unwrap();
            })
        });
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "topcoat_si.request");
        assert_eq!(attr(&spans[0], "request_id").as_deref(), Some("req_ok"));
        assert_eq!(attr(&spans[0], "input_tokens").as_deref(), Some("3"));
        assert_eq!(attr(&spans[0], "output_tokens").as_deref(), Some("2"));
        assert!(!format!("{spans:?}").contains("synthetic-private-prompt"));
        assert!(!format!("{spans:?}").contains("synthetic-secret"));
    }

    #[test]
    fn failed_request_exports_one_span_one_sentry_event_and_retry_breadcrumb() {
        let (exporter, provider) = provider();
        let events = sentry::test::with_captured_events(|| {
            with_default(subscriber(&provider), || {
                runtime().block_on(async {
            let broken = "HTTP/1.1 503 Unavailable\r\nrequest-id: req_failed\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{".to_string();
            let (client, server) = serve(vec![broken.clone(), broken]).await;
            let error = client.send(params()).await.unwrap_err();
            assert_eq!(error.request_id(), Some("req_failed"));
            server.await.unwrap();
        })
            })
        });
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(attr(&spans[0], "request_id").as_deref(), Some("req_failed"));
        assert_eq!(attr(&spans[0], "attempts").as_deref(), Some("2"));
        assert!(matches!(spans[0].status, Status::Error { .. }));
        assert_eq!(events.len(), 1);
        assert!(events[0].exception.len() >= 3, "{:?}", events[0].exception);
        assert_eq!(events[0].breadcrumbs.len(), 1);
        assert!(format!("{:?}", events[0].breadcrumbs).contains("Retrying API request"));
    }

    #[test]
    fn concurrent_requests_keep_their_parents() {
        let (exporter, provider) = provider();
        with_default(subscriber(&provider), || {
            runtime().block_on(async {
                let (a, sa) = serve(vec![response("req_a", false)]).await;
                let (b, sb) = serve(vec![response("req_b", false)]).await;
                let (a, b) = tokio::join!(
                    a.send(params()).instrument(tracing::info_span!("parent_a")),
                    b.send(params()).instrument(tracing::info_span!("parent_b"))
                );
                a.unwrap();
                b.unwrap();
                sa.await.unwrap();
                sb.await.unwrap();
            })
        });
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 4);
        for suffix in ["a", "b"] {
            let parent = spans
                .iter()
                .find(|s| s.name == format!("parent_{suffix}"))
                .unwrap();
            let request = spans
                .iter()
                .find(|s| attr(s, "request_id") == Some(format!("req_{suffix}")))
                .unwrap();
            assert_eq!(request.parent_span_id, parent.span_context.span_id());
        }
    }

    #[test]
    fn dropped_stream_closes_its_request_span() {
        let (exporter, provider) = provider();
        with_default(subscriber(&provider), || {
            runtime().block_on(async {
                let (client, server) = serve(vec![response("req_stream", true)]).await;
                let mut params = params();
                params.stream = true;
                let mut stream = Box::pin(client.stream(&params).await.unwrap());
                stream.next().await.unwrap().unwrap();
                assert!(exporter.get_finished_spans().unwrap().is_empty());
                drop(stream);
                server.await.unwrap();
            })
        });
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(attr(&spans[0], "request_id").as_deref(), Some("req_stream"));
        assert_eq!(attr(&spans[0], "output_tokens").as_deref(), Some("2"));
    }

    #[test]
    fn stream_failure_emits_one_error_and_closes_on_eof() {
        let (exporter, provider) = provider();
        let events = sentry::test::with_captured_events(|| {
            with_default(subscriber(&provider), || {
                runtime().block_on(async {
            let body = "event: error\ndata: {\"error\":{\"type\":\"overloaded_error\",\"message\":\"failed\"}}\n\n";
            let response = format!("HTTP/1.1 200 OK\r\nrequest-id: req_stream_failed\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let (client, server) = serve(vec![response]).await;
            let mut params = params(); params.stream = true;
            let mut stream = Box::pin(client.stream(&params).await.unwrap());
            assert_eq!(stream.next().await.unwrap().unwrap_err().request_id(), Some("req_stream_failed"));
            assert!(stream.next().await.is_none());
            assert!(stream.next().await.is_none());
            assert_eq!(exporter.get_finished_spans().unwrap().len(), 1);
            server.await.unwrap();
        })
            })
        });
        assert_eq!(events.len(), 1);
        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        assert!(matches!(spans[0].status, Status::Error { .. }));
    }
    #[test]
    fn debug_flag_child() {
        if std::env::var_os("TOPCOAT_SI_REVIEW_CHILD").is_none() {
            return;
        }
        runtime().block_on(async {
            let (client, server) = serve(vec![response("synthetic-private-response", true)]).await;
            let mut params = params();
            params.stream = true;
            let events: Vec<_> = client.stream(&params).await.unwrap().collect().await;
            assert!(events.iter().all(Result::is_ok));
            server.await.unwrap();
        });
    }

    #[test]
    fn legacy_debug_flag_never_prints_wire_bodies() {
        for value in ["0", "1"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "telemetry::debug_flag_child", "--nocapture"])
                .env("CLAUDIUS_DEBUG_STREAM", value)
                .env("TOPCOAT_SI_REVIEW_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let output = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!output.contains("synthetic-private-prompt"));
            assert!(!output.contains("synthetic-private-response"));
            assert!(!output.contains("synthetic-secret"));
        }
    }
}
