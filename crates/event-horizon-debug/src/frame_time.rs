use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Five seconds of frame durations for the F3 overlay's 1% and 0.1% lows.
#[derive(Clone, Debug, Default)]
pub struct FrameTimeWindow {
    frames: VecDeque<(Instant, f32)>,
}

impl FrameTimeWindow {
    /// The window the lows are computed over.
    pub const WINDOW: Duration = Duration::from_secs(5);

    pub fn record(&mut self, now: Instant, millis: f32) {
        if millis > 0.0 {
            self.frames.push_back((now, millis));
        }
        self.prune(now);
    }
    fn prune(&mut self, now: Instant) {
        while self
            .frames
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > Self::WINDOW)
        {
            self.frames.pop_front();
        }
    }
    /// The 1% and 0.1% worst frame durations of the window, as FPS: the
    /// slowest 1% and 0.1% of frames, inverted.
    pub fn lows(&self, now: Instant) -> Option<(f32, f32)> {
        let mut millis: Vec<f32> = self
            .frames
            .iter()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= Self::WINDOW)
            .map(|(_, millis)| *millis)
            .collect();
        let n = millis.len();
        if n == 0 {
            return None;
        }
        millis.sort_by(f32::total_cmp);
        let worst = |fraction: f64| {
            let rank = ((n as f64 * fraction).ceil() as usize).clamp(1, n);
            millis[n - rank]
        };
        Some((1000.0 / worst(0.01), 1000.0 / worst(0.001)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_lows_are_the_worst_percent_of_the_last_five_seconds() {
        let t0 = Instant::now();
        let mut window = FrameTimeWindow::default();
        // 200 frames of 10 ms (100 FPS), the slowest 100 ms (10 FPS) and the
        // second slowest 50 ms (20 FPS): the 1% low is the second worst, the
        // 0.1% low the very worst.
        for _ in 0..198 {
            window.record(t0, 10.0);
        }
        window.record(t0, 50.0);
        window.record(t0, 100.0);
        let (one, tenth) = window.lows(t0).unwrap();
        assert!((one - 20.0).abs() < 1e-4, "1% low: {one}");
        assert!((tenth - 10.0).abs() < 1e-4, "0.1% low: {tenth}");

        // Frames older than the window are dropped.
        let later = t0 + Duration::from_secs(6);
        assert!(window.lows(later).is_none());
    }
}
