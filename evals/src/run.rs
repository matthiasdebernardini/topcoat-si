//! Running one arm on one case, judging it, and recording the result.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use carpa::code_mode::{self, CodeMode, Limits, Pool, packs};
use carpa::jev::{JevClient, JevTool, Outcome, Questions};
use carpa::{
    Anthropic, CacheControlEphemeral, ClientTool, ContentBlock, Error, ErrorKind, FirecrawlClient,
    FirecrawlFetchPage, FirecrawlWebSearch, KnownModel, Message, MessageCreateParams, MessageParam,
    Model, ToolRun, ToolRunError,
};

use crate::cases::{Case, Kind, Split};
use crate::grade::deterministic_pass;
use crate::plan::Arm;

const MAX_TOKENS: u32 = 4096;
const MAX_TURNS: u32 = 15;
// No cap: the tool loop's Firecrawl tools have none, so a cap here would handicap code mode only.
const FIRECRAWL_MAX_CREDITS: u32 = u32::MAX;
const MAX_WORKERS: usize = 2;
/// A run passes the judge only on a yes at this certainty or more.
const JUDGE_MIN_CERTAINTY: f64 = 0.8;
const JUDGE_QUESTION: &str = "judge";

const BASE_SYSTEM: &str = "Answer the user's question using the tools. Do not answer from \
    memory. End with a plain-text answer.";
const TOOL_LOOP_SYSTEM: &str = "You may call several tools in one turn.";
// Copied verbatim from the system prompt in README.md's code-mode example ("One pool per app,
// one `CodeMode` per conversation"), which "Advice for apps" says to follow.
const CODE_MODE_SYSTEM: &str = "Do each step in one snippet: fetch everything you need at once \
    with asyncio.gather, then extract in Python. Print only what you need for the answer, not \
    raw pages. When you have enough, stop calling the tool and answer.";

/// The clients every run shares.
pub struct Services {
    pub anthropic: Anthropic,
    pub firecrawl: FirecrawlClient,
    pub jev: JevClient,
    pub pool: Arc<Pool>,
}

impl Services {
    /// Builds every client from the environment and starts the sandbox pool. Calls no paid API.
    pub async fn from_env() -> Result<Self, String> {
        let judge_model = std::env::var("TYPESAFE_DEFAULT_MODEL").unwrap_or_default();
        check_pinned(&judge_model)?;
        let anthropic = Anthropic::try_new(None).map_err(|error| format!("Anthropic: {error}"))?;
        let firecrawl =
            FirecrawlClient::from_env().map_err(|error| format!("Firecrawl: {error}"))?;
        let jev = JevClient::from_env().map_err(|error| format!("Jev: {error}"))?;
        let monty = std::env::var_os("MONTY_BIN")
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "MONTY_BIN is not set or empty".to_string())?;
        let pool = code_mode::pool(monty, MAX_WORKERS)
            .await
            .map_err(|error| format!("sandbox pool: {error}"))?;
        Ok(Self {
            anthropic,
            firecrawl,
            jev,
            pool,
        })
    }
}

/// Refuses a judge model that is not a pinned version.
pub fn check_pinned(model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("TYPESAFE_DEFAULT_MODEL is not set or empty".to_string());
    }
    let lower = model.to_lowercase();
    if lower.contains("latest") || lower.contains("preview") {
        return Err(format!(
            "TYPESAFE_DEFAULT_MODEL is {model:?}; pin a version such as jev-1.13.0"
        ));
    }
    Ok(())
}

/// One run's line in the results file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub case_id: String,
    pub kind: Kind,
    pub split: Split,
    pub arm: Arm,
    pub model: String,
    pub repeat: u32,
    pub pass: bool,
    pub deterministic_pass: bool,
    /// Jev's probability of yes; `None` when the run had no text or the judge call failed.
    pub judge_probability: Option<f64>,
    pub judge_error: Option<String>,
    pub answer: Option<String>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub model_calls: usize,
    pub tool_calls: BTreeMap<String, usize>,
    pub python_calls: usize,
    pub empty_final_answer: bool,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_micro_cents: u64,
    pub wall_seconds: f64,
}

impl RunRecord {
    /// Whether Jev was asked about this run.
    pub fn judged(&self) -> bool {
        self.judge_probability.is_some() || self.judge_error.is_some()
    }
}

/// What one arm's run produced, before grading.
struct ArmOutcome {
    responses: Vec<Message>,
    answer: Option<String>,
    error: Option<Error>,
    /// When the model's last response arrived, before any cleanup.
    finished: Instant,
}

impl ArmOutcome {
    /// The outcome of a `run_tools` call that ended at `finished`.
    fn from_run(run: Result<ToolRun, Box<ToolRunError>>, finished: Instant) -> Self {
        match run {
            Ok(run) => Self {
                answer: run.text(),
                responses: run.responses,
                error: None,
                finished,
            },
            Err(failed) => Self {
                responses: failed.responses,
                answer: None,
                error: Some(failed.error),
                finished,
            },
        }
    }
}

/// Runs `arm` on `case` with `model`, judges the answer, and returns the record.
pub async fn run_case(
    services: &Services,
    case: &Case,
    arm: Arm,
    model: KnownModel,
    repeat: u32,
) -> RunRecord {
    let started = Instant::now();
    let outcome = match arm {
        Arm::ToolLoop => run_tool_loop(services, case, model).await,
        Arm::CodeMode => run_code_mode(services, case, model).await,
    };
    let wall_seconds = outcome.finished.duration_since(started).as_secs_f64();

    let deterministic = deterministic_pass(case, outcome.answer.as_deref());
    let (judge_probability, judge_yes, judge_error) = match &outcome.answer {
        Some(answer) => match judge(&services.jev, case, answer).await {
            Ok((probability, yes)) => (Some(probability), yes, None),
            Err(error) => (None, false, Some(error.to_string())),
        },
        None => (None, false, None),
    };

    let mut tool_calls: BTreeMap<String, usize> = BTreeMap::new();
    for block in outcome
        .responses
        .iter()
        .flat_map(|response| &response.content)
    {
        if let ContentBlock::ToolUse(tool_use) = block {
            *tool_calls.entry(tool_use.name.clone()).or_default() += 1;
        }
    }
    let usage = outcome.responses.iter().map(|response| &response.usage);
    let sum = |field: fn(&carpa::Usage) -> i32| -> i64 {
        usage.clone().map(|usage| i64::from(field(usage))).sum()
    };

    RunRecord {
        case_id: case.id.clone(),
        kind: case.kind,
        split: case.split,
        arm,
        model: model.to_string(),
        repeat,
        pass: deterministic && judge_yes,
        deterministic_pass: deterministic,
        judge_probability,
        judge_error,
        empty_final_answer: outcome.error.is_none() && outcome.answer.is_none(),
        answer: outcome.answer,
        error: outcome.error.as_ref().map(ToString::to_string),
        error_kind: outcome
            .error
            .as_ref()
            .map(|error| kind_name(error.kind()).to_string()),
        model_calls: outcome.responses.len(),
        python_calls: tool_calls.get("python").copied().unwrap_or(0),
        tool_calls,
        input_tokens: sum(|usage| usage.input_tokens),
        output_tokens: sum(|usage| usage.output_tokens),
        cache_read_tokens: sum(|usage| usage.cache_read_input_tokens.unwrap_or(0)),
        cache_write_tokens: sum(|usage| usage.cache_creation_input_tokens.unwrap_or(0)),
        cost_micro_cents: outcome
            .responses
            .iter()
            .map(|response| model.cost_micro_cents(&response.usage))
            .sum(),
        wall_seconds,
    }
}

fn params(case: &Case, model: KnownModel, arm_system: &str) -> MessageCreateParams {
    MessageCreateParams::new(
        MAX_TOKENS,
        vec![MessageParam::user(case.prompt.as_str())],
        Model::Known(model),
    )
    .with_system(format!("{BASE_SYSTEM}\n\n{arm_system}"))
    .with_cache_control(CacheControlEphemeral::new())
}

async fn run_tool_loop(services: &Services, case: &Case, model: KnownModel) -> ArmOutcome {
    let search = FirecrawlWebSearch::new(services.firecrawl.clone());
    let fetch = FirecrawlFetchPage::new(services.firecrawl.clone());
    let jev = JevTool::new(services.jev.clone());
    let tools: [&dyn ClientTool; 3] = [&search, &fetch, &jev];
    let run = services
        .anthropic
        .run_tools(params(case, model, TOOL_LOOP_SYSTEM), &tools, MAX_TURNS)
        .await;
    ArmOutcome::from_run(run, Instant::now())
}

async fn run_code_mode(services: &Services, case: &Case, model: KnownModel) -> ArmOutcome {
    let search = FirecrawlWebSearch::new(services.firecrawl.clone());
    let fetch = FirecrawlFetchPage::new(services.firecrawl.clone());
    let conversation_packs = vec![
        packs::firecrawl(search, fetch, FIRECRAWL_MAX_CREDITS),
        packs::jev(services.jev.clone()),
        packs::research(),
        packs::triage(),
        packs::reports(),
    ];
    let code = match CodeMode::try_new(services.pool.clone(), conversation_packs, Limits::default())
    {
        Ok(code) => code,
        Err(error) => {
            return ArmOutcome {
                responses: Vec::new(),
                answer: None,
                error: Some(error),
                finished: Instant::now(),
            };
        }
    };
    let tools: [&dyn ClientTool; 1] = [&code];
    let run = services
        .anthropic
        .run_tools(params(case, model, CODE_MODE_SYSTEM), &tools, MAX_TURNS)
        .await;
    // Returning the worker is cleanup, not the arm's work, so the clock stops before it.
    let finished = Instant::now();
    if let Err(error) = code.finish().await {
        eprintln!("warning: could not return the sandbox worker: {error}");
    }
    ArmOutcome::from_run(run, finished)
}

/// Asks Jev the case's judge question about `answer`: the probability of yes, and whether it is
/// yes at the required certainty.
async fn judge(jev: &JevClient, case: &Case, answer: &str) -> carpa::Result<(f64, bool)> {
    let questions = Questions::new().noul(JUDGE_QUESTION, case.judge.as_str());
    let response = jev.ask(answer, &questions).await?;
    let probability = response.noul(JUDGE_QUESTION)?;
    let yes = matches!(
        response.yes_no(JUDGE_QUESTION, JUDGE_MIN_CERTAINTY)?,
        Outcome::Answered(true)
    );
    Ok((probability, yes))
}

/// A short name for the error kind, for grouping in the results.
fn kind_name(kind: &ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Validation { .. } => "validation",
        ErrorKind::RequestTooLarge { .. } => "request_too_large",
        ErrorKind::UnsupportedSchema { .. } => "unsupported_schema",
        ErrorKind::Connection => "connection",
        ErrorKind::Timeout => "timeout",
        ErrorKind::Api { .. } => "api",
        ErrorKind::Interrupted => "interrupted",
        ErrorKind::Decode => "decode",
        ErrorKind::MaxTokens => "max_tokens",
        ErrorKind::Refusal => "refusal",
        ErrorKind::Io { .. } => "io",
        ErrorKind::Sandbox => "sandbox",
        ErrorKind::Tool => "tool",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_model_must_be_pinned() {
        assert_eq!(check_pinned("jev-1.13.0"), Ok(()));
        assert!(check_pinned("jev-latest").is_err());
        assert!(check_pinned("Jev-Preview-2").is_err());
        assert_eq!(
            check_pinned(" "),
            Err("TYPESAFE_DEFAULT_MODEL is not set or empty".to_string())
        );
    }
}
