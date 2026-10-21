# Topcoat SI review harness

Integration tests for the client against local servers: retries, streaming, error context, logging, metrics and telemetry. All network tests use synthetic credentials and localhost; no live Anthropic call is made.

Run from the repository root:

```sh
export CARGO_TARGET_DIR=/tmp/topcoat-si-review-target

env -u TOPCOAT_SI_API_KEY -u ANTHROPIC_API_KEY -u TOPCOAT_SI_BASE_URL -u ANTHROPIC_BASE_URL -u CLAUDIUS_DEBUG_STREAM mbx nextest run --manifest-path review-harness/Cargo.toml -p topcoat-si-review-harness --locked --no-fail-fast
```

Filter a suite by adding, for example, `-E 'test(review_streaming::)'`. Add `--lib` for the telemetry checks.

The crate depends on `tracing` and leaves exporter configuration to its host. The harness checks request spans, error source chains, retry breadcrumbs, concurrent parents, and streamed failure and cancellation with an in-memory OpenTelemetry exporter and a Sentry test transport. OpenTelemetry, `tracing-opentelemetry` and Sentry are dependencies of this harness only, not of the SDK.
