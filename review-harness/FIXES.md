# Claudius repairs

All 22 original findings and nine additional findings are repaired. The final implementation is `d12a7e4`, based on the eight packages in the authorized repair plan. Package 6 precedes package 5 so zero-token cache prewarming cannot bypass the agent's budget guard. Source remains a locally modified 0.33.0 vendor snapshot; this is not an upstream release.

## Commits and regressions

The initial snapshot is `5bf4a08`; `530888f` consolidates the harness and inverts six bug-asserting tests. `fdb105b` explicitly tracks the vendor lockfile, which the upstream ignore rule excluded from the first snapshot commit. Final telemetry, source-lint repairs, and expanded regression coverage landed in `d12a7e4`.

Test names below are in `tests/all/review_*.rs` unless qualified with `telemetry::` (harness `src/lib.rs`) or `backoff::tests::` (crate unit test).

| Finding | Repair | Commit | Regression test |
| --- | --- | --- | --- |
| 1 | Redact client Debug and mark both authentication headers sensitive | `3ec6902` | `client_debug_must_redact_credentials`, `baseten_client_debug_must_redact_credentials`, `auth_header_debug_redacts_both_credentials` |
| 2 | Require completed stream state; deliver one error to stream and completion receiver | `9304b16` | `eof_without_message_stop_must_not_succeed`, `stream_error_must_reach_final_message_receiver`, `stop_reason_is_sufficient_without_message_stop` |
| 3 | Terminal SSE failures end the stream | `effce47` | `oversized_buffer_must_terminate_after_error`, `malformed_event_terminates_before_later_event`, `every_sse_failure_is_counted_once` |
| 4 | Remove LoggingStream and expose completion failures through the ordinary stream | `9304b16`, `d12a7e4` | `public_stream_reports_missing_message_error`, `public_stream_reports_failure_once`, `telemetry::stream_failure_emits_one_error_and_closes_on_eof` |
| 5 | Count framing, transport, JSON and API-event failures once | `effce47` | `anthropic_stream_error_increments_error_counter`, `every_sse_failure_is_counted_once` |
| 6 | Preserve common HTTP status/type/request ID/retry metadata and distinguish overload | `eb210c2`, `d12a7e4` | `http_metadata_survives_each_error_kind`, `request_id_should_be_preserved_for_server_error`, `telemetry::stream_failure_emits_one_error_and_closes_on_eof` |
| 7 | Retry connect/pre-header disconnects and retryable HTTP statuses, including unreadable error bodies | `eb210c2` | `disconnected_connection_before_headers_should_retry`, `connection_refused_retries_when_listener_becomes_available`, `truncated_503_body_should_still_retry`, `retryable_statuses_retry_once` |
| 8 | Retain timeout source chains through one transport mapper | `eb210c2`, `d12a7e4` | `network_timeout_preserves_source_chain`, `telemetry::failed_request_exports_one_span_one_sentry_event_and_retry_breadcrumb` |
| 9 | Remove caller-poll timing from SSE processing | `effce47` | `slow_consumer_must_still_receive_buffered_events` (no sleep) |
| 10 | Accumulate server-tool JSON with the same builder as client tools | `9304b16` | `server_tool_json_deltas_must_be_accumulated` |
| 11 | Recognize earliest LF, CRLF or CR frame delimiter, including split delimiters | `effce47` | `crlf_is_a_valid_sse_line_ending`, `split_crlf_and_cr_frames_decode` |
| 12 | Preserve nested web-search error discriminator | `27b1393` | `web_search_error_round_trip_preserves_required_type` |
| 13 | Accept request_too_large and unknown web-search error codes | `27b1393` | `web_search_request_too_large_is_valid_response`, `future_response_enums_do_not_discard_messages` |
| 14 | Tagged message/fallback_message/compaction/advisor_message usage, optional model, unknown fallback | `27b1393` | `ordinary_usage_iteration_can_omit_model`, `all_usage_iteration_kinds_and_null_model_decode` |
| 15 | Remove allocation-heavy Debug-based message/system size rejection | `27b1393` | `accepts_base64_image_below_anthropic_limit` |
| 16 | Permit cache prewarming with zero output; reject incompatible generation options | `27b1393` | `cache_prewarm_allows_zero_output_tokens`, `cache_prewarm_rejects_output_producing_options` |
| 17 | Permit Bash restart without a command | `b02d2b3` | `documented_restart_invokes_callback` |
| 18 | Map editor -1 end sentinel to u32::MAX; reject other negative endpoints | `b02d2b3` | `documented_end_of_file_range_invokes_callback`, `invalid_negative_editor_range_is_rejected` |
| 19 | Keep billed answers on budget exhaustion and answer outstanding tool calls with error results | `b02d2b3` | `over_budget_completed_answer_is_preserved`, `over_budget_tool_calls_receive_error_results` |
| 20 | Return pause_turn to the outer budget guard | `b02d2b3` | `pause_turn_stops_after_budget_exhaustion` |
| 21 | Serialize the document source discriminator once | `27b1393` | `content_document_serializes_one_type_key` |
| 22 | Use exclusive PDF citation endpoint in helpers and documentation | `27b1393` | `single_pdf_page_uses_exclusive_endpoint` |
| N1 | Never retry timeouts; default non-streaming total deadline is 600 seconds | `eb210c2` | `timeout_does_not_repeat_a_billed_post`, `batch_timeout_is_not_retried`, `truncated_200_and_builder_errors_are_not_retried` |
| N2 | Backoff floor is 0.5 × 2^attempt seconds, positive jitter, capped at 8 seconds | `eb210c2` | `backoff::tests::backoff_floor_and_cap_without_sleep` |
| N3 | Honor seconds, HTTP-date and retry-after-ms; return delays above 60 seconds without sleeping | `eb210c2` | `http_date_retry_after_is_preserved_without_sleep`, `http_metadata_survives_each_error_kind` |
| N4 | Disable redirects so x-api-key cannot cross hosts | `3ec6902` | `redirect_does_not_forward_api_key` |
| N5 | Delete legacy debug flag body dumps; explicit TRACE wire target replaces them | `3ec6902`, `d12a7e4` | `telemetry::legacy_debug_flag_never_prints_wire_bodies` (subprocesses with both 0 and 1) |
| N6 | Reject sparse/unbounded block start indices before allocation | `9304b16` | `sparse_block_index_is_rejected_without_allocating` |
| N7 | Ignore future SSE event types; accept unknown response enums; retain ID and byte length on invalid 200 | `effce47`, `27b1393`, `d12a7e4` | `large_event_and_unknown_event_are_supported`, `future_response_enums_do_not_discard_messages`, `invalid_200_retains_request_id_and_length` |
| N8 | Allow events up to the existing 1 MiB buffer limit | `effce47` | `large_event_and_unknown_event_are_supported` (100 KiB web-search encrypted content) |
| N9 | Skip empty environment keys and trim selected credentials | `3ec6902` | `empty_primary_env_key_falls_back_and_trims` |

`fallback_message_does_not_make_empty_stream_successful` guards the separately identified agent fallback-message path. Repeated-poll assertions cover the stream wrappers after error and EOF. No listed defect was deferred.

## Reportable failures and Echt

`d12a7e4` removes ClientLogger and its success-only wrappers. Ordinary message, stream, token-count, batch and model operations emit `claudius.request` spans with operation/model/stream mode, attempt count, HTTP status, request ID, usage and error status when available. Retries emit warning breadcrumbs; a terminal request failure emits one error event retaining its source chain. Stream spans live until EOF, failure or drop. Prompt and response bodies appear only under the explicit `claudius::wire` TRACE target.

Bash, editor, search, unknown-tool and malformed-input failures report tool name and tool-use ID. Recoverable terminal/history/output failures warn with their source. Constructors and local validation return contextual errors for the caller to report. Production unwrap/expect are denied in the library and every executable crate root. The PTY wrapper returns invalid-input errors for empty/NUL-containing arguments. Missing model prices return validation errors; custom-model budgets require explicit rates instead of silently adopting another model's prices.

The following APIs intentionally change: the Error enum carries shared HttpMetadata and causal errors; with_base_url, KnownModel::token_rates, Budget::from_model/from_dollars_with_model and dependent chat configuration methods return Result. ClientLogger/send_with_logger/stream_with_logger and the old backoff tuning API are removed. Response enums have unknown variants. Callers must handle errors and new variants explicitly.

The host owns the OpenTelemetry exporter, Sentry initialization and flushing. Echt's local source documents `/otlp/v1/traces` with bearer authentication and standard Sentry envelope ingestion. Configure the host with its own endpoint/key or DSN; no credentials or exporter are embedded in Claudius. The tested subscriber composition is:

```rust
tracing_subscriber::registry()
    .with(tracing_opentelemetry::layer()
        .with_tracer(provider.tracer("claudius-host"))
        .with_filter(tracing_subscriber::filter::LevelFilter::INFO))
    .with(sentry_tracing::layer()
        .span_filter(|_| false)
        .event_filter(|metadata| match *metadata.level() {
            tracing::Level::ERROR => sentry_tracing::EventFilter::Event,
            tracing::Level::WARN => sentry_tracing::EventFilter::Breadcrumb,
            _ => sentry_tracing::EventFilter::Ignore,
        }))
```

This composition lets OpenTelemetry own spans and Sentry own error events/breadcrumbs. Disabling Sentry's span scopes avoids losing retry breadcrumbs when the same async request span is re-entered across polls with Sentry 0.49.3. Keep these layers and a correctly bound Sentry hub in the host; emitting a tracing event alone does not deliver it to Echt.

Regression coverage includes `telemetry::sdk_emits_spans_without_caller_instrumentation`, `telemetry::failed_request_exports_one_span_one_sentry_event_and_retry_breadcrumb`, `telemetry::concurrent_requests_keep_their_parents`, `telemetry::dropped_stream_closes_its_request_span`, and `telemetry::tool_failure_retains_source_and_tool_context_for_sentry`.

## Validation and implementation choices

- 63/63 harness tests passed; 685/685 library tests passed. Clippy passed with `--all-targets --features rustls-tls,binaries --no-default-features -- -D warnings`; formatting and whitespace checks passed.
- The installed ast-grep rules scanned 97 source files, with zero blocking findings. All 153 remaining advisory matches have explicit dispositions in [AST-GREP-REVIEW.md](AST-GREP-REVIEW.md); they are not suppressed or described as zero warnings.
- There are no deliberate test sleeps over one second. Multi-attempt policy tests use paused time where possible; maximum backoff is checked without sleeping.
- Only `tracing` was added to the SDK dependencies. HTTP-date parsing reuses its existing `time` dependency. The harness uses OpenTelemetry 0.32 with tracing-opentelemetry 0.33 because that published bridge requires 0.32; a matching 0.34 bridge was unavailable in the registry. Sentry integration stays in the harness.
- Non-streaming requests use a 600-second total deadline; streams retain a 60-second read-inactivity timeout, bounded by any shorter caller timeout. The default is corroborated by the [official Python SDK constants](https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/_constants.py). The four iteration variants are documented in the [official beta Messages API](https://platform.claude.com/docs/en/api/beta/messages/create).
- Build outputs stay outside the vendor snapshot. The sandbox prevented mbx from sweeping its shared global cache; those maintenance warnings did not fail tests or produce Rust/Clippy warnings.

No live Anthropic request, hosted Sentry project, production Echt collector, or native-TLS configuration was tested. The clipboard credential was not accessed and no paid model calls were made. Local OpenTelemetry/Sentry tests establish instrumentation and serialization, not production delivery. The invoice app is not yet wired to this crate; its exporter setup remains outside this repair's scope. Historical REVIEW.md intentionally remains the original audit.
