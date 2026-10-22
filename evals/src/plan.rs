//! Which runs to do, in what order, and when the budget stops them.

use std::fmt;

use serde::{Deserialize, Serialize};
use topcoat_si::KnownModel;

/// One micro-cent is a millionth of a cent.
pub const MICRO_CENTS_PER_CENT: u64 = 1_000_000;

/// How Claude reaches the tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    /// `run_tools` with web search, page fetch and Jev as separate tools.
    ToolLoop,
    /// `run_tools` with the one `python` tool.
    CodeMode,
}

impl Arm {
    /// The name used in records and tables.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolLoop => "tool_loop",
            Self::CodeMode => "code_mode",
        }
    }
}

impl fmt::Display for Arm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One run: a case, an arm, a model and a repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedRun {
    /// Index into the selected cases.
    pub case: usize,
    pub arm: Arm,
    pub model: KnownModel,
    pub repeat: u32,
}

/// The two arms in the order they run for `repeat`: tool loop first on even repeats, code mode
/// first on odd ones.
pub fn arm_order(repeat: u32) -> [Arm; 2] {
    if repeat.is_multiple_of(2) {
        [Arm::ToolLoop, Arm::CodeMode]
    } else {
        [Arm::CodeMode, Arm::ToolLoop]
    }
}

/// Every run in execution order: for each case, each repeat, each model, both arms back to back.
pub fn plan(cases: usize, models: &[KnownModel], repeats: u32) -> Vec<PlannedRun> {
    let mut runs = Vec::new();
    for case in 0..cases {
        for repeat in 0..repeats {
            for &model in models {
                for arm in arm_order(repeat) {
                    runs.push(PlannedRun {
                        case,
                        arm,
                        model,
                        repeat,
                    });
                }
            }
        }
    }
    runs
}

/// Whether the next run must not start: the spend so far has reached the budget.
pub fn budget_reached(spent_micro_cents: u64, budget_cents: u64) -> bool {
    spent_micro_cents >= budget_cents.saturating_mul(MICRO_CENTS_PER_CENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arms_alternate_between_repeats() {
        assert_eq!(arm_order(0), [Arm::ToolLoop, Arm::CodeMode]);
        assert_eq!(arm_order(1), [Arm::CodeMode, Arm::ToolLoop]);
        assert_eq!(arm_order(2), [Arm::ToolLoop, Arm::CodeMode]);
    }

    #[test]
    fn plan_runs_arms_back_to_back_per_case_and_repeat() {
        let models = [KnownModel::ClaudeSonnet55, KnownModel::ClaudeHaiku55];
        let runs = plan(2, &models, 2);
        assert_eq!(runs.len(), 2 * 2 * 2 * 2);
        let order: Vec<(usize, u32, KnownModel, Arm)> = runs
            .iter()
            .take(8)
            .map(|run| (run.case, run.repeat, run.model, run.arm))
            .collect();
        assert_eq!(
            order,
            vec![
                (0, 0, KnownModel::ClaudeSonnet55, Arm::ToolLoop),
                (0, 0, KnownModel::ClaudeSonnet55, Arm::CodeMode),
                (0, 0, KnownModel::ClaudeHaiku55, Arm::ToolLoop),
                (0, 0, KnownModel::ClaudeHaiku55, Arm::CodeMode),
                (0, 1, KnownModel::ClaudeSonnet55, Arm::CodeMode),
                (0, 1, KnownModel::ClaudeSonnet55, Arm::ToolLoop),
                (0, 1, KnownModel::ClaudeHaiku55, Arm::CodeMode),
                (0, 1, KnownModel::ClaudeHaiku55, Arm::ToolLoop),
            ]
        );
        assert_eq!(runs[8].case, 1);
    }

    #[test]
    fn budget_stops_once_spend_reaches_it() {
        assert!(!budget_reached(0, 1));
        assert!(!budget_reached(999_999, 1));
        assert!(budget_reached(1_000_000, 1));
        assert!(budget_reached(1_000_001, 1));
        assert!(budget_reached(0, 0));
        assert!(!budget_reached(299_999_999, 300));
    }
}
