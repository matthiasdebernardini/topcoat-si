//! Evals for topcoat-si. `code-mode` compares code mode with the plain tool loop on the same
//! cases and models. See README.md in this directory.

mod cases;
mod grade;
mod plan;
mod report;
mod run;
mod stats;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use time::OffsetDateTime;
use time::macros::format_description;
use topcoat_si::KnownModel;

use crate::cases::{Case, Split};
use crate::plan::{MICRO_CENTS_PER_CENT, PlannedRun, budget_reached};
use crate::report::{Selection, Summary};
use crate::run::{RunRecord, Services};

const SUITE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/suites/code_mode.jsonl");
const RESULTS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/results");
const BASELINE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/baselines/code-mode.json");
const DEFAULT_MODELS: &str = "sonnet-5-5";
const DEFAULT_REPEAT: u32 = 3;

const USAGE: &str = "\
Usage: topcoat-evals code-mode --budget-cents N [options]

Runs every selected case with the tool loop and with code mode, grades each answer, and writes
evals/results/<UTC timestamp>-code-mode.{jsonl,md} and <timestamp>-spotcheck.md.
This calls paid APIs (Anthropic, Firecrawl, Jev) unless --plan is given.

Options:
  --budget-cents N     Required. Stop before a run once Anthropic spend reaches N cents.
                       Jev judge calls and Firecrawl credits are not counted.
  --models A,B         Models, with or without the claude- prefix. Default: sonnet-5-5.
                       Known: opus-5-5, sonnet-5-5, haiku-5-5, fable-5-1.
  --repeat N           Runs per case, arm and model. Default: 3.
  --split S            dev, holdout or all. Default: dev.
  --cases ID,ID        Only these case ids. Default: every case in the split.
  --plan               Print the runs in execution order and their count; call nothing.
  --save-baseline      Also write the summary to evals/baselines/code-mode.json.
  -h, --help           Print this help.

Needs ANTHROPIC_API_KEY, FIRECRAWL_API_KEY, TYPESAFE_API_KEY, TYPESAFE_DEFAULT_MODEL (a pinned
Jev version) and MONTY_BIN, except with --plan.";

/// Which splits to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitFilter {
    Only(Split),
    All,
}

impl SplitFilter {
    /// The name used on the command line.
    fn as_str(self) -> &'static str {
        match self {
            Self::Only(split) => split.as_str(),
            Self::All => "all",
        }
    }

    fn contains(self, split: Split) -> bool {
        match self {
            Self::Only(only) => only == split,
            Self::All => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Options {
    budget_cents: u64,
    models: Vec<KnownModel>,
    repeat: u32,
    split: SplitFilter,
    cases: Option<Vec<String>>,
    plan: bool,
    save_baseline: bool,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    Help,
    CodeMode(Options),
}

fn parse_models(list: &str) -> Result<Vec<KnownModel>, String> {
    let mut models = Vec::new();
    for name in list.split(',').map(str::trim) {
        let model = KnownModel::from_str(name)
            .or_else(|_| KnownModel::from_str(&format!("claude-{name}")))
            .map_err(|_| format!("unknown model {name:?}"))?;
        if !models.contains(&model) {
            models.push(model);
        }
    }
    Ok(models)
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Command::Help);
    }
    let Some((command, rest)) = args.split_first() else {
        return Err("missing command: code-mode".to_string());
    };
    if command != "code-mode" {
        return Err(format!("unknown command {command:?}"));
    }
    let mut budget_cents = None;
    let mut options = Options {
        budget_cents: 0,
        models: parse_models(DEFAULT_MODELS)?,
        repeat: DEFAULT_REPEAT,
        split: SplitFilter::Only(Split::Dev),
        cases: None,
        plan: false,
        save_baseline: false,
    };
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let mut value = || {
            rest.next()
                .map(String::as_str)
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--budget-cents" => {
                let value = value()?;
                budget_cents = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("--budget-cents: {value:?} is not a whole number"))?,
                );
            }
            "--models" => options.models = parse_models(value()?)?,
            "--repeat" => {
                let value = value()?;
                options.repeat = value
                    .parse::<u32>()
                    .ok()
                    .filter(|repeat| *repeat > 0)
                    .ok_or_else(|| format!("--repeat: {value:?} is not a positive number"))?;
            }
            "--split" => {
                options.split = match value()? {
                    "dev" => SplitFilter::Only(Split::Dev),
                    "holdout" => SplitFilter::Only(Split::Holdout),
                    "all" => SplitFilter::All,
                    other => return Err(format!("--split: {other:?} is not dev, holdout or all")),
                };
            }
            "--cases" => {
                options.cases = Some(
                    value()?
                        .split(',')
                        .map(|id| id.trim().to_string())
                        .collect(),
                );
            }
            "--plan" => options.plan = true,
            "--save-baseline" => options.save_baseline = true,
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    options.budget_cents = budget_cents.ok_or("--budget-cents is required")?;
    Ok(Command::CodeMode(options))
}

/// The cases the options select, in file order. Errors name every requested id that is unknown
/// or outside the split.
fn select(all: Vec<Case>, options: &Options) -> Result<Vec<Case>, String> {
    if let Some(ids) = &options.cases {
        let mut problems = Vec::new();
        for id in ids {
            match all.iter().find(|case| &case.id == id) {
                None => problems.push(format!("no case with id {id:?}")),
                Some(case) if !options.split.contains(case.split) => problems.push(format!(
                    "case {id:?} is in split {}, excluded by split {}",
                    case.split,
                    options.split.as_str()
                )),
                Some(_) => {}
            }
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
    }
    let selected: Vec<Case> = all
        .into_iter()
        .filter(|case| options.split.contains(case.split))
        .filter(|case| {
            options
                .cases
                .as_ref()
                .is_none_or(|ids| ids.contains(&case.id))
        })
        .collect();
    if selected.is_empty() {
        return Err("no cases match the split and ids given".to_string());
    }
    Ok(selected)
}

fn describe(run: &PlannedRun, cases: &[Case]) -> String {
    let case = &cases[run.case];
    format!(
        "{} ({}, {}) {} {} repeat {}",
        case.id, case.kind, case.split, run.model, run.arm, run.repeat
    )
}

fn load_baseline(path: &Path) -> Option<Summary> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(summary) => Some(summary),
        Err(error) => {
            eprintln!("warning: ignoring {}: {error}", path.display());
            None
        }
    }
}

async fn execute(options: &Options, cases: &[Case], runs: &[PlannedRun]) -> Result<(), String> {
    // Every check that can refuse to start runs before the first paid call.
    let services = Services::from_env().await?;
    let stamp = OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .map_err(|error| format!("timestamp: {error}"))?;
    let results = PathBuf::from(RESULTS_DIR);
    fs::create_dir_all(&results).map_err(|error| format!("{}: {error}", results.display()))?;
    let records_path = results.join(format!("{stamp}-code-mode.jsonl"));
    let file = File::create_new(&records_path)
        .map_err(|error| format!("{}: {error}", records_path.display()))?;
    let mut writer = BufWriter::new(file);

    let mut records: Vec<RunRecord> = Vec::new();
    let mut spent: u64 = 0;
    let mut cut_short = false;
    for (index, run) in runs.iter().enumerate() {
        if budget_reached(spent, options.budget_cents) {
            cut_short = true;
            println!(
                "Budget reached: spent {spent} of {} micro-cents. Stopped before run {} of {}: {}. \
                 Skipped {} runs.",
                options.budget_cents.saturating_mul(MICRO_CENTS_PER_CENT),
                index + 1,
                runs.len(),
                describe(run, cases),
                runs.len() - index
            );
            break;
        }
        let case = &cases[run.case];
        let record = run::run_case(&services, case, run.arm, run.model, run.repeat).await;
        spent = spent.saturating_add(record.cost_micro_cents);
        println!(
            "[{}/{}] {}: {} ({} model calls, {} micro-cents, {:.1}s){}",
            index + 1,
            runs.len(),
            describe(run, cases),
            if record.pass { "pass" } else { "fail" },
            record.model_calls,
            record.cost_micro_cents,
            record.wall_seconds,
            record
                .error
                .as_ref()
                .map_or_else(String::new, |error| format!(": {error}")),
        );
        let line = serde_json::to_string(&record).map_err(|error| format!("record: {error}"))?;
        writeln!(writer, "{line}")
            .and_then(|()| writer.flush())
            .map_err(|error| format!("{}: {error}", records_path.display()))?;
        records.push(record);
    }

    let selection = Selection {
        split: options.split.as_str().to_string(),
        cases: cases.iter().map(|case| case.id.clone()).collect(),
        models: options.models.iter().map(ToString::to_string).collect(),
        repeat: options.repeat,
        cut_short,
    };
    let summary = report::summarize(&records, selection);
    let baseline = load_baseline(Path::new(BASELINE));
    let table = report::markdown(&summary, baseline.as_ref());
    println!("\n{table}");
    println!(
        "Anthropic spend: {spent} micro-cents of a {}-cent budget. Jev judge calls are not counted.",
        options.budget_cents
    );
    let write_new = |path: PathBuf, contents: &str| {
        File::create_new(&path)
            .and_then(|mut file| file.write_all(contents.as_bytes()))
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    write_new(results.join(format!("{stamp}-code-mode.md")), &table)?;
    write_new(
        results.join(format!("{stamp}-spotcheck.md")),
        &report::spotcheck(&records, cases),
    )?;
    println!("Results in {}", results.display());
    if options.save_baseline {
        if cut_short {
            return Err(
                "not saving the baseline: the budget cut this run short, so it is partial"
                    .to_string(),
            );
        }
        let baseline_path = PathBuf::from(BASELINE);
        if let Some(parent) = baseline_path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let json =
            serde_json::to_string_pretty(&summary).map_err(|error| format!("summary: {error}"))?;
        fs::write(&baseline_path, json)
            .map_err(|error| format!("{}: {error}", baseline_path.display()))?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&args) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::CodeMode(options)) => options,
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cases = match fs::read_to_string(SUITE)
        .map_err(|error| format!("{SUITE}: {error}"))
        .and_then(|text| cases::parse_cases(&text).map_err(|error| format!("{SUITE}: {error}")))
        .and_then(|all| select(all, &options))
    {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runs = plan::plan(cases.len(), &options.models, options.repeat);

    if options.plan {
        for (index, run) in runs.iter().enumerate() {
            println!("{:>4}. {}", index + 1, describe(run, &cases));
        }
        println!(
            "{} runs: {} cases x 2 arms x {} models x {} repeats",
            runs.len(),
            cases.len(),
            options.models.len(),
            options.repeat
        );
        return ExitCode::SUCCESS;
    }

    match execute(&options, &cases, &runs).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn budget_cents_is_required() {
        assert_eq!(
            parse_args(&args(&["code-mode", "--plan"])),
            Err("--budget-cents is required".to_string())
        );
        assert_eq!(
            parse_args(&args(&["code-mode", "--budget-cents"])),
            Err("--budget-cents needs a value".to_string())
        );
        assert_eq!(
            parse_args(&args(&["code-mode", "--budget-cents", "ten"])),
            Err("--budget-cents: \"ten\" is not a whole number".to_string())
        );
        let Ok(Command::CodeMode(options)) =
            parse_args(&args(&["code-mode", "--budget-cents", "300"]))
        else {
            panic!("expected code-mode options");
        };
        assert_eq!(options.budget_cents, 300);
    }
}
