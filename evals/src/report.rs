//! The summary table, the baseline comparison and the spot-check file.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::cases::{Case, Kind};
use crate::plan::Arm;
use crate::run::RunRecord;
use crate::stats::{self, Paired};

const MICRO_CENTS_PER_DOLLAR: f64 = 100_000_000.0;
const SPOTCHECK_RUNS: usize = 10;

/// The machine-readable summary, also the baseline format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub selection: Selection,
    pub groups: Vec<GroupSummary>,
    pub paired: Vec<PairedSummary>,
}

/// What a run was asked to do, so a baseline is only compared with a like run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// `dev`, `holdout` or `all`.
    pub split: String,
    /// The selected case ids, in file order.
    pub cases: Vec<String>,
    pub models: Vec<String>,
    pub repeat: u32,
    /// Whether the run is partial: the budget stopped it, or `summarize` found runs missing
    /// or duplicated.
    pub cut_short: bool,
}

/// One model and arm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSummary {
    pub model: String,
    pub arm: Arm,
    pub runs: usize,
    pub passes: usize,
    pub pass_rate: f64,
    pub wilson_low: f64,
    pub wilson_high: f64,
    pub mean_model_calls: f64,
    pub median_model_calls: f64,
    /// The median over cases of each case's mean run cost.
    pub median_cost_per_case_micro_cents: f64,
    /// Total cost divided by passes; `None` with no passes.
    pub cost_per_pass_micro_cents: Option<f64>,
    pub p50_seconds: f64,
    pub p90_seconds: f64,
    pub empty_final_answer_rate: f64,
    pub error_rate: f64,
}

/// Code mode against the tool loop for one model, over all cases or one kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedSummary {
    pub model: String,
    /// `None` for all cases.
    pub kind: Option<Kind>,
    pub cases: usize,
    /// Cases where code mode's pass rate is higher, lower or equal.
    pub outcome: Paired,
    /// The median over cases of code mode's mean cost divided by the tool loop's.
    pub median_cost_ratio: Option<f64>,
}

/// Summarizes `records`, keeping models in the order `selection` gives.
pub fn summarize(records: &[RunRecord], selection: Selection) -> Summary {
    let mut groups = Vec::new();
    let mut paired = Vec::new();
    for model in &selection.models {
        for arm in [Arm::ToolLoop, Arm::CodeMode] {
            let runs: Vec<&RunRecord> = records
                .iter()
                .filter(|record| &record.model == model && record.arm == arm)
                .collect();
            if !runs.is_empty() {
                groups.push(group(model, arm, &runs));
            }
        }

        let per_case = per_case(records, model);
        let mut kinds: Vec<Option<Kind>> = vec![None];
        for &(kind, _, _) in per_case.values() {
            if !kinds.contains(&Some(kind)) {
                kinds.push(Some(kind));
            }
        }
        kinds[1..].sort();
        for kind in kinds {
            let pairs: Vec<&(Kind, CaseArm, CaseArm)> = per_case
                .values()
                .filter(|(case_kind, _, _)| kind.is_none_or(|kind| kind == *case_kind))
                .collect();
            if pairs.is_empty() {
                continue;
            }
            let rates: Vec<(f64, f64)> = pairs
                .iter()
                .map(|(_, tool_loop, code_mode)| (code_mode.pass_rate, tool_loop.pass_rate))
                .collect();
            let ratios: Vec<f64> = pairs
                .iter()
                .filter(|(_, tool_loop, _)| tool_loop.mean_cost > 0.0)
                .map(|(_, tool_loop, code_mode)| code_mode.mean_cost / tool_loop.mean_cost)
                .collect();
            paired.push(PairedSummary {
                model: model.clone(),
                kind,
                cases: pairs.len(),
                outcome: stats::paired(&rates),
                median_cost_ratio: stats::median(&ratios),
            });
        }
    }
    Summary {
        selection,
        groups,
        paired,
    }
}

fn group(model: &str, arm: Arm, runs: &[&RunRecord]) -> GroupSummary {
    let count = runs.len();
    let rate = |predicate: fn(&RunRecord) -> bool| {
        runs.iter().filter(|record| predicate(record)).count() as f64 / count as f64
    };
    let passes = runs.iter().filter(|record| record.pass).count();
    let (wilson_low, wilson_high) = stats::wilson_95(passes, count);
    let calls: Vec<f64> = runs
        .iter()
        .map(|record| record.model_calls as f64)
        .collect();
    let seconds: Vec<f64> = runs.iter().map(|record| record.wall_seconds).collect();
    let total_cost: f64 = runs
        .iter()
        .map(|record| record.cost_micro_cents as f64)
        .sum();
    let mut case_costs: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for record in runs {
        case_costs
            .entry(record.case_id.as_str())
            .or_default()
            .push(record.cost_micro_cents as f64);
    }
    let case_means: Vec<f64> = case_costs
        .values()
        .filter_map(|costs| stats::mean(costs))
        .collect();
    GroupSummary {
        model: model.to_string(),
        arm,
        runs: count,
        passes,
        pass_rate: passes as f64 / count as f64,
        wilson_low,
        wilson_high,
        mean_model_calls: stats::mean(&calls).unwrap_or(0.0),
        median_model_calls: stats::median(&calls).unwrap_or(0.0),
        median_cost_per_case_micro_cents: stats::median(&case_means).unwrap_or(0.0),
        cost_per_pass_micro_cents: (passes > 0).then(|| total_cost / passes as f64),
        p50_seconds: stats::percentile(&seconds, 0.5).unwrap_or(0.0),
        p90_seconds: stats::percentile(&seconds, 0.9).unwrap_or(0.0),
        empty_final_answer_rate: rate(|record| record.empty_final_answer),
        error_rate: rate(|record| record.error.is_some()),
    }
}

/// One arm's results on one case, over its repeats.
struct CaseArm {
    pass_rate: f64,
    mean_cost: f64,
}

/// For `model`, every case both arms ran, with its kind and each arm's results.
fn per_case(records: &[RunRecord], model: &str) -> BTreeMap<String, (Kind, CaseArm, CaseArm)> {
    let mut runs: BTreeMap<(&str, Arm), Vec<&RunRecord>> = BTreeMap::new();
    let mut kinds: BTreeMap<&str, Kind> = BTreeMap::new();
    for record in records.iter().filter(|record| record.model == model) {
        runs.entry((record.case_id.as_str(), record.arm))
            .or_default()
            .push(record);
        kinds.insert(record.case_id.as_str(), record.kind);
    }
    let arm = |runs: &[&RunRecord]| CaseArm {
        pass_rate: runs.iter().filter(|record| record.pass).count() as f64 / runs.len() as f64,
        mean_cost: runs
            .iter()
            .map(|record| record.cost_micro_cents as f64)
            .sum::<f64>()
            / runs.len() as f64,
    };
    let mut cases = BTreeMap::new();
    for (case_id, kind) in kinds {
        if let (Some(tool_loop), Some(code_mode)) = (
            runs.get(&(case_id, Arm::ToolLoop)),
            runs.get(&(case_id, Arm::CodeMode)),
        ) {
            cases.insert(case_id.to_string(), (kind, arm(tool_loop), arm(code_mode)));
        }
    }
    cases
}

fn dollars(micro_cents: f64) -> String {
    format!("${:.4}", micro_cents / MICRO_CENTS_PER_DOLLAR)
}

fn percent(rate: f64) -> String {
    format!("{:.0}%", rate * 100.0)
}

/// The summary as Markdown, with the change from `baseline` when there is one.
pub fn markdown(summary: &Summary, baseline: Option<&Summary>) -> String {
    let mut out = String::new();
    out.push_str("## Per model and arm\n\n");
    out.push_str(
        "| model | arm | runs | pass rate (95% Wilson) | model calls mean / median | \
         median Anthropic cost per case | Anthropic cost per pass | p50 / p90 time | empty answers | errors |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    for group in &summary.groups {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} ({}-{}) | {:.1} / {:.1} | {} | {} | {:.1}s / {:.1}s | {} | {} |",
            group.model,
            group.arm,
            group.runs,
            percent(group.pass_rate),
            percent(group.wilson_low),
            percent(group.wilson_high),
            group.mean_model_calls,
            group.median_model_calls,
            dollars(group.median_cost_per_case_micro_cents),
            group
                .cost_per_pass_micro_cents
                .map_or_else(|| "-".to_string(), dollars),
            group.p50_seconds,
            group.p90_seconds,
            percent(group.empty_final_answer_rate),
            percent(group.error_rate),
        );
    }

    out.push_str("\n## Code mode against the tool loop, case by case\n\n");
    out.push_str("| model | kind | cases | wins | losses | ties | median Anthropic cost ratio |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for paired in &summary.paired {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} |",
            paired.model,
            paired.kind.map_or("all", Kind::as_str),
            paired.cases,
            paired.outcome.wins,
            paired.outcome.losses,
            paired.outcome.ties,
            paired
                .median_cost_ratio
                .map_or_else(|| "-".to_string(), |ratio| format!("{ratio:.2}")),
        );
    }

    if let Some(baseline) = baseline {
        out.push_str("\n## Change from the baseline\n\n");
        if baseline.selection != summary.selection {
            let _ = writeln!(
                out,
                "**Warning: the baseline ran a different selection, so the change below compares \
                 unlike runs.** Baseline: {}. This run: {}.\n",
                describe_selection(&baseline.selection),
                describe_selection(&summary.selection),
            );
        }
        out.push_str("| model | arm | pass rate | median Anthropic cost per case |\n");
        out.push_str("|---|---|---|---|\n");
        for group in &summary.groups {
            let Some(before) = baseline
                .groups
                .iter()
                .find(|before| before.model == group.model && before.arm == group.arm)
            else {
                let _ = writeln!(
                    out,
                    "| {} | {} | not in baseline | |",
                    group.model, group.arm
                );
                continue;
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} -> {} ({:+.0} points) | {} -> {} |",
                group.model,
                group.arm,
                percent(before.pass_rate),
                percent(group.pass_rate),
                (group.pass_rate - before.pass_rate) * 100.0,
                dollars(before.median_cost_per_case_micro_cents),
                dollars(group.median_cost_per_case_micro_cents),
            );
        }
    }
    out
}

fn describe_selection(selection: &Selection) -> String {
    format!(
        "split {}, cases {}, models {}, repeat {}{}",
        selection.split,
        selection.cases.join(","),
        selection.models.join(","),
        selection.repeat,
        if selection.cut_short { ", partial" } else { "" }
    )
}

/// Up to 10 judged runs spread evenly over all of them: index i * judged / n for i in 0..n,
/// n = min(10, judged).
pub fn spotcheck_indices(judged: usize) -> Vec<usize> {
    let count = judged.min(SPOTCHECK_RUNS);
    (0..count).map(|i| i * judged / count).collect()
}

/// The spot-check file: judged runs with the question, the answer, Jev's verdict and the
/// reference facts.
pub fn spotcheck(records: &[RunRecord], cases: &[Case]) -> String {
    let judged: Vec<&RunRecord> = records.iter().filter(|record| record.judged()).collect();
    let mut out = format!(
        "# Judge spot check\n\n{} of {} judged runs, spread evenly over the run order.\n",
        spotcheck_indices(judged.len()).len(),
        judged.len()
    );
    for index in spotcheck_indices(judged.len()) {
        let record = judged[index];
        let Some(case) = cases.iter().find(|case| case.id == record.case_id) else {
            continue;
        };
        let verdict = match (record.judge_probability, &record.judge_error) {
            (Some(probability), _) => format!(
                "p(yes) = {probability:.2}; run {}",
                if record.pass {
                    "passed"
                } else {
                    "did not pass"
                }
            ),
            (None, Some(error)) => format!("judge failed: {error}"),
            (None, None) => "not judged".to_string(),
        };
        let must_include: Vec<String> = case
            .must_include
            .iter()
            .map(|group| group.join(" | "))
            .collect();
        let _ = write!(
            out,
            "\n## {} / {} / {} / repeat {}\n\n**Question:** {}\n\n**Answer:**\n\n{}\n\n\
             **Jev:** {verdict}\n\n**Judge question:** {}\n\n**Must include:** {}\n\n\
             **Must not include:** {}\n\n**Deterministic pass:** {}\n",
            record.case_id,
            record.model,
            record.arm,
            record.repeat,
            case.prompt,
            record.answer.as_deref().unwrap_or(""),
            case.judge,
            must_include.join("; "),
            case.must_not_include.join("; "),
            record.deterministic_pass,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cases::Split;

    fn record(case_id: &str, arm: Arm, pass: bool, cost_micro_cents: u64) -> RunRecord {
        RunRecord {
            case_id: case_id.to_string(),
            kind: Kind::Fanout,
            split: Split::Dev,
            arm,
            model: "claude-sonnet-5-5".to_string(),
            repeat: 0,
            pass,
            deterministic_pass: pass,
            judge_probability: None,
            judge_error: None,
            answer: None,
            error: None,
            error_kind: None,
            model_calls: 1,
            tool_calls: BTreeMap::new(),
            python_calls: 0,
            empty_final_answer: false,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_micro_cents,
            wall_seconds: 1.0,
        }
    }

    #[test]
    fn spotcheck_spreads_over_every_judged_run() {
        assert_eq!(
            spotcheck_indices(25),
            vec![0, 2, 5, 7, 10, 12, 15, 17, 20, 22]
        );
        assert_eq!(spotcheck_indices(7), vec![0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(spotcheck_indices(0), Vec::<usize>::new());
    }

    #[test]
    fn per_case_pairs_arms_by_case_id() {
        // The arms list the cases in opposite orders, and case c ran only one arm.
        let records = [
            record("a", Arm::ToolLoop, true, 100),
            record("b", Arm::ToolLoop, false, 200),
            record("c", Arm::ToolLoop, true, 300),
            record("b", Arm::CodeMode, true, 50),
            record("a", Arm::CodeMode, false, 400),
        ];
        let cases = per_case(&records, "claude-sonnet-5-5");
        assert_eq!(cases.keys().collect::<Vec<_>>(), ["a", "b"]);
        let (_, tool_loop, code_mode) = &cases["a"];
        assert_eq!((tool_loop.pass_rate, tool_loop.mean_cost), (1.0, 100.0));
        assert_eq!((code_mode.pass_rate, code_mode.mean_cost), (0.0, 400.0));
        let (_, tool_loop, code_mode) = &cases["b"];
        assert_eq!((tool_loop.pass_rate, tool_loop.mean_cost), (0.0, 200.0));
        assert_eq!((code_mode.pass_rate, code_mode.mean_cost), (1.0, 50.0));
    }
}
