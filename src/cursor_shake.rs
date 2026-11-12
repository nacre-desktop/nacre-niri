//! Fixed-size pointer shake detection and cursor scale animation.

use std::time::Duration;

const WINDOW_MS: u32 = 250;
const QUIET_MS: u32 = 180;
const MIN_TRAVEL: f64 = 180.;
const MIN_REVERSALS: u8 = 3;
const HISTORY: usize = 16;

#[derive(Clone, Copy, Default)]
struct Sample {
    x: f64,
    y: f64,
    time: u32,
}

/// Tracks a short trail without allocating on motion events.
pub struct CursorShake {
    samples: [Sample; HISTORY],
    len: usize,
    next: usize,
    last_axis_delta: f64,
    reversals: u8,
    enlarged_at: Option<Duration>,
    last_motion: Option<Duration>,
}

impl Default for CursorShake {
    fn default() -> Self {
        Self {
            samples: [Sample::default(); HISTORY],
            len: 0,
            next: 0,
            last_axis_delta: 0.,
            reversals: 0,
            enlarged_at: None,
            last_motion: None,
        }
    }
}

impl CursorShake {
    pub fn motion(&mut self, x: f64, y: f64, time_ms: u32, now: Duration, enabled: bool) {
        self.last_motion = Some(now);
        if !enabled {
            self.reset_trail();
            self.enlarged_at = None;
            return;
        }
        let previous = if self.len == 0 {
            None
        } else {
            Some(self.samples[(self.next + HISTORY - 1) % HISTORY])
        };
        if let Some(previous) = previous {
            let dt = time_ms.wrapping_sub(previous.time);
            if dt > WINDOW_MS {
                self.reset_trail();
            }
            let dx = x - previous.x;
            let dy = y - previous.y;
            let axis_delta = if dx.abs() >= dy.abs() { dx } else { dy };
            if axis_delta.abs() > 2. {
                if self.last_axis_delta.signum() != 0.
                    && axis_delta.signum() != self.last_axis_delta.signum()
                {
                    self.reversals = self.reversals.saturating_add(1);
                }
                self.last_axis_delta = axis_delta;
            }
        }
        self.samples[self.next] = Sample {
            x,
            y,
            time: time_ms,
        };
        self.next = (self.next + 1) % HISTORY;
        self.len = (self.len + 1).min(HISTORY);
        if self.len < 5 || self.reversals < MIN_REVERSALS {
            return;
        }
        let oldest = self.samples[(self.next + HISTORY - self.len) % HISTORY];
        if time_ms.wrapping_sub(oldest.time) > WINDOW_MS {
            return;
        }
        let mut travel = 0.;
        let mut min_x = x;
        let mut max_x = x;
        let mut min_y = y;
        let mut max_y = y;
        let mut prev = oldest;
        for i in 1..self.len {
            let sample = self.samples[(self.next + HISTORY - self.len + i) % HISTORY];
            travel += ((sample.x - prev.x).powi(2) + (sample.y - prev.y).powi(2)).sqrt();
            min_x = min_x.min(sample.x);
            max_x = max_x.max(sample.x);
            min_y = min_y.min(sample.y);
            max_y = max_y.max(sample.y);
            prev = sample;
        }
        let diagonal = ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt();
        // KDE KWin's ShakeDetector compares trail length to its bounding-box diagonal.
        // Requiring 3 reversals and 180 logical px of travel avoids single flicks.
        if travel >= MIN_TRAVEL && travel >= diagonal * 3.5 {
            self.enlarged_at = Some(now);
            self.reset_trail();
        }
    }

    pub fn scale(&self, now: Duration) -> f64 {
        let Some(start) = self.enlarged_at else {
            return 1.;
        };
        let age = now.saturating_sub(start);
        let active = self
            .last_motion
            .is_some_and(|last| now.saturating_sub(last) < Duration::from_millis(QUIET_MS as u64));
        let phase = if active {
            age.as_secs_f64() / 0.12
        } else {
            1. - now
                .saturating_sub(
                    self.last_motion.unwrap_or(start) + Duration::from_millis(QUIET_MS as u64),
                )
                .as_secs_f64()
                / 0.18
        };
        1. + 1.5 * phase.clamp(0., 1.)
    }

    pub fn animating(&self, now: Duration) -> bool {
        self.enlarged_at.is_some_and(|_| self.scale(now) > 1.01)
    }

    fn reset_trail(&mut self) {
        self.len = 0;
        self.reversals = 0;
        self.last_axis_delta = 0.;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sequence(points: &[(f64, f64, u32)]) -> CursorShake {
        let mut tracker = CursorShake::default();
        for &(x, y, t) in points {
            tracker.motion(x, y, t, Duration::from_millis(t as u64), true);
        }
        tracker
    }
    #[test]
    fn detects_rapid_shake_but_not_slow_or_single_reversal() {
        let shake = sequence(&[
            (0., 0., 0),
            (50., 0., 25),
            (0., 0., 50),
            (50., 0., 75),
            (0., 0., 100),
            (50., 0., 125),
        ]);
        assert!(shake.scale(Duration::from_millis(125)) > 1.);
        let slow = sequence(&[
            (0., 0., 0),
            (50., 0., 100),
            (0., 0., 200),
            (50., 0., 300),
            (0., 0., 400),
        ]);
        assert_eq!(slow.scale(Duration::from_millis(400)), 1.);
        let one = sequence(&[(0., 0., 0), (100., 0., 25), (0., 0., 50)]);
        assert_eq!(one.scale(Duration::from_millis(50)), 1.);
    }
}
