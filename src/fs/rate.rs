//! How fast a job is moving bytes, worked out from when its counts were taken.
//! No clock of its own: every sample carries the instant it was taken at.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Bytes per second over a window of the last few seconds of a job.
///
/// The rate is the bytes gained between the oldest and the newest sample in
/// the window, divided by the time between them. A burst — a clone landing a
/// whole file at once — is spread over the window rather than read off the
/// one gap it arrived in.
#[derive(Debug, Default)]
pub struct Rate {
    /// `(when, bytes done by then)`, oldest first.
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    /// How far back the window reaches.
    const WINDOW: Duration = Duration::from_secs(5);

    /// The shortest span the samples must cover before a rate is given.
    const SETTLE: Duration = Duration::from_secs(1);

    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `done` bytes were done at `at`. Samples are expected in
    /// the order they were taken. Of those older than the window, the newest
    /// is kept as the window's start, so the span stays a full window wide.
    pub fn record(&mut self, at: Instant, done: u64) {
        self.samples.push_back((at, done));
        while self
            .samples
            .get(1)
            .is_some_and(|&(second, _)| at.duration_since(second) >= Self::WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// Bytes per second across the window, or `None` while the samples span
    /// less than `SETTLE`. A count that went backwards reads as no progress.
    pub fn per_second(&self) -> Option<u64> {
        let (&(first, from), &(last, to)) = (self.samples.front()?, self.samples.back()?);
        let span = last.duration_since(first);
        if span < Self::SETTLE {
            return None;
        }

        Some((to.saturating_sub(from) as f64 / span.as_secs_f64()) as u64)
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    /// Records `(milliseconds since the start, bytes done)` pairs.
    fn sampled(samples: &[(u64, u64)]) -> Rate {
        let start = Instant::now();
        let mut rate = Rate::new();
        for &(ms, done) in samples {
            rate.record(start + Duration::from_millis(ms), done);
        }
        rate
    }

    #[test]
    fn nothing_is_said_before_the_samples_span_a_second() {
        assert_eq!(Rate::new().per_second(), None);
        assert_eq!(sampled(&[(0, 0)]).per_second(), None);
        assert_eq!(sampled(&[(0, 0), (900, 90 * MIB)]).per_second(), None);
    }

    #[test]
    fn a_steady_copy_reads_as_its_own_speed() {
        let samples: Vec<(u64, u64)> = (0..=40).map(|i| (i * 50, i * 5 * MIB)).collect();
        assert_eq!(sampled(&samples).per_second(), Some(100 * MIB));
    }

    #[test]
    fn a_burst_is_spread_over_the_window() {
        // Ten MiB a second for four seconds, then a clone lands 1 GiB at once.
        let mut samples: Vec<(u64, u64)> = (0..=8).map(|i| (i * 500, i * 5 * MIB)).collect();
        samples.push((4050, 40 * MIB + 1024 * MIB));

        let per_second = sampled(&samples).per_second().unwrap();
        // Read off the last gap alone that would be over 20 GiB a second.
        assert!(per_second < 300 * MIB, "{}", per_second / MIB);
    }

    #[test]
    fn samples_older_than_the_window_stop_counting() {
        // Fast for the first ten seconds, then stalled for the next ten.
        let mut samples: Vec<(u64, u64)> = (0..=10).map(|s| (s * 1000, s * 100 * MIB)).collect();
        samples.extend((11..=20).map(|s| (s * 1000, 1000 * MIB)));

        assert_eq!(sampled(&samples).per_second(), Some(0));
    }

    #[test]
    fn a_count_that_went_backwards_reads_as_no_progress() {
        assert_eq!(sampled(&[(0, 10), (2000, 4)]).per_second(), Some(0));
    }
}
