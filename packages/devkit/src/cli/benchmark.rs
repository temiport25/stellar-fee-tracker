/// Runs benchmarks against the fee tracker pipeline.
pub struct Benchmark;

impl Benchmark {
    /// Runs SMA, EMA, and WMA on spike data and prints a comparison table.
    pub fn compare_spike(fees: &[f64], window: usize, alpha: f64) {
        use crate::analysis::rolling_window::RollingWindow;

        let sma = RollingWindow::sma(fees, window);
        let ema = RollingWindow::ema(fees, alpha);
        let wma = RollingWindow::wma(fees, window);

        println!("{:<6} {:>12} {:>12} {:>12}", "idx", "SMA", "EMA", "WMA");
        let len = sma.len().min(ema.len()).min(wma.len());
        let offset = window - 1;
        for i in 0..len {
            println!(
                "{:<6} {:>12.4} {:>12.4} {:>12.4}",
                i + offset,
                sma[i],
                ema[i + offset],
                wma[i]
            );
        }
    }

    /// Run all analysis benchmarks and print a summary table.
    pub fn run_all(fees: &[f64], window: usize, alpha: f64) {
        println!("=== Benchmark Results ===");
        println!("Input: {} data points, window={}, alpha={}", fees.len(), window, alpha);
        println!();
        Self::compare_spike(fees, window, alpha);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_does_not_panic() {
        let fees: Vec<f64> = (1..=10).map(|x| x as f64 * 100.0).collect();
        Benchmark::run_all(&fees, 3, 0.3);
    }
}