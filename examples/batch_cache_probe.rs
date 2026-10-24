//! Measures how long one-request Message Batches queue and how often they hit the prompt cache.
//!
//! Sends rounds of one-request Claude Fable 5.1 batches, one after another, as one conversation:
//! each round adds a short user/assistant exchange to the last, the way research turns do. The
//! system prompt is a synthetic prefix of about 15k tokens with a 1-hour `cache_control`, and the
//! request carries a 1-hour `cache_control` too. Each run starts the prefix with its own start
//! time, so a second run never reads the first run's cache.
//!
//! For each round it prints the queue time (batch creation to `ended_at`), the cache read and
//! write tokens, `usage.service_tier`, the result and the cost from `cost_micro_cents`, then
//! totals. It costs under $1 for 6 rounds.
//!
//! ```text
//! ANTHROPIC_API_KEY=... cargo run --example batch_cache_probe -- --rounds 6 --gap-mins 10
//! ```

use std::process::ExitCode;
use std::time::Duration;

use time::OffsetDateTime;
use carpa::{
    Anthropic, BatchOutcome, BatchWait, CacheControlEphemeral, Control, KnownModel,
    MessageCreateParams, MessageParam, Model, SystemPrompt, TextBlock, Usage,
};

const USAGE: &str = "\
Usage: batch_cache_probe [--rounds N] [--gap-mins N]

Sends N one-request Claude Fable 5.1 batches as one growing conversation over a ~15k-token
cached system prefix, and prints each round's queue time, cache tokens, service tier, result
and cost. Spends real money (under $1 for 6 rounds). Needs ANTHROPIC_API_KEY.

Options:
  --rounds N     rounds to send (default 6)
  --gap-mins N   minutes to wait between rounds (default 0)
  -h, --help     print this help";

const MODEL: KnownModel = KnownModel::ClaudeFable51;
const MAX_TOKENS: u32 = 50;
const POLL: Duration = Duration::from_secs(10);
/// Lines of synthetic prefix; each is about 25 tokens, so about 15k tokens in all.
const PREFIX_LINES: usize = 600;
/// 1 dollar is 100,000,000 micro-cents.
const MICRO_CENTS_PER_DOLLAR: f64 = 100_000_000.0;

struct Args {
    rounds: usize,
    gap: Duration,
}

/// Parses the command line; `Err` holds the text to print and whether it is an error.
fn parse_args() -> Result<Args, (String, bool)> {
    let mut args = Args {
        rounds: 6,
        gap: Duration::ZERO,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        let mut number = |name: &str| -> Result<u64, (String, bool)> {
            argv.next()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| (format!("{name} needs a whole number\n\n{USAGE}"), true))
        };
        match arg.as_str() {
            "-h" | "--help" => return Err((USAGE.to_string(), false)),
            "--rounds" => {
                args.rounds = usize::try_from(number("--rounds")?)
                    .map_err(|_| ("--rounds is too large".to_string(), true))?;
            }
            "--gap-mins" => args.gap = Duration::from_secs(number("--gap-mins")? * 60),
            other => return Err((format!("unknown argument {other}\n\n{USAGE}"), true)),
        }
    }
    if args.rounds == 0 {
        return Err((format!("--rounds must be at least 1\n\n{USAGE}"), true));
    }
    Ok(args)
}

/// The synthetic system prefix, starting with this run's start time.
fn system_prompt(run_started: OffsetDateTime) -> SystemPrompt {
    let mut lines = vec![format!(
        "Cache probe run started {run_started}. Answer every question in one short sentence."
    )];
    for i in 0..PREFIX_LINES {
        lines.push(format!(
            "Rule {i}: record {} of section {} stays filed under region {} with review cycle {} \
             and owner team {}.",
            i * 7 + 3,
            i % 40,
            i % 13,
            i % 9 + 1,
            i % 17
        ));
    }
    let block =
        TextBlock::new(lines.join("\n")).with_cache_control(CacheControlEphemeral::one_hour());
    SystemPrompt::from(vec![block])
}

/// One round's measurements.
struct Round {
    queue: Option<Duration>,
    usage: Option<Usage>,
    result: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err((text, is_error)) => {
            if is_error {
                eprintln!("{text}");
                return ExitCode::FAILURE;
            }
            println!("{text}");
            return ExitCode::SUCCESS;
        }
    };
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &Args) -> carpa::Result<()> {
    let client = Anthropic::try_new(None)?;
    let run_started = OffsetDateTime::now_utc();
    let system = system_prompt(run_started);
    let mut messages: Vec<MessageParam> = Vec::new();
    let mut rounds = Vec::new();

    for round in 1..=args.rounds {
        if round > 1 && !args.gap.is_zero() {
            println!("waiting {} min", args.gap.as_secs() / 60);
            tokio::time::sleep(args.gap).await;
        }
        messages.push(MessageParam::user(format!(
            "Round {round}: name one colour and one animal."
        )));
        let params = MessageCreateParams::new(MAX_TOKENS, messages.clone(), Model::Known(MODEL))
            .with_system(system.clone())
            .with_cache_control(CacheControlEphemeral::one_hour());

        let custom_id = format!("probe-r{round}");
        let batch = client.submit_one(&custom_id, params).await?;
        println!("round {round}: batch {}", batch.id);
        let wait = BatchWait {
            poll: POLL,
            deadline: None,
            on_poll: &mut |_| Control::Continue,
        };
        let done = client.await_one(&batch.id, &custom_id, wait).await?;
        let queue = done
            .batch
            .ended_at
            .and_then(|ended_at| Duration::try_from(ended_at - done.batch.created_at).ok());

        let (usage, result) = match done.outcome {
            BatchOutcome::Succeeded(message) => {
                let usage = message.usage.clone();
                messages.push(MessageParam::from(message));
                (Some(usage), "succeeded".to_string())
            }
            other => {
                messages.push(MessageParam::assistant("(no reply)"));
                let result = match other {
                    BatchOutcome::Errored(error) => format!("errored: {error}"),
                    BatchOutcome::Canceled => "canceled".to_string(),
                    BatchOutcome::Expired => "expired".to_string(),
                    other => format!("{other:?}"),
                };
                (None, result)
            }
        };
        rounds.push(Round {
            queue,
            usage,
            result,
        });
        print_table(&rounds);
    }
    print_totals(&rounds);
    Ok(())
}

fn tokens(count: Option<i32>) -> u64 {
    u64::try_from(count.unwrap_or(0)).unwrap_or(0)
}

fn minutes_seconds(duration: Option<Duration>) -> String {
    duration.map_or_else(
        || "-".to_string(),
        |duration| format!("{}:{:02}", duration.as_secs() / 60, duration.as_secs() % 60),
    )
}

fn print_table(rounds: &[Round]) {
    println!();
    println!(
        "{:>5} {:>7} {:>7} {:>9} {:>9} {:>6} {:>9} {:>10}  result",
        "round", "queue", "input", "cache_rd", "cache_wr", "hit%", "tier", "cost $"
    );
    for (i, round) in rounds.iter().enumerate() {
        let queue = minutes_seconds(round.queue);
        let Some(usage) = &round.usage else {
            println!(
                "{:>5} {queue:>7} {:>7} {:>9} {:>9} {:>6} {:>9} {:>10}  {}",
                i + 1,
                "-",
                "-",
                "-",
                "-",
                "-",
                "-",
                round.result
            );
            continue;
        };
        let input = tokens(Some(usage.input_tokens));
        let read = tokens(usage.cache_read_input_tokens);
        let write = tokens(usage.cache_creation_input_tokens);
        let prompt = input + read + write;
        let hit = if prompt == 0 {
            0.0
        } else {
            read as f64 * 100.0 / prompt as f64
        };
        let tier = usage
            .service_tier
            .map_or_else(|| "none".to_string(), |tier| tier.to_string());
        let cost = MODEL.cost_micro_cents(usage) as f64 / MICRO_CENTS_PER_DOLLAR;
        println!(
            "{:>5} {queue:>7} {input:>7} {read:>9} {write:>9} {hit:>6.1} {tier:>9} {cost:>10.4}  {}",
            i + 1,
            round.result
        );
    }
}

fn print_totals(rounds: &[Round]) {
    let usages: Vec<&Usage> = rounds
        .iter()
        .filter_map(|round| round.usage.as_ref())
        .collect();
    let read: u64 = usages
        .iter()
        .map(|usage| tokens(usage.cache_read_input_tokens))
        .sum();
    let prompt: u64 = usages
        .iter()
        .map(|usage| {
            tokens(Some(usage.input_tokens))
                + tokens(usage.cache_read_input_tokens)
                + tokens(usage.cache_creation_input_tokens)
        })
        .sum();
    let cost: u64 = usages
        .iter()
        .map(|usage| MODEL.cost_micro_cents(usage))
        .sum();
    let queues: Vec<Duration> = rounds.iter().filter_map(|round| round.queue).collect();
    let total_queue: Duration = queues.iter().sum();
    let mean_queue = u32::try_from(queues.len())
        .ok()
        .filter(|count| *count > 0)
        .map(|count| total_queue / count);
    let max_queue = queues.iter().max().copied();
    let batch_tier = usages
        .iter()
        .filter(|usage| usage.service_tier == Some(carpa::ServiceTier::Batch))
        .count();

    println!();
    println!(
        "totals over {} rounds ({} succeeded):",
        rounds.len(),
        usages.len()
    );
    println!(
        "  queue time: mean {}, max {}",
        minutes_seconds(mean_queue),
        minutes_seconds(max_queue)
    );
    let hit = if prompt == 0 {
        0.0
    } else {
        read as f64 * 100.0 / prompt as f64
    };
    println!("  cache hit: {hit:.1}% of {prompt} prompt tokens read from cache");
    println!(
        "  service_tier = batch on {batch_tier} of {} results",
        usages.len()
    );
    println!("  cost: ${:.4}", cost as f64 / MICRO_CENTS_PER_DOLLAR);
    if batch_tier < usages.len() {
        println!(
            "  some results lack service_tier = batch: the cost column is at full price for \
             those, and the real batch bill is half"
        );
    }
}
