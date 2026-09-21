#[cfg(test)]
mod tests {
    use claudius::Error;
    use opentelemetry::trace::{Span, Status, Tracer, TracerProvider};
    use opentelemetry_otlp::{Protocol, WithExportConfig};
    use std::io::{Read, Write};

    #[test]
    fn sentry_captures_error_source_chain() {
        let error = Error::connection("connection failed", Some(Box::new(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "peer reset connection"))));
        let events = sentry::test::with_captured_events(|| { sentry::capture_error(&error); });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].exception.len(), 2);
        assert_eq!(events[0].exception[0].value.as_deref(), Some("peer reset connection"));
        println!("Sentry captured source chain: {:?}", events[0].exception);
    }

    #[test]
    fn sentry_does_not_automatically_attach_api_metadata() {
        let error = Error::api(429, Some("rate_limit_error".into()), "retry later".into(), Some("req_demo".into()));
        let events = sentry::test::with_captured_events(|| { sentry::capture_error(&error); });
        assert_eq!(events.len(), 1);
        assert!(events[0].tags.is_empty());
        assert!(!events[0].contexts.contains_key("anthropic"));
        assert!(!events[0].contexts.contains_key("http"));
        assert!(events[0].extra.is_empty());
        println!("Sentry event requires explicit API metadata enrichment: {:?}", events[0].exception);
    }

    #[test]
    fn sdk_emits_no_spans_without_caller_instrumentation() {
        let exporter = opentelemetry_sdk::trace::InMemorySpanExporter::default();
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().with_simple_exporter(exporter.clone()).build();
        opentelemetry::global::set_tracer_provider(provider.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(async {
            let client = claudius::Anthropic::new(Some("dummy".into())).unwrap();
            let mut params = claudius::MessageCreateParams::simple("test", claudius::KnownModel::ClaudeHaiku45);
            params.max_tokens = 0;
            client.send(params).await
        });
        assert!(result.is_err());
        provider.force_flush().unwrap();
        assert!(exporter.get_finished_spans().unwrap().is_empty());
        provider.shutdown().unwrap();
    }

    #[test]
    fn manually_recorded_error_exports_over_otlp_http() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/traces", listener.local_addr().unwrap());
        let receiver = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = socket.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&chunk[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let len: usize = headers.lines().find_map(|l| l.strip_prefix("content-length:").map(|s| s.trim().parse().unwrap())).unwrap();
                    if request.len() >= end + 4 + len {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                        return serde_json::from_slice::<serde_json::Value>(&request[end+4..end+4+len]).unwrap();
                    }
                }
            }
        });
        let exporter = opentelemetry_otlp::SpanExporter::builder().with_http().with_protocol(Protocol::HttpJson).with_endpoint(endpoint).build().unwrap();
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().with_simple_exporter(exporter).build();
        let tracer = provider.tracer("claudius-review");
        let mut span = tracer.start("anthropic.messages");
        let error = Error::api(429, Some("rate_limit_error".into()), "retry later".into(), Some("req_demo".into()));
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
