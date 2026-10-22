//! Interval, percentile and paired-comparison helpers.

/// z for a two-sided 95% interval.
const Z_95: f64 = 1.959_963_984_540_054;

/// The 95% Wilson score interval for `passes` out of `runs`, as (low, high). `(0, 1)` for no
/// runs.
pub fn wilson_95(passes: usize, runs: usize) -> (f64, f64) {
    if runs == 0 {
        return (0.0, 1.0);
    }
    let n = runs as f64;
    let p = passes as f64 / n;
    let z2 = Z_95 * Z_95;
    let denominator = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denominator;
    let half = Z_95 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denominator;
    ((center - half).max(0.0), (center + half).min(1.0))
}

/// The nearest-rank percentile `q` (0 to 1) of `values`, or `None` when empty.
pub fn percentile(values: &[f64], q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (q * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// The median of `values`: the mean of the two middle values for an even count.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[middle - 1] + sorted[middle]) / 2.0)
    } else {
        Some(sorted[middle])
    }
}

/// The mean of `values`, or `None` when empty.
pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

/// Wins, losses and ties of the treatment against the control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Paired {
    pub wins: usize,
    pub losses: usize,
    pub ties: usize,
}

/// Counts, over (treatment, control) pass-rate pairs, the cases where the treatment is higher,
/// lower or equal.
pub fn paired(pairs: &[(f64, f64)]) -> Paired {
    let mut result = Paired::default();
    for &(treatment, control) in pairs {
        if treatment > control {
            result.wins += 1;
        } else if treatment < control {
            result.losses += 1;
        } else {
            result.ties += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "{actual} is not {expected}"
        );
    }

    #[test]
    fn wilson_matches_known_values() {
        let (low, high) = wilson_95(8, 10);
        close(low, 0.4902);
        close(high, 0.9433);
        let (low, high) = wilson_95(0, 10);
        close(low, 0.0);
        close(high, 0.2775);
        let (low, high) = wilson_95(10, 10);
        close(low, 0.7225);
        close(high, 1.0);
        assert_eq!(wilson_95(0, 0), (0.0, 1.0));
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let values = [5.0, 1.0, 4.0, 2.0, 3.0, 10.0, 9.0, 8.0, 7.0, 6.0];
        assert_eq!(percentile(&values, 0.5), Some(5.0));
        assert_eq!(percentile(&values, 0.9), Some(9.0));
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(mean(&[1.0, 2.0, 6.0]), Some(3.0));
    }

    #[test]
    fn paired_counts_wins_losses_and_ties() {
        let pairs = [
            (1.0, 0.0),
            (0.5, 0.5),
            (0.0, 1.0 / 3.0),
            (2.0 / 3.0, 1.0 / 3.0),
        ];
        assert_eq!(
            paired(&pairs),
            Paired {
                wins: 2,
                losses: 1,
                ties: 1
            }
        );
    }
}
