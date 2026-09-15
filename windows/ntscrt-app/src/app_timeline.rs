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

    /// Frames the animation renders to: the clip's own count for a video,
    /// duration × rate for a still.
    pub fn effective_frame_count(&self) -> usize {
        if let Some(v) = self.video.as_ref() {
            return v.total_frames();
        }
        let (duration, fps) = self.effective_timeline();
        ((duration.max(0.0) * fps.max(1.0)).round() as usize).max(1)
    }

    /// Quantise a normalised time to the nearest frame, so a dragged or
    /// nudged keyframe lands where a rendered frame actually is. Frame 0
    /// is t=0 and the last frame is t=1, matching `Timeline::t_for_frame`.
    pub fn snap_to_frame(&self, t: f64) -> f64 {
        let total = self.effective_frame_count();
        if total <= 1 {
            return 0.0;
        }
        let last = (total - 1) as f64;
        (t.clamp(0.0, 1.0) * last).round() / last
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

    /// The keyframed NTSC settings per clip frame, for the playback and
    /// pre-render producers. None when nothing is keyed, so they use the
    /// live settings as before.
    pub(crate) fn per_frame_ntsc_json(&self) -> Option<crate::video::playback::PerFrameJson> {
        let ev = self.timeline_evaluator()?;
        let total = self.effective_frame_count();
        Some(std::sync::Arc::new(move |frame: usize| {
            let t = if total > 1 { frame as f64 / (total - 1) as f64 } else { 0.0 };
            Some(ev.ntsc_json(t))
        }))
    }

    /// A playing video has moved to `frame`: put the playhead there and show
    /// the animation's state without treating it as an edit — the frame the
    /// producer baked (and the cache holds) already has these settings in it,
    /// so nothing upstream is stale.
    pub(crate) fn follow_video_frame(&mut self, frame: usize) {
        let total = self.effective_frame_count();
        self.playhead = if total > 1 { frame as f64 / (total - 1) as f64 } else { 0.0 };
        self.apply_timeline_state_at_playhead();
    }

    /// Move the playhead and show what the animation looks like there.
    pub fn scrub_timeline(&mut self, t: f64) {
        let t = t.clamp(0.0, 1.0);

        // A video's playhead *is* its position, quantised to frames, so
        // scrubbing seeks it; the seek puts the playhead on the frame and
        // applies the animation there.
        if let Some(video) = self.video.as_ref() {
            let last = video.total_frames().saturating_sub(1);
            let frame = ((t * last as f64).round() as usize).min(last);
            self.seek_to_frame(frame);
            return;
        }

        self.playhead = t;
        self.apply_timeline_at_playhead();
    }

    /// Push the evaluated state into the live settings, so the sidebar shows
    /// what the preview is showing.
    ///
    /// Moving the playhead is not an edit: with keyframes the producers bake
    /// each frame from the animation (`per_frame_ntsc_json`), so cached
    /// frames stay valid and nothing upstream needs invalidating — only the
    /// preview needs redrawing.
    pub fn apply_timeline_at_playhead(&mut self) {
        self.apply_timeline_state_at_playhead();
    }

    fn apply_timeline_state_at_playhead(&mut self) {
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
        self.mark_dirty();
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

    /// Drag a keyframe to a new time. Returns the key's index afterwards:
    /// the list stays sorted by time, so dragging one key past another
    /// changes both their indices, and a drag in progress has to follow the
    /// key rather than the slot it started in.
    pub fn move_keyframe(&mut self, index: usize, t: f64) -> Option<usize> {
        let t = t.clamp(0.0, 1.0);
        let tl = self.timeline_mut();
        if index >= tl.keys.len() {
            return None;
        }
        let mut key = tl.keys.remove(index);
        key.t = t;
        // Insert after any key already at or before `t`, so a key dragged
        // onto another's exact time settles on the far side of it and the
        // order of the two stays what the drag direction implies.
        let at = tl.keys.partition_point(|k| k.t <= t);
        tl.keys.insert(at, key);
        self.leave_preset();
        self.mark_chain_input_edited();
        Some(at)
    }

    /// Step a keyframe by whole frames (arrow keys). Returns its new index.
    pub fn nudge_keyframe(&mut self, index: usize, frames: i64) -> Option<usize> {
        let total = self.effective_frame_count();
        let t = self.timeline_keys().get(index)?.t;
        if total <= 1 {
            return Some(index);
        }
        let last = (total - 1) as f64;
        let frame = ((t * last).round() as i64 + frames).clamp(0, last as i64);
        self.move_keyframe(index, frame as f64 / last)
    }

    /// Whether the arrow keys belong to the timeline right now — a key is
    /// parked under the playhead, so Left/Right nudge it — rather than to
    /// the transport bar's frame stepping.
    pub fn timeline_owns_arrows(&self) -> bool {
        self.timeline_open && self.keyframe_at_playhead().is_some()
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
