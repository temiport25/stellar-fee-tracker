/// Summary statistics for a fee distribution.
#[derive(Debug, Clone)]
pub struct FeeDistributionSummary {
    pub min: u64,
    pub max: u64,
    pub mean: f64,
    pub median: u64,
    pub std_dev: f64,
    /// Percentiles p10, p20, ..., p90, p99 (index 0 = p10, index 9 = p99).
    pub percentiles: [u64; 10],
}

/// Computes percentile statistics over fee samples.
pub struct Percentile;

impl Percentile {
    /// Returns the nearest-rank percentile of a sorted slice.
    /// `p` must be in 1..=100. Returns 0 for empty slices.
    pub fn nearest_rank(sorted: &[u64], p: usize) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        let idx = ((p as f64 / 100.0) * sorted.len() as f64).ceil() as usize;
        sorted[idx.saturating_sub(1).min(sorted.len() - 1)]
    }

    /// Returns the linear-interpolation percentile of a sorted slice.
    /// `p` must be in 0..=100. Returns 0 for empty slices.
    pub fn linear_interpolation(sorted: &[u64], p: usize) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        if sorted.len() == 1 {
            return sorted[0];
        }
        let rank = (p as f64 / 100.0) * (sorted.len() - 1) as f64;
        let lo = rank.floor() as usize;
        let hi = rank.ceil() as usize;
        let frac = rank - lo as f64;
        (sorted[lo] as f64 + frac * (sorted[hi] as f64 - sorted[lo] as f64)).floor() as u64
    }

    /// Returns a full fee distribution summary for a sorted slice.
    /// Returns `None` for empty slices.
    pub fn fee_distribution_summary(sorted: &[u64]) -> Option<FeeDistributionSummary> {
        if sorted.is_empty() {
            return None;
        }
        let n = sorted.len();
        let min = sorted[0];
        let max = sorted[n - 1];
        let mean = sorted.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let median = Self::nearest_rank(sorted, 50);
        let variance =
            sorted.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / n as f64;
        let std_dev = variance.sqrt();
        let ps = [10, 20, 30, 40, 50, 60, 70, 80, 90, 99];
        let percentiles = ps.map(|p| Self::nearest_rank(sorted, p));
        Some(FeeDistributionSummary { min, max, mean, median, std_dev, percentiles })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_basic() {
        let data = [10, 20, 30, 40, 50];
        assert_eq!(Percentile::nearest_rank(&data, 50), 30);
        assert_eq!(Percentile::nearest_rank(&data, 100), 50);
        assert_eq!(Percentile::nearest_rank(&data, 1), 10);
    }

    #[test]
    fn nearest_rank_empty() {
        assert_eq!(Percentile::nearest_rank(&[], 50), 0);
    }

    #[test]
    fn linear_interpolation_basic() {
        let data = [10, 20, 30, 40, 50];
        assert_eq!(Percentile::linear_interpolation(&data, 0), 10);
        assert_eq!(Percentile::linear_interpolation(&data, 100), 50);
        assert_eq!(Percentile::linear_interpolation(&data, 50), 30);
    }

    #[test]
    fn linear_interpolation_empty() {
        assert_eq!(Percentile::linear_interpolation(&[], 50), 0);
    }

    #[test]
    fn fee_distribution_summary_basic() {
        let data = [10u64, 20, 30, 40, 50];
        let s = Percentile::fee_distribution_summary(&data).unwrap();
        assert_eq!(s.min, 10);
        assert_eq!(s.max, 50);
        assert_eq!(s.mean, 30.0);
        assert_eq!(s.median, 30);
        assert_eq!(s.percentiles[9], 50); // p99
    }

    #[test]
    fn fee_distribution_summary_empty() {
        assert!(Percentile::fee_distribution_summary(&[]).is_none());
    }
}
