//! A token bucket for one mod's log lines: a burst of lines passes at once, then the bucket refills
//! at a steady rate. Time is passed in, so the bucket never reads the clock itself.

use std::time::Instant;

/// One token in units. Fractions of a token are kept, so slow refills do not round away.
const UNIT: u128 = 1_000_000;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// What the bucket said about one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The line passes.
    Allowed,
    /// The line passes; this many lines were dropped since the last one that did.
    AllowedAfter(u32),
    /// The line is dropped.
    Dropped,
}

/// A token bucket: it holds up to `burst` tokens and gains `per_second` tokens each second. A line
/// costs one token. It starts full.
#[derive(Debug)]
pub struct RateLimit {
    burst: u32,
    per_second: u32,
    /// The tokens in the bucket, in `UNIT`s.
    units: u128,
    /// The time the bucket last gained tokens; `None` until the first line.
    last: Option<Instant>,
    /// Lines dropped since the last one that passed.
    dropped: u32,
}

impl RateLimit {
    pub fn new(burst: u32, per_second: u32) -> RateLimit {
        RateLimit {
            burst,
            per_second,
            units: u128::from(burst) * UNIT,
            last: None,
            dropped: 0,
        }
    }

    /// Decides one line at `now`. Calls must come in time order; an earlier `now` gains nothing.
    pub fn allow(&mut self, now: Instant) -> Verdict {
        let cap = u128::from(self.burst) * UNIT;
        if let Some(last) = self.last {
            let nanos = now.saturating_duration_since(last).as_nanos();
            let gained = nanos * u128::from(self.per_second) * UNIT / NANOS_PER_SECOND;
            if gained > 0 {
                self.units = (self.units + gained).min(cap);
                self.last = Some(now);
            }
        } else {
            self.last = Some(now);
        }
        if self.units >= UNIT {
            self.units -= UNIT;
            match std::mem::take(&mut self.dropped) {
                0 => Verdict::Allowed,
                n => Verdict::AllowedAfter(n),
            }
        } else {
            self.dropped = self.dropped.saturating_add(1);
            Verdict::Dropped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_burst_passes_then_drops() {
        let t0 = Instant::now();
        let mut bucket = RateLimit::new(3, 1);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0), Verdict::Dropped);
        assert_eq!(bucket.allow(t0), Verdict::Dropped);
    }

    #[test]
    fn tokens_refill_with_time() {
        let t0 = Instant::now();
        let mut bucket = RateLimit::new(2, 1);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0), Verdict::Dropped);
        // Half a second gives half a token: still not enough for a line.
        assert_eq!(
            bucket.allow(t0 + Duration::from_millis(500)),
            Verdict::Dropped
        );
        // A full second in all gives one whole token.
        assert_eq!(
            bucket.allow(t0 + Duration::from_secs(1)),
            Verdict::AllowedAfter(2)
        );
        assert_eq!(bucket.allow(t0 + Duration::from_secs(1)), Verdict::Dropped);
    }

    #[test]
    fn dropped_lines_are_counted_once() {
        let t0 = Instant::now();
        let mut bucket = RateLimit::new(1, 1);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0), Verdict::Dropped);
        assert_eq!(bucket.allow(t0), Verdict::Dropped);
        assert_eq!(bucket.allow(t0), Verdict::Dropped);
        let later = t0 + Duration::from_secs(1);
        assert_eq!(bucket.allow(later), Verdict::AllowedAfter(3));
        // The count was handed over: the next line that passes reports nothing.
        assert_eq!(
            bucket.allow(later + Duration::from_secs(1)),
            Verdict::Allowed
        );
    }

    #[test]
    fn a_long_gap_never_exceeds_the_burst() {
        let t0 = Instant::now();
        let mut bucket = RateLimit::new(2, 5);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        let much_later = t0 + Duration::from_secs(3600);
        assert_eq!(bucket.allow(much_later), Verdict::Allowed);
        assert_eq!(bucket.allow(much_later), Verdict::Allowed);
        assert_eq!(bucket.allow(much_later), Verdict::Dropped);
    }

    #[test]
    fn zero_per_second_never_refills() {
        let t0 = Instant::now();
        let mut bucket = RateLimit::new(2, 0);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0), Verdict::Allowed);
        assert_eq!(bucket.allow(t0 + Duration::from_secs(60)), Verdict::Dropped);
    }
}
