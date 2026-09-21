# Claudius repair evidence

The repaired vendored crate passes **63 harness tests and 685 library tests**. Clippy passes with warnings denied, including the executable targets. [FIXES.md](FIXES.md) maps all 22 original findings and nine additional findings to commits and tests. [AST-GREP-REVIEW.md](AST-GREP-REVIEW.md) records the source scan and dispositions of its remaining advisory matches.

[REVIEW.md](REVIEW.md) and the original logs describe the historical baseline: 10 passes and 21 failures. Correcting six tests that asserted bugs changed that baseline to 4 passes and 27 failures. The current harness asserts repaired behavior. Seven integration suites now share one test binary; nextest still isolates tests in separate processes for process-global metric and environment checks.

Run from the project root. All network tests use synthetic credentials and localhost. `mbx` requires Cargo, cargo-nextest, and the appropriate Rust toolchain.

```sh
export CARGO_TARGET_DIR=/tmp/claudius-review-target

env -u CLAUDIUS_API_KEY -u ANTHROPIC_API_KEY -u CLAUDIUS_BASE_URL -u ANTHROPIC_BASE_URL -u CLAUDIUS_DEBUG_STREAM mbx nextest run --manifest-path reviews/claudius/Cargo.toml -p claudius-review-harness --locked --no-fail-fast

env -u CLAUDIUS_API_KEY -u CLAUDIUS_BASE_URL -u ANTHROPIC_BASE_URL -u CLAUDIUS_DEBUG_STREAM ANTHROPIC_API_KEY=review-dummy-key mbx nextest run --manifest-path vendor/claudius/Cargo.toml -p claudius --lib --locked --no-default-features --features rustls-tls --no-fail-fast

mbx clippy --manifest-path vendor/claudius/Cargo.toml --all-targets --no-default-features --features rustls-tls,binaries --locked -- -D warnings
```

Filter an integration suite by adding, for example, `-E 'test(review_streaming::)'` to the harness command. Add `--lib` for telemetry checks. No test deliberately sleeps longer than one second; the old 31-second consumer test now exercises already-buffered events without sleeping. Retry policy tests use virtual time where possible and inspect request counts.

The crate depends on `tracing`, with exporter configuration left to its host. The harness checks automatic request spans, error source chains, retry breadcrumbs, concurrent parents, streamed failure/cancellation, and contextual tool errors with an in-memory OpenTelemetry exporter and Sentry test transport. A separate test serializes an exception through an OTLP/HTTP exporter to a local receiver.

OpenTelemetry API/SDK/OTLP 0.32 are paired with `tracing-opentelemetry` 0.33; Sentry and `sentry-tracing` use 0.49.3. These dependencies belong to this unpublished harness, not the vendored SDK. See [FIXES.md](FIXES.md) for host configuration and validation limits.

Final-run summaries are in [logs/fixed-validation.txt](logs/fixed-validation.txt). Historical logs remain unchanged. No live Anthropic call, hosted Sentry project, production Echt collector, or native-TLS run was used.
