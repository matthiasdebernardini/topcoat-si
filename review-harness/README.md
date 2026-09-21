# Claudius review evidence

Use [the Claude review prompt](../../CLAUDE-CLAUDIUS-REVIEW.md) for the independent audit. [REVIEW.md](REVIEW.md) contains the original 22 findings and source references. The pinned production source is in [vendor/claudius](../../vendor/claudius/); the relative Cargo dependency in this directory points there. Tests are kept outside the upstream crate.

The initial review found 685 existing library tests passing with a dummy API key. The review harness has 31 checks: 27 reproductions plus four telemetry checks. Verified after vendoring with nextest: **21 fail and 10 pass**, matching the expected results before fixes. Six passing reproductions deliberately assert faulty behavior; four passing telemetry checks demonstrate manual interoperability and its limits. An all-green run is not the expected baseline.

Run from the snapshot root with cargo-nextest installed. Clear inherited API endpoint/key settings and put build output outside the snapshot. The fake environment key is needed only by some original library tests; all review network tests use local endpoints and explicit dummy keys.

```sh
export CARGO_TARGET_DIR="${TMPDIR:-/tmp}/claudius-review-target"
```

Original library baseline:

```sh
env -u CLAUDIUS_API_KEY -u CLAUDIUS_BASE_URL -u ANTHROPIC_BASE_URL -u CLAUDIUS_DEBUG_STREAM ANTHROPIC_API_KEY=review-dummy-key cargo nextest run --manifest-path vendor/claudius/Cargo.toml -p claudius --lib --locked --no-default-features --features rustls-tls
```

Complete independent harness (expected nonzero exit because it reproduces defects):

```sh
env -u CLAUDIUS_API_KEY -u ANTHROPIC_API_KEY -u CLAUDIUS_BASE_URL -u ANTHROPIC_BASE_URL -u CLAUDIUS_DEBUG_STREAM cargo nextest run --manifest-path reviews/claudius/Cargo.toml -p claudius-review-harness --locked --no-fail-fast
```

Telemetry interoperability checks only:

```sh
env -u CLAUDIUS_API_KEY -u ANTHROPIC_API_KEY -u CLAUDIUS_BASE_URL -u ANTHROPIC_BASE_URL -u CLAUDIUS_DEBUG_STREAM cargo nextest run --manifest-path reviews/claudius/Cargo.toml -p claudius-review-harness --locked --lib
```

Choose one reproduction suite by adding `--test review_streaming`, `--test review_client`, `--test review_types`, `--test review_error_debug`, `--test review_metrics`, `--test review_agent`, or `--test review_logging` to the complete-harness command. The slow-consumer reproduction takes 31 seconds. The harness is intentionally unpublished and has its own Cargo.lock; it does not add telemetry dependencies to Claudius.

The four tests in src/lib.rs use Sentry's in-memory test transport and an OTLP/HTTP receiver on localhost. No hosted account or live model request is needed. They verify standard error-chain capture and explicitly instrumented export, not automatic instrumentation or production collector integration.

Original logs are retained in [logs](logs/). `vendored-nextest.log` records verification after moving the source and harness here. Original paths and cargo-test commands in historical logs refer to the original review session; use nextest for new test runs.

For an independent follow-up, write `CLAUDE-REVIEW.md` in this directory and leave REVIEW.md as the original report. New reproduction tests belong here, not inside vendor/claudius.
