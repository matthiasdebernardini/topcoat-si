# Claudius

A typed Rust client for the Anthropic Messages API: requests and responses for the current
Claude models, streaming, tools (including server-side web search), prompt caching, Message
Batches, files, cost accounting, and monitoring.

```toml
[dependencies]
claudius = { version = "0.33.0", default-features = false, features = ["rustls-tls"] }
```

The default feature is `native-tls`; pick `rustls-tls` instead to avoid OpenSSL.

## Client

`Anthropic::new(None)` reads `CLAUDIUS_API_KEY`, then `ANTHROPIC_API_KEY`. A value of the form
`file:///path` is read from that file. The base URL comes from `CLAUDIUS_BASE_URL` or
`ANTHROPIC_BASE_URL` when set; pass it without the `/v1/` suffix.

```rust,no_run
use std::time::Duration;

use claudius::{Anthropic, KnownModel, MessageCreateParams};

#[tokio::main]
async fn main() -> claudius::Result<()> {
    let client = Anthropic::new(None)?
        .with_max_retries(3)
        .with_timeout(Duration::from_secs(120))?;

    let params = MessageCreateParams::simple("Name three Roman emperors.", KnownModel::ClaudeHaiku55)
        .with_system("Answer in one line.");
    let message = client.send(params).await?;

    for block in &message.content {
        if let Some(text) = block.as_text() {
            println!("{}", text.text);
        }
    }
    println!("{:?}, {} output tokens", message.stop_reason, message.usage.output_tokens);
    Ok(())
}
```

## Models

`KnownModel` covers the current models: `ClaudeOpus55`, `ClaudeSonnet55`, `ClaudeHaiku55` and
`ClaudeFable51`. Any other model ID parses to `Model::Custom`:

```rust
use claudius::{KnownModel, Model};

let model: Model = "claude-opus-5-5".parse().unwrap_or_else(|()| Model::Custom("claude-opus-5-5".into()));
assert_eq!(model, Model::Known(KnownModel::ClaudeOpus55));
```

Current models (Opus 5.5, Sonnet 5.5, Haiku 5.5, Fable 5.1) reject `thinking: {"type":"enabled"}`. Opus 5.5, Sonnet 5.5 and Fable 5.1 also reject `"disabled"`; Haiku 5.5 rejects it at effort `xhigh` or `max`. `temperature` only accepts 1.0, `top_p` only values >= 0.99, and `top_k` is rejected at any value.

## Streaming

`client.stream` returns raw server-sent events. Wrap them in `AccumulatingStream` to print
deltas as they arrive and still get the assembled `Message` at the end:

```rust,no_run
use std::io::Write;

use claudius::{
    AccumulatingStream, Anthropic, ContentBlockDelta, Error, KnownModel, Message,
    MessageCreateParams, MessageParam, MessageStreamEvent, Model,
};
use futures::StreamExt;

async fn stream_reply(client: &Anthropic, prompt: &str) -> claudius::Result<Message> {
    let params = MessageCreateParams::new_streaming(
        4096,
        vec![MessageParam::user(prompt)],
        Model::Known(KnownModel::ClaudeSonnet55),
    );
    let events = client.stream(&params).await?;
    let (mut events, done) = AccumulatingStream::new(events);
    while let Some(event) = events.next().await {
        if let MessageStreamEvent::ContentBlockDelta(delta) = event? {
            if let ContentBlockDelta::TextDelta(text) = delta.delta {
                print!("{}", text.text);
                std::io::stdout().flush()?;
            }
        }
    }
    done.await
        .map_err(|_| Error::streaming("stream ended without a final message", None))?
}
```

Check `message.stop_reason == Some(StopReason::MaxTokens)` to detect a reply cut off at `max_tokens`.

## Tools

Custom tools are `ToolUnionParam::CustomTool(ToolParam)` with a JSON schema; the model's calls come
back as `ContentBlock::ToolUse` and you answer with `ContentBlock::ToolResult`. Set
`ToolParam::with_strict(true)` to have the input validated against the schema. See
`examples/streaming-tool-use.rs` for the full loop.

Web search runs on Anthropic's side, so one request returns the searches, their results and the
cited answer:

```rust
use claudius::{KnownModel, MessageCreateParams, ToolUnionParam, WebSearchTool};

let search = WebSearchTool::new().with_max_uses(5);
let params = MessageCreateParams::simple_streaming("What changed in Rust 1.90?", KnownModel::ClaudeSonnet55)
    .with_tools(vec![ToolUnionParam::WebSearch20260318(search)]);
```

`WebSearchTool20250305` and `ToolUnionParam::WebSearch20250305` remain for callers on the older
tool version. Web fetch is `ToolUnionParam::WebFetch20260318(WebFetchTool::new())`, with
`with_response_inclusion(ResponseInclusion::Full)` to control what comes back. Code execution is
`ToolUnionParam::CodeExecution20260521(CodeExecutionTool::new())`. `examples/server_tools.rs`
uses all three in one request.

## Structured outputs

`OutputFormat::json_schema` constrains the reply to a JSON schema. Set it through `OutputConfig`;
no beta header is needed.

```rust
use claudius::{KnownModel, MessageCreateParams, OutputConfig, OutputFormat};
use serde_json::json;

let schema = json!({"type": "object", "properties": {"summary": {"type": "string"}},
                    "required": ["summary"], "additionalProperties": false});
let params = MessageCreateParams::simple("Summarise Rust in one line.", KnownModel::ClaudeHaiku55)
    .with_output_config(OutputConfig::new().with_format(OutputFormat::json_schema(schema)));
assert!(params.output_config.is_some());
```

## Prompt caching

Mark a block with `cache_control` to cache everything up to it, or set top-level `cache_control`
to let the API place the breakpoint on the last cacheable block (automatic caching).
`CacheControlEphemeral::new()` lives five minutes; `CacheControlEphemeral::one_hour()` lives an hour:

```rust
use claudius::{CacheControlEphemeral, KnownModel, MessageCreateParams, SystemPrompt, TextBlock};

let long_instructions = "...".repeat(2000);

// Explicit breakpoint on the system prompt, kept for one hour.
let system = SystemPrompt::from_blocks(vec![
    TextBlock::new(long_instructions).with_cache_control(CacheControlEphemeral::one_hour()),
]);
let explicit = MessageCreateParams::simple("Summarise the rules.", KnownModel::ClaudeHaiku55)
    .with_system(system);

// Automatic caching.
let automatic = MessageCreateParams::simple("Continue.", KnownModel::ClaudeHaiku55)
    .with_cache_control(CacheControlEphemeral::new());
```

`Usage::cache_creation_input_tokens` and `Usage::cache_read_input_tokens` report writes and hits.
`Usage::cache_creation` splits the writes into `ephemeral_5m_input_tokens` and
`ephemeral_1h_input_tokens`, which are priced separately. Reads are billed at the model's
`TokenRates::cache_read`, a small fraction of the input rate. `examples/cache.rs` checks both
against the live API.

To learn why a request missed the cache, send `with_diagnostics(DiagnosticsRequest::first_turn())`
and then `DiagnosticsRequest::after(previous_id)` on later turns. The response's
`diagnostics.cache_miss_reason` names the earliest divergence (`CacheMissReason::is_changed`).

## Message Batches

Batches cost half as much and finish within 24 hours:

```rust,no_run
use claudius::{
    Anthropic, KnownModel, MessageBatchCreateParams, MessageBatchCreateRequest,
    MessageBatchProcessingStatus, MessageCreateParams,
};
use futures::StreamExt;

async fn run_batch(client: &Anthropic) -> claudius::Result<()> {
    let ask = |id: &str, q: &str| {
        MessageBatchCreateRequest::new(id, MessageCreateParams::simple(q, KnownModel::ClaudeHaiku55))
    };
    let requests = vec![ask("a", "2+2?"), ask("b", "3+3?")];
    let batch = client.create_message_batch(MessageBatchCreateParams::new(requests)).await?;

    while client.retrieve_message_batch(&batch.id).await?.processing_status
        != MessageBatchProcessingStatus::Ended
    {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }

    let results = client.stream_message_batch_results(&batch.id).await?;
    futures::pin_mut!(results);
    while let Some(result) = results.next().await {
        let result = result?;
        println!("{}: {:?}", result.custom_id, result.result);
    }
    Ok(())
}
```

`KnownModel::cost_micro_cents` applies the 50% batch rate when a result's `usage.service_tier` is
`batch`. `TokenRates::batch()` is there to price a result by hand.

## Usage and cost

Rates are integer nano-cents per token (`TokenRates`); costs are micro-cents, rounded up
(1 dollar = 100,000,000 micro-cents). `KnownModel::cost_micro_cents` prices one response's
`Usage`, including Haiku 5.5's long-prompt tier.

```rust
use claudius::{KnownModel, Usage};

let usage = Usage::new(10_000, 2_000).with_cache_read_input_tokens(50_000);
let micro_cents = KnownModel::ClaudeSonnet55.cost_micro_cents(&usage);
// 10,000 × $2/MTok + 2,000 × $10/MTok + 50,000 × $0.10/MTok = $0.045
assert_eq!(micro_cents, 4_500_000);
```

`with_service_tier(ServiceTierChoice::Auto)` lets a request use Priority Tier capacity, and
`Usage::service_tier` reports the tier that served it. Fast mode (`with_speed(InferenceSpeed::Fast)`)
exists on Opus 5.5 only and is priced from `KnownModel::fast_token_rates`. `with_inference_geo("us")`
pins inference to the US at 1.1x. `cost_micro_cents` reads all three from `Usage`, so the price
stays right without extra code.

Web search requests are billed at one cent each and included by `cost_micro_cents`.
`cost_micro_cents` prices a single response: sum the costs of responses rather than summing
`Usage` first, because a summed usage can lose the tier, speed and geo modifiers or wrongly
extend one response's modifiers to another's tokens.

## Monitoring

`Anthropic::last_rate_limits()` returns the rate-limit headers of the latest response
that carried any `anthropic-ratelimit-*`, `anthropic-priority-*`, `anthropic-fast-*` or
`retry-after` header as `RateLimits` (`None` before the first); responses without them, such as a
proxy's error page, leave it unchanged. The windows `requests`, `tokens`, `input_tokens`,
`output_tokens`, `priority_input_tokens`, `priority_output_tokens`, `fast_input_tokens` and
`fast_output_tokens` each hold `limit`, `remaining` and `reset`.

Every API call runs in a `claudius.request` tracing span. On `send` and `stream` it records
`request_id`, `http.status_code`, `attempts`, the token counts and `cost_micro_cents`. The crate
also emits plain tracing events; aggregate them with any subscriber (durations in seconds):

| Target | Level | Message | Fields |
|---|---|---|---|
| `claudius::usage` | `INFO` | `usage` | `model`, `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`, `cost_micro_cents` (`None` for unknown models) |
| `claudius::client` | `DEBUG` | `request_completed` | `duration_seconds`, `ok` |
| `claudius::client` | `DEBUG` | `retry` | `backoff_seconds` |
| `claudius::stream` | `TRACE` | `event` | (none) |
| `claudius::stream` | `TRACE` | `bytes` | `bytes` |
| `claudius::stream` | `DEBUG` | `first_byte` | `ttfb_seconds` |
| `claudius::stream` | `DEBUG` | `completed` | `duration_seconds` |
| `claudius::stream` | `DEBUG` | `error` | (none) |

`claudius::usage` fires once per completed `send` and once per stream that reaches `message_stop`.

The Admin API reports organization-wide usage and cost. It needs an Admin key, passed as
`Anthropic::new(Some(admin_key))`:

```rust,no_run
use claudius::{Anthropic, BucketWidth, CostGroupBy, CostReportParams, UsageGroupBy, UsageReportParams};
use time::{Duration, OffsetDateTime};

async fn report(admin_key: String) -> claudius::Result<()> {
    let admin = Anthropic::new(Some(admin_key))?;
    let since = OffsetDateTime::now_utc() - Duration::days(7);
    let usage = UsageReportParams::new(since)
        .with_bucket_width(BucketWidth::OneDay)
        .group_by(UsageGroupBy::Model);
    for bucket in admin.usage_report(usage).await?.data {
        for r in bucket.results {
            println!("{} {:?}: {} output tokens", bucket.starting_at.date(), r.model, r.output_tokens);
        }
    }
    let cost = CostReportParams::new(since).group_by(CostGroupBy::Description);
    for bucket in admin.cost_report(cost).await?.data {
        for r in bucket.results {
            println!("{:?}: {} {} cents", r.description, r.amount, r.currency);
        }
    }
    Ok(())
}
```

`examples/usage_monitoring.rs` prints one response's usage, cost and rate limits, then both reports.

## Files

`upload_file` stores a file once, `DocumentBlock::from_file_id` references it in later requests,
and `delete_file` removes it. See `examples/files.rs`.

```rust,no_run
use claudius::{Anthropic, ContentBlock, DocumentBlock, KnownModel, MessageCreateParams};
use claudius::{MessageParam, MessageParamContent, MessageRole, TextBlock};

async fn summarise(client: &Anthropic, text: &str) -> claudius::Result<()> {
    let file = client.upload_file("notes.txt", "text/plain", text.as_bytes().to_vec(), None).await?;
    let content = MessageParamContent::Array(vec![
        ContentBlock::Document(DocumentBlock::from_file_id(&file.id)),
        ContentBlock::Text(TextBlock::new("Summarise this document in one sentence.")),
    ]);
    let messages = vec![MessageParam::new(content, MessageRole::User)];
    let params = MessageCreateParams::new(512, messages, KnownModel::ClaudeHaiku55.into());
    let answer = client.send(params).await;
    client.delete_file(&file.id).await?;
    println!("{:?}", answer?.stop_reason);
    Ok(())
}
```

## Errors and retries

Every call returns `claudius::Result<T>`. Rate limits, overloads, server errors, timeouts and
connection failures are retried up to `with_max_retries` times (default 3) with exponential
backoff from 0.5 s capped at 8 s, or the server's `retry-after` when longer; a `retry-after`
above 60 s is returned immediately. Errors carry the HTTP status and request ID:

```rust,no_run
use claudius::{Anthropic, Error, MessageCreateParams};

async fn send(client: &Anthropic, params: MessageCreateParams) {
    match client.send(params).await {
        Ok(message) => println!("{}", message.id),
        Err(err) if err.is_rate_limit() => eprintln!("rate limited, retry after {:?}", err.retry_after()),
        Err(err @ Error::Authentication { .. }) => eprintln!("check the API key: {err}"),
        Err(err) => eprintln!("{err}; {:?}", err.metadata()),
    }
}
```

## Examples

Each needs an API key in the environment:

```bash
cargo run --example basic_chat
cargo run --example batch
cargo run --example cache
cargo run --example files
cargo run --example models_example
cargo run --example retry_example
cargo run --example server_tools
cargo run --example streaming
cargo run --example streaming-tool-use
cargo run --example usage_monitoring
```

## License

Apache-2.0.
