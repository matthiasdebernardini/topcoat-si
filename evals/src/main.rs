//! Evals for carpa. `code-mode` compares code mode with the plain tool loop on the same
//! cases and models; `summarize` rebuilds a finished run's summary from its results file. See
//! README.md in this directory.

mod cases;
mod db;
mod grade;
mod plan;
mod report;
mod run;
mod stats;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use time::OffsetDateTime;
use time::macros::format_description;
use carpa::KnownModel;

use crate::cases::{Case, Split};
use crate::db::EvalDb;
use crate::plan::{Arm, MICRO_CENTS_PER_CENT, PlannedRun, budget_reached};
use crate::report::{Selection, Summary};
use crate::run::{RunRecord, Services};

const SUITE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/suites/code_mode.jsonl");
const RESULTS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/results");
const BASELINE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/baselines/code-mode.json");
const DEFAULT_MODELS: &str = "sonnet-5-5";
const DEFAULT_REPEAT: u32 = 3;

const USAGE: &str = "\
Usage: carpa-evals code-mode --budget-cents N [options]
       carpa-evals summarize <results.jsonl> [--save-baseline]
       carpa-evals summarize --db PATH [--run STAMP] [--save-baseline]

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
  --db PATH            Also store every run in the SurrealDB directory PATH, under the
                       timestamp used for the file names. Ignored with --plan.
  --plan               Print the runs in execution order and their count; call nothing.
  --save-baseline      Also write the summary to evals/baselines/code-mode.json.
  -h, --help           Print this help.

Needs ANTHROPIC_API_KEY, FIRECRAWL_API_KEY, TYPESAFE_API_KEY, TYPESAFE_DEFAULT_MODEL (a pinned
Jev version) and MONTY_BIN, except with --plan.

summarize prints the summary of a finished evals/results/<timestamp>-code-mode.jsonl, with the
change from the baseline, and writes no results files. It calls nothing and needs no keys.
Give either a results file or --db PATH. With --db, --run STAMP picks the run to summarize;
without --run, summarize lists the stamps stored in PATH and exits.
  --save-baseline      Also write the summary to evals/baselines/code-mode.json.";

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
    db: Option<PathBuf>,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    Help,
    CodeMode(Options),
    /// Summarize the results file at `path`.
    Summarize {
        path: PathBuf,
        save_baseline: bool,
    },
    /// Summarize the run `run` stored in the database at `db`, or list its runs when `run` is
    /// `None`.
    SummarizeDb {
        db: PathBuf,
        run: Option<String>,
        save_baseline: bool,
    },
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
        return Err("missing command: code-mode or summarize".to_string());
    };
    match command.as_str() {
        "code-mode" => parse_code_mode(rest),
        "summarize" => parse_summarize(rest),
        _ => Err(format!("unknown command {command:?}")),
    }
}

fn parse_summarize(args: &[String]) -> Result<Command, String> {
    let mut path = None;
    let mut db = None;
    let mut run = None;
    let mut save_baseline = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--save-baseline" => save_baseline = true,
            "--db" => db = Some(PathBuf::from(value()?)),
            "--run" => run = Some(value()?),
            other if other.starts_with("--") => return Err(format!("unknown option {other:?}")),
            other if path.is_none() => path = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    if let Some(db) = db {
        if path.is_some() {
            return Err("give either a results file or --db, not both".to_string());
        }
        return Ok(Command::SummarizeDb {
            db,
            run,
            save_baseline,
        });
    }
    if run.is_some() {
        return Err("--run needs --db".to_string());
    }
    let path = path.ok_or("summarize needs a results file")?;
    Ok(Command::Summarize {
        path,
        save_baseline,
    })
}

fn parse_code_mode(rest: &[String]) -> Result<Command, String> {
    let mut budget_cents = None;
    let mut options = Options {
        budget_cents: 0,
        models: parse_models(DEFAULT_MODELS)?,
        repeat: DEFAULT_REPEAT,
        split: SplitFilter::Only(Split::Dev),
        cases: None,
        plan: false,
        save_baseline: false,
        db: None,
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
            "--db" => options.db = Some(PathBuf::from(value()?)),
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

/// Reads and validates the suite file.
fn load_suite() -> Result<Vec<Case>, String> {
    fs::read_to_string(SUITE)
        .map_err(|error| format!("{SUITE}: {error}"))
        .and_then(|text| cases::parse_cases(&text).map_err(|error| format!("{SUITE}: {error}")))
}

/// The selection `code-mode` records for `options` and the `cases` they select.
fn selection(options: &Options, cases: &[Case], cut_short: bool) -> Selection {
    Selection {
        split: options.split.as_str().to_string(),
        cases: cases.iter().map(|case| case.id.clone()).collect(),
        models: options.models.iter().map(ToString::to_string).collect(),
        repeat: options.repeat,
        cut_short,
    }
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
    let eval_db = match &options.db {
        Some(path) => Some(EvalDb::open(path).await?),
        None => None,
    };
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
        if let Some(eval_db) = &eval_db {
            eval_db.insert(&stamp, &record).await?;
        }
        records.push(record);
    }

    let summary = report::summarize(&records, selection(options, cases, cut_short));
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
        save_baseline(&summary)?;
    }
    Ok(())
}

/// Writes `summary` to the baseline file, refusing a partial run.
fn save_baseline(summary: &Summary) -> Result<(), String> {
    if summary.selection.cut_short {
        return Err(
            "not saving the baseline: the run is partial (cut short by the budget, or runs are \
             missing or duplicated)"
                .to_string(),
        );
    }
    let baseline_path = PathBuf::from(BASELINE);
    if let Some(parent) = baseline_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let json =
        serde_json::to_string_pretty(summary).map_err(|error| format!("summary: {error}"))?;
    fs::write(&baseline_path, json).map_err(|error| format!("{}: {error}", baseline_path.display()))
}

/// Parses a results file, one `RunRecord` per non-blank line. Errors name the line.
fn parse_records(text: &str) -> Result<Vec<RunRecord>, String> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record =
            serde_json::from_str(line).map_err(|error| format!("line {}: {error}", index + 1))?;
        records.push(record);
    }
    Ok(records)
}

/// The selection a results file ran, as `code-mode` would have recorded it: the case ids in
/// `suite` order, the models in first-seen order and the highest repeat plus one. Errors name
/// every case id not in `suite`.
///
/// The split is `dev` or `holdout` when every record is in that split, else `all`. A file of
/// only dev (or only holdout) cases cannot tell `--split all` with such `--cases` from the
/// narrower split, so this picks the narrower one.
///
/// This sets `cut_short` to false; `incomplete_runs` finds partial files.
fn infer_selection(records: &[RunRecord], suite: &[Case]) -> Result<Selection, String> {
    if records.is_empty() {
        return Err("no runs in the results file".to_string());
    }
    let split = if records.iter().all(|record| record.split == Split::Dev) {
        SplitFilter::Only(Split::Dev)
    } else if records.iter().all(|record| record.split == Split::Holdout) {
        SplitFilter::Only(Split::Holdout)
    } else {
        SplitFilter::All
    };
    let mut unknown: Vec<&str> = Vec::new();
    for record in records {
        let id = record.case_id.as_str();
        if !suite.iter().any(|case| case.id == id) && !unknown.contains(&id) {
            unknown.push(id);
        }
    }
    if !unknown.is_empty() {
        return Err(format!("case ids not in the suite: {}", unknown.join(", ")));
    }
    let cases: Vec<String> = suite
        .iter()
        .filter(|case| records.iter().any(|record| record.case_id == case.id))
        .map(|case| case.id.clone())
        .collect();
    let mut models: Vec<String> = Vec::new();
    for record in records {
        if !models.contains(&record.model) {
            models.push(record.model.clone());
        }
    }
    let repeat = records
        .iter()
        .map(|record| record.repeat.saturating_add(1))
        .max()
        .unwrap_or(0);
    Ok(Selection {
        split: split.as_str().to_string(),
        cases,
        models,
        repeat,
        cut_short: false,
    })
}

/// Every run of `selection` (each case, model, arm and repeat) that `records` lacks or has more
/// than once, in plan order, as `missing ...` or `duplicated (N times) ...`.
fn incomplete_runs(records: &[RunRecord], selection: &Selection) -> Vec<String> {
    let mut counts: HashMap<(&str, &str, Arm, u32), usize> = HashMap::new();
    for record in records {
        *counts
            .entry((&record.case_id, &record.model, record.arm, record.repeat))
            .or_default() += 1;
    }
    let mut problems = Vec::new();
    for case in &selection.cases {
        for repeat in 0..selection.repeat {
            for model in &selection.models {
                for arm in plan::arm_order(repeat) {
                    let run = format!("{case} {model} {arm} repeat {repeat}");
                    match counts.get(&(case.as_str(), model.as_str(), arm, repeat)) {
                        None => problems.push(format!("missing {run}")),
                        Some(1) => {}
                        Some(count) => problems.push(format!("duplicated ({count} times) {run}")),
                    }
                }
            }
        }
    }
    problems
}

/// Prints the summary of the results file at `path`, and saves it as the baseline if asked.
/// Writes no results files and calls nothing.
fn summarize_file(path: &Path, save: bool) -> Result<(), String> {
    let records = fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| parse_records(&text))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    summarize_records(&records, save)
}

/// Prints the run stored under `run` in the database at `path`, or lists the stored runs when
/// `run` is `None`.
async fn summarize_db(path: &Path, run: Option<&str>, save: bool) -> Result<(), String> {
    let eval_db = EvalDb::open(path).await?;
    let Some(run) = run else {
        for stamp in eval_db.runs().await? {
            println!("{stamp}");
        }
        return Ok(());
    };
    let records = eval_db.records(run).await?;
    if records.is_empty() {
        return Err(format!("{}: no run {run}", path.display()));
    }
    summarize_records(&records, save)
}

/// Prints the summary of `records`, and saves it as the baseline if asked.
fn summarize_records(records: &[RunRecord], save: bool) -> Result<(), String> {
    let mut selection = infer_selection(records, &load_suite()?)?;
    let problems = incomplete_runs(records, &selection);
    if problems.is_empty() {
        eprintln!("Complete: all {} runs present once.", records.len());
    } else {
        selection.cut_short = true;
        eprintln!(
            "Partial: {} runs missing or duplicated, for example: {}",
            problems.len(),
            problems
                .iter()
                .take(5)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    let summary = report::summarize(records, selection);
    let baseline = load_baseline(Path::new(BASELINE));
    println!("{}", report::markdown(&summary, baseline.as_ref()));
    if save {
        save_baseline(&summary)?;
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
        Ok(Command::Summarize {
            path,
            save_baseline,
        }) => {
            return match summarize_file(&path, save_baseline) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("error: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(Command::SummarizeDb {
            db,
            run,
            save_baseline,
        }) => {
            return match summarize_db(&db, run.as_deref(), save_baseline).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("error: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cases = match load_suite().and_then(|all| select(all, &options)) {
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
    use std::collections::BTreeMap;

    use super::*;
    use crate::cases::Kind;

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

    #[test]
    fn summarize_needs_a_path() {
        assert_eq!(
            parse_args(&args(&["summarize"])),
            Err("summarize needs a results file".to_string())
        );
        assert_eq!(
            parse_args(&args(&["summarize", "--save-baseline"])),
            Err("summarize needs a results file".to_string())
        );
        assert_eq!(
            parse_args(&args(&["summarize", "a.jsonl", "b.jsonl"])),
            Err("unexpected argument \"b.jsonl\"".to_string())
        );
        assert_eq!(
            parse_args(&args(&["summarize", "a.jsonl"])),
            Ok(Command::Summarize {
                path: PathBuf::from("a.jsonl"),
                save_baseline: false,
            })
        );
        assert_eq!(
            parse_args(&args(&["summarize", "--save-baseline", "a.jsonl"])),
            Ok(Command::Summarize {
                path: PathBuf::from("a.jsonl"),
                save_baseline: true,
            })
        );
    }

    fn record(case_id: &str, split: Split, model: &str, repeat: u32) -> RunRecord {
        RunRecord {
            case_id: case_id.to_string(),
            kind: Kind::Fanout,
            split,
            arm: Arm::ToolLoop,
            model: model.to_string(),
            repeat,
            pass: true,
            deterministic_pass: true,
            judge_probability: Some(0.95),
            judge_error: None,
            answer: Some("2015, 2018".to_string()),
            error: None,
            error_kind: None,
            model_calls: 2,
            tool_calls: BTreeMap::from([("web_search".to_string(), 3)]),
            python_calls: 0,
            empty_final_answer: false,
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 40,
            cost_micro_cents: 1234,
            wall_seconds: 1.5,
        }
    }

    #[test]
    fn records_round_trip_through_jsonl() {
        let mut failed = record("b", Split::Holdout, "claude-opus-5-5", 1);
        failed.arm = Arm::CodeMode;
        failed.pass = false;
        failed.judge_probability = None;
        failed.judge_error = Some("timeout".to_string());
        failed.answer = None;
        failed.error = Some("connection reset".to_string());
        failed.error_kind = Some("connection".to_string());
        let records = vec![record("a", Split::Dev, "claude-sonnet-5-5", 0), failed];
        let mut text = String::new();
        for record in &records {
            text.push_str(&serde_json::to_string(record).expect("record serializes"));
            text.push('\n');
        }
        assert_eq!(parse_records(&text), Ok(records));
        assert!(parse_records("{}\n").is_err_and(|error| error.starts_with("line 1: ")));
    }

    fn case(id: &str, split: Split) -> Case {
        Case {
            id: id.to_string(),
            split,
            kind: Kind::Fanout,
            prompt: "Which years?".to_string(),
            must_include: vec![vec!["2015".to_string()]],
            must_not_include: Vec::new(),
            judge: "Does the answer name 2015?".to_string(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn selection_comes_from_the_records() {
        let suite = [case("a", Split::Dev), case("b", Split::Dev)];
        let records = [
            record("b", Split::Dev, "claude-sonnet-5-5", 0),
            record("a", Split::Dev, "claude-opus-5-5", 2),
            record("b", Split::Dev, "claude-opus-5-5", 1),
            record("a", Split::Dev, "claude-sonnet-5-5", 0),
        ];
        assert_eq!(
            infer_selection(&records, &suite),
            Ok(Selection {
                split: "dev".to_string(),
                cases: vec!["a".to_string(), "b".to_string()],
                models: vec![
                    "claude-sonnet-5-5".to_string(),
                    "claude-opus-5-5".to_string()
                ],
                repeat: 3,
                cut_short: false,
            })
        );
        let holdout = [record("a", Split::Holdout, "claude-sonnet-5-5", 0)];
        assert_eq!(
            infer_selection(&holdout, &suite).map(|selection| selection.split),
            Ok("holdout".to_string())
        );
        let mixed = [
            record("a", Split::Dev, "claude-sonnet-5-5", 0),
            record("b", Split::Holdout, "claude-sonnet-5-5", 0),
        ];
        assert_eq!(
            infer_selection(&mixed, &suite).map(|selection| selection.split),
            Ok("all".to_string())
        );
        assert!(infer_selection(&[], &suite).is_err());
    }

    /// The records a finished `code-mode` run of `options` would write.
    fn records_for(options: &Options, cases: &[Case]) -> Vec<RunRecord> {
        plan::plan(cases.len(), &options.models, options.repeat)
            .iter()
            .map(|run| {
                let case = &cases[run.case];
                let mut record = record(&case.id, case.split, &run.model.to_string(), run.repeat);
                record.arm = run.arm;
                record
            })
            .collect()
    }

    #[test]
    fn inferred_selection_matches_code_mode_in_suite_order() {
        let suite = vec![
            case("zeta", Split::Dev),
            case("alpha", Split::Holdout),
            case("mid", Split::Dev),
            case("beta", Split::Dev),
        ];
        let Ok(Command::CodeMode(options)) = parse_args(&args(&[
            "code-mode",
            "--budget-cents",
            "1",
            "--models",
            "opus-5-5,sonnet-5-5",
            "--repeat",
            "2",
            "--cases",
            "beta,zeta,mid",
        ])) else {
            panic!("expected code-mode options");
        };
        let cases = select(suite.clone(), &options).expect("cases are in the suite");
        let records = records_for(&options, &cases);
        assert_eq!(
            infer_selection(&records, &suite),
            Ok(selection(&options, &cases, false))
        );
        assert_eq!(
            infer_selection(&records, &suite).map(|selection| selection.cases),
            Ok(vec![
                "zeta".to_string(),
                "mid".to_string(),
                "beta".to_string()
            ])
        );

        let Ok(Command::CodeMode(all)) = parse_args(&args(&[
            "code-mode",
            "--budget-cents",
            "1",
            "--split",
            "all",
        ])) else {
            panic!("expected code-mode options");
        };
        let cases = select(suite.clone(), &all).expect("cases are in the suite");
        let records = records_for(&all, &cases);
        assert_eq!(
            infer_selection(&records, &suite),
            Ok(selection(&all, &cases, false))
        );

        let stray = [record("gamma", Split::Dev, "claude-sonnet-5-5", 0)];
        assert_eq!(
            infer_selection(&stray, &suite),
            Err("case ids not in the suite: gamma".to_string())
        );
    }

    fn complete_run() -> (Vec<RunRecord>, Selection) {
        let suite = vec![case("b", Split::Dev), case("a", Split::Dev)];
        let Ok(Command::CodeMode(options)) = parse_args(&args(&[
            "code-mode",
            "--budget-cents",
            "1",
            "--repeat",
            "2",
        ])) else {
            panic!("expected code-mode options");
        };
        let cases = select(suite.clone(), &options).expect("cases are in the suite");
        let records = records_for(&options, &cases);
        let selection = infer_selection(&records, &suite).expect("records are in the suite");
        (records, selection)
    }

    #[test]
    fn a_complete_run_has_no_incomplete_runs() {
        let (records, selection) = complete_run();
        assert_eq!(records.len(), 8);
        assert_eq!(incomplete_runs(&records, &selection), Vec::<String>::new());
    }

    #[test]
    fn missing_runs_are_named() {
        let (mut records, selection) = complete_run();
        let dropped = records.remove(3);
        assert_eq!((dropped.case_id.as_str(), dropped.repeat), ("b", 1));
        assert_eq!(
            incomplete_runs(&records, &selection),
            vec![format!(
                "missing b claude-sonnet-5-5 {} repeat 1",
                dropped.arm
            )]
        );
    }

    #[test]
    fn duplicated_runs_are_named() {
        let (mut records, selection) = complete_run();
        records.push(records[0].clone());
        records.push(records[0].clone());
        assert_eq!(
            incomplete_runs(&records, &selection),
            vec!["duplicated (3 times) b claude-sonnet-5-5 tool_loop repeat 0".to_string()]
        );
    }
}
