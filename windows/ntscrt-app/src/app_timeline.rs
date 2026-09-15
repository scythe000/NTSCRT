//! Timeline editing, ported from the keyframe half of
//! `Sources/CrtApp/AppState.swift`.
//!
//! The editing model is After Effects': scrub the playhead, dial in a look,
//! press Keyframe. Nothing is keyed until you press it. Clicking a keyframe
//! jumps to it, and any parameter you then change updates that keyframe in
//! place rather than creating a new one — which is why the panels write
//! straight into the live settings and the keyframe is re-snapshotted on
//! demand.
//!
//! Scrubbing applies the evaluated state to the live settings, so the
//! sidebar always shows what the playhead is showing.

use std::collections::BTreeMap;

use ntscrt_core::{Easing, Keyframe, Timeline, TimelineEvaluator};

use crate::app::NtscrtApp;

/// How close a keyframe has to be to the playhead to count as "the one under
/// it". Matches the macOS tolerance.
const PARK_TOLERANCE: f64 = 0.005;

impl NtscrtApp {
    /// The timeline being edited, created on first use.
    pub fn timeline_mut(&mut self) -> &mut Timeline {
        self.timeline.get_or_insert_with(Timeline::default)
    }

    pub fn timeline_keys(&self) -> &[Keyframe] {
        self.timeline.as_ref().map(|t| t.keys.as_slice()).unwrap_or(&[])
    }

    /// Length and rate of the animation. A clip brings its own; a still uses
    /// whatever the timeline stores.
    pub fn effective_timeline(&self) -> (f64, f64) {
        if let Some(v) = self.video.as_ref() {
            let fps = v.frame_rate().max(1.0);
            return (v.total_frames() as f64 / fps, fps);
        }
        match self.timeline.as_ref() {
            Some(t) => (t.duration, t.fps),
            None => (2.0, 30.0),
        }
    }

    /// Build an evaluator for the current timeline, or None when there is
    /// nothing keyed.
    pub fn timeline_evaluator(&self) -> Option<TimelineEvaluator> {
        let tl = self.timeline.as_ref()?;
        if !tl.has_keys() {
            return None;
        }
        let meta: BTreeMap<String, ntscrt_core::ShaderMeta> = self
            .shader_param_meta
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    ntscrt_core::ShaderMeta {
                        minimum: p.minimum,
                        maximum: p.maximum,
                        step: p.step,
                    },
                )
            })
            .collect();
        TimelineEvaluator::new(tl, meta, ntscrt_core::timeline::ntsc_interp_table())
    }

    /// Move the playhead and show what the animation looks like there.
    pub fn scrub_timeline(&mut self, t: f64) {
        self.playhead = t.clamp(0.0, 1.0);

        // A video's playhead *is* its position, so scrubbing seeks it.
        if self.video.is_some() {
            let (_, fps) = self.effective_timeline();
            let total = self.video.as_ref().map(|v| v.total_frames()).unwrap_or(1);
            let frame = ((self.playhead * (total.saturating_sub(1)) as f64).round() as usize)
                .min(total.saturating_sub(1));
            let _ = fps;
            self.seek_to_frame(frame);
        }

        self.apply_timeline_at_playhead();
    }

    /// Push the evaluated state into the live settings, so the sidebar shows
    /// what the preview is showing.
    pub fn apply_timeline_at_playhead(&mut self) {
        let Some(ev) = self.timeline_evaluator() else { return };
        let t = self.playhead;

        let json = ev.ntsc_json(t);
        if self.ntsc.set_settings_json(&json).is_err() {
            // A keyframe written by a newer ntsc-rs can carry settings this
            // build doesn't know; leaving the previous state is better than
            // clearing the panel.
            return;
        }
        for (name, value) in ev.shader_params(t) {
            if self.shader_params.contains_key(&name) {
                self.set_shader_param(&name, value);
            }
        }
        self.mark_chain_input_edited();
    }

    /// Snapshot every animatable parameter at the playhead.
    ///
    /// Replaces the keyframe already sitting there rather than stacking a
    /// second one on the same spot — that is what makes "click a key, change
    /// something" edit it in place.
    pub fn set_keyframe_at_playhead(&mut self) {
        let ntsc = self
            .ntsc
            .settings_json()
            .ok()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let shader: BTreeMap<String, f32> =
            self.shader_params.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let t = self.playhead;

        let tl = self.timeline_mut();
        match tl.keys.iter().position(|k| (k.t - t).abs() < PARK_TOLERANCE) {
            Some(i) => {
                // Keep the easing already chosen for this key.
                tl.keys[i].ntsc = ntsc;
                tl.keys[i].shader = shader;
            }
            None => {
                tl.keys.push(Keyframe { t, easing: Easing::Linear, shader, ntsc });
                tl.keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
            }
        }
        self.leave_preset();
        self.mark_chain_input_edited();
    }

    /// Index of the keyframe under the playhead, if any.
    pub fn keyframe_at_playhead(&self) -> Option<usize> {
        self.timeline_keys()
            .iter()
            .position(|k| (k.t - self.playhead).abs() < PARK_TOLERANCE)
    }

    pub fn delete_keyframe(&mut self, index: usize) {
        let tl = self.timeline_mut();
        if index < tl.keys.len() {
            tl.keys.remove(index);
        }
        self.leave_preset();
        self.mark_chain_input_edited();
    }

    pub fn clear_keyframes(&mut self) {
        self.timeline_mut().keys.clear();
        self.leave_preset();
        self.mark_chain_input_edited();
    }

    /// Drag a keyframe to a new time.
    pub fn move_keyframe(&mut self, index: usize, t: f64) {
        let t = t.clamp(0.0, 1.0);
        let tl = self.timeline_mut();
        if index >= tl.keys.len() {
            return;
        }
        tl.keys[index].t = t;
        tl.keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        self.leave_preset();
        self.mark_chain_input_edited();
    }

    pub fn set_keyframe_easing(&mut self, index: usize, easing: Easing) {
        let tl = self.timeline_mut();
        if index < tl.keys.len() {
            tl.keys[index].easing = easing;
        }
        self.leave_preset();
        self.mark_chain_input_edited();
    }

    /// Advance the playhead while previewing the animation, looping at the
    /// end. Stills only — a video's playhead is driven by playback.
    pub fn tick_timeline_preview(&mut self) {
        if !self.timeline_playing || self.video.is_some() {
            return;
        }
        let (duration, fps) = self.effective_timeline();
        let frames = (duration * fps).round().max(1.0);
        let step = 1.0 / frames;
        let next = self.playhead + step;
        self.playhead = if next > 1.0 { 0.0 } else { next };
        self.apply_timeline_at_playhead();
    }

    pub fn toggle_timeline_preview(&mut self) {
        // On a video the transport owns playback; the timeline just follows.
        if self.video.is_some() {
            self.toggle_playback();
            return;
        }
        self.timeline_playing = !self.timeline_playing;
    }
}
