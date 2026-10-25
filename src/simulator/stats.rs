//! Statistik latensi sederhana untuk uji beban.

use std::fmt;
use std::time::Duration;

#[derive(Debug, Default, Clone)]
pub struct LatencyStats {
    samples: Vec<Duration>,
    errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsReport {
    pub requests: usize,
    pub errors: usize,
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
    pub max: Duration,
}

impl LatencyStats {
    pub fn record(&mut self, latency: Duration) {
        self.samples.push(latency);
    }

    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    pub fn merge(&mut self, other: LatencyStats) {
        self.samples.extend(other.samples);
        self.errors += other.errors;
    }

    pub fn report(&self) -> StatsReport {
        let mut sorted = self.samples.clone();
        sorted.sort();
        let at = |fraction: f64| -> Duration {
            if sorted.is_empty() {
                return Duration::ZERO;
            }
            // Nearest-rank: persentil ke-p adalah elemen ke ceil(p * n).
            let rank = (fraction * sorted.len() as f64).ceil() as usize;
            sorted[rank.clamp(1, sorted.len()) - 1]
        };
        StatsReport {
            requests: sorted.len(),
            errors: self.errors,
            p50: at(0.50),
            p95: at(0.95),
            p99: at(0.99),
            max: sorted.last().copied().unwrap_or_default(),
        }
    }
}

impl fmt::Display for StatsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} request, {} galat, p50 {:?}, p95 {:?}, p99 {:?}, maks {:?}",
            self.requests, self.errors, self.p50, self.p95, self.p99, self.max
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn persentil_nearest_rank() {
        let mut stats = LatencyStats::default();
        for n in 1..=100 {
            stats.record(ms(n));
        }

        let report = stats.report();

        assert_eq!(report.requests, 100);
        assert_eq!(
            (report.p50, report.p95, report.p99, report.max),
            (ms(50), ms(95), ms(99), ms(100))
        );
    }

    #[test]
    fn kosong_satu_sampel_dan_penggabungan() {
        assert_eq!(LatencyStats::default().report().max, Duration::ZERO);

        let mut one = LatencyStats::default();
        one.record(ms(7));
        let report = one.report();
        assert_eq!((report.p50, report.p99), (ms(7), ms(7)));

        let mut a = LatencyStats::default();
        a.record(ms(1));
        a.record_error();
        let mut b = LatencyStats::default();
        b.record(ms(3));
        b.record_error();
        a.merge(b);
        let merged = a.report();
        assert_eq!((merged.requests, merged.errors, merged.max), (2, 2, ms(3)));
    }
}
