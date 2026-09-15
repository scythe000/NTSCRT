//! Wall-clock pacing for the preview's own animations.
//!
//! egui repaints as fast as the display allows, so anything that advances
//! "one step per repaint" runs at the monitor's refresh rate: a 2-second
//! keyframe loop plays in 0.8 s at 60 Hz and 0.33 s at 144 Hz. The macOS
//! app paces its timeline preview at the timeline's own fps and caps
//! Animate at 30 fps (NTSC is a 30 fps format; faster only burns CPU). A
//! [`Pacer`] gives the same behaviour: ask it how many frames are due at
//! this instant, and how long until the next one.

use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct Pacer {
    /// When the next frame is due. None while stopped, so starting plays
    /// its first frame immediately rather than a period late.
    next: Option<Instant>,
}

impl Pacer {
    /// How many frames of a `fps` animation are due at `now`. Usually 0 or
    /// 1; more when a repaint took longer than a frame, so the animation
    /// keeps wall-clock time instead of slowing down. A stall longer than
    /// a second (window hidden, heavy export) is not caught up — the
    /// animation resumes from where it was rather than racing.
    pub fn frames_due(&mut self, now: Instant, fps: f64) -> u32 {
        let period = Duration::from_secs_f64(1.0 / fps.max(1.0));
        let Some(next) = self.next else {
            self.next = Some(now + period);
            return 1;
        };
        if now < next {
            return 0;
        }
        let late = now - next;
        if late > Duration::from_secs(1) {
            self.next = Some(now + period);
            return 1;
        }
        let due = 1 + (late.as_secs_f64() / period.as_secs_f64()).floor() as u32;
        self.next = Some(next + period * due);
        due
    }

    /// Time until the next frame is due, for `request_repaint_after`.
    pub fn until_next(&self, now: Instant) -> Duration {
        self.next.map(|n| n.saturating_duration_since(now)).unwrap_or(Duration::ZERO)
    }

    /// Forget the schedule, so the next start plays a frame at once.
    pub fn reset(&mut self) {
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn a_fast_display_does_not_speed_the_animation_up() {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        // 144 Hz repaints against a 24 fps timeline for one second.
        let mut frames = 0;
        for i in 0..144 {
            frames += p.frames_due(t0 + secs(i as f64 / 144.0), 24.0);
        }
        assert_eq!(frames, 24);
    }

    #[test]
    fn a_slow_display_catches_up_by_taking_several_frames() {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        assert_eq!(p.frames_due(t0, 24.0), 1);
        // The next repaint arrives 260 ms later: frames 1..=6 (at 1/24 s
        // steps) have all come due.
        assert_eq!(p.frames_due(t0 + secs(0.26), 24.0), 6);
        // And it stays on schedule afterwards: frame 7 is due at 7/24 s.
        assert_eq!(p.frames_due(t0 + secs(0.27), 24.0), 0);
        assert!(p.until_next(t0 + secs(0.27)) <= secs(1.0 / 24.0));
        assert_eq!(p.frames_due(t0 + secs(0.30), 24.0), 1);
    }

    #[test]
    fn a_long_stall_resumes_instead_of_racing() {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        p.frames_due(t0, 24.0);
        assert_eq!(p.frames_due(t0 + secs(10.0), 24.0), 1);
    }

    #[test]
    fn reset_plays_a_frame_immediately_on_restart() {
        let mut p = Pacer::default();
        let t0 = Instant::now();
        p.frames_due(t0, 24.0);
        assert_eq!(p.frames_due(t0 + secs(0.001), 24.0), 0);
        p.reset();
        assert_eq!(p.frames_due(t0 + secs(0.002), 24.0), 1);
    }
}
