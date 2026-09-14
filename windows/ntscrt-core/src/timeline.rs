//! Keyframe animation, ported from `Sources/CrtApp/Timeline.swift`.
//!
//! Keyframes are *master* keyframes: each one snapshots every animatable
//! parameter at once. Parameters you don't touch between two keys therefore
//! interpolate between equal values and hold still on their own, which is
//! what makes "dial in a look, press Keyframe, move, dial in another" work
//! without tracking per-parameter tracks.
//!
//! Times are proportional (0..1), not absolute, so changing the duration
//! stretches the whole animation rather than stranding keys past the end.
//!
//! Easing is carried *per keyframe* and applies to the segment leaving it —
//! CSS semantics, where ease-in means a slow start to that segment.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::settings_ui::{descriptors, SettingDescriptor, SettingKind};
use crate::settings_ui::NtscEffectFullSettings;

/// Segment easing. The string forms are the macOS build's, so preset files
/// move between the two unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Easing {
    #[default]
    #[serde(rename = "Linear")]
    Linear,
    #[serde(rename = "Ease in")]
    EaseIn,
    #[serde(rename = "Ease out")]
    EaseOut,
    #[serde(rename = "Ease in-out")]
    EaseInOut,
    #[serde(rename = "Hold")]
    Hold,
}

impl Easing {
    pub const ALL: [Easing; 5] = [
        Easing::Linear,
        Easing::EaseIn,
        Easing::EaseOut,
        Easing::EaseInOut,
        Easing::Hold,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Easing::Linear => "Linear",
            Easing::EaseIn => "Ease in",
            Easing::EaseOut => "Ease out",
            Easing::EaseInOut => "Ease in-out",
            Easing::Hold => "Hold",
        }
    }

    /// Shape normalised segment progress 0..1.
    ///
    /// `Hold` returns 0 throughout: the segment keeps its starting value and
    /// snaps at the next key, which is what a hold is.
    pub fn apply(self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Easing::Linear => u,
            Easing::EaseIn => u * u,
            Easing::EaseOut => 1.0 - (1.0 - u) * (1.0 - u),
            Easing::EaseInOut => {
                if u < 0.5 {
                    2.0 * u * u
                } else {
                    1.0 - (-2.0 * u + 2.0).powi(2) / 2.0
                }
            }
            Easing::Hold => 0.0,
        }
    }
}

/// A whole-state snapshot at one point on the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyframe {
    /// Normalised position, 0..1.
    pub t: f64,
    #[serde(default)]
    pub easing: Easing,
    /// Shader parameter values by name.
    #[serde(default)]
    pub shader: BTreeMap<String, f32>,
    /// ntsc-rs settings, the same object shape the preset's static block uses.
    #[serde(default)]
    pub ntsc: serde_json::Map<String, serde_json::Value>,
}

/// The timeline as stored in a preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    #[serde(default = "default_duration")]
    pub duration: f64,
    #[serde(default = "default_fps")]
    pub fps: f64,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub keys: Vec<Keyframe>,
}

fn default_duration() -> f64 {
    2.0
}
fn default_fps() -> f64 {
    30.0
}

impl Default for Timeline {
    fn default() -> Self {
        Self { duration: 2.0, fps: 30.0, enabled: false, keys: Vec::new() }
    }
}

impl Timeline {
    pub fn has_keys(&self) -> bool {
        !self.keys.is_empty()
    }

    /// Frames this timeline renders to, at its own duration and rate.
    pub fn frame_count(&self) -> u32 {
        ((self.duration.max(0.0) * self.fps.max(1.0)).round() as u32).max(1)
    }

    /// Normalised position of a frame index.
    pub fn t_for_frame(&self, frame: u32) -> f64 {
        let total = self.frame_count();
        if total <= 1 {
            return 0.0;
        }
        (frame.min(total - 1) as f64) / ((total - 1) as f64)
    }

    /// Keys sorted by time, which the evaluator requires.
    pub fn sorted_keys(&self) -> Vec<Keyframe> {
        let mut k = self.keys.clone();
        k.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        k
    }
}

/// How one ntsc-rs setting interpolates, derived from its descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtscInterp {
    /// Float or percentage: straight lerp.
    Lerp,
    /// Integer range: lerp then round.
    LerpInt,
    /// Booleans, enumerations and anything unrecognised (`version`, group
    /// switches) — an in-between value would be meaningless, so it holds the
    /// starting key's value until the next one.
    Hold,
}

/// Bounds for a shader parameter, so interpolated values stay legal.
#[derive(Debug, Clone, Copy)]
pub struct ShaderMeta {
    pub minimum: f32,
    pub maximum: f32,
    pub step: f32,
}

/// The interpolation rule for every ntsc-rs setting, read off the schema.
pub fn ntsc_interp_table() -> BTreeMap<String, NtscInterp> {
    fn walk(
        descs: &[SettingDescriptor<NtscEffectFullSettings>],
        out: &mut BTreeMap<String, NtscInterp>,
    ) {
        for d in descs {
            let kind = match &d.kind {
                SettingKind::Percentage { .. } | SettingKind::FloatRange { .. } => NtscInterp::Lerp,
                SettingKind::IntRange { .. } => NtscInterp::LerpInt,
                // A group's own value is its enable switch.
                SettingKind::Boolean | SettingKind::Enumeration { .. } => NtscInterp::Hold,
                SettingKind::Group { children } => {
                    walk(children, out);
                    NtscInterp::Hold
                }
            };
            out.insert(d.id.name.to_string(), kind);
        }
    }
    let mut out = BTreeMap::new();
    walk(&descriptors(), &mut out);
    out
}

/// Evaluates a timeline at any point. Built once, then read-only, so it can
/// be handed to an export thread.
pub struct TimelineEvaluator {
    keys: Vec<Keyframe>,
    shader_meta: BTreeMap<String, ShaderMeta>,
    ntsc_interp: BTreeMap<String, NtscInterp>,
}

impl TimelineEvaluator {
    /// None when there are no keyframes — there is nothing to evaluate, and
    /// callers should use the static settings instead.
    pub fn new(
        timeline: &Timeline,
        shader_meta: BTreeMap<String, ShaderMeta>,
        ntsc_interp: BTreeMap<String, NtscInterp>,
    ) -> Option<Self> {
        let keys = timeline.sorted_keys();
        if keys.is_empty() {
            return None;
        }
        Some(Self { keys, shader_meta, ntsc_interp })
    }

    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// The pair of keys bracketing `t`, and the eased progress between them.
    /// Clamps outside the first and last key rather than extrapolating.
    fn segment(&self, t: f64) -> (&Keyframe, &Keyframe, f64) {
        let first = &self.keys[0];
        let last = &self.keys[self.keys.len() - 1];
        if t <= first.t || self.keys.len() == 1 {
            return (first, first, 0.0);
        }
        if t >= last.t {
            return (last, last, 0.0);
        }
        let mut a = first;
        let mut b = last;
        for k in &self.keys {
            if k.t <= t {
                a = k;
            } else {
                b = k;
                break;
            }
        }
        let span = b.t - a.t;
        let raw = if span > 0.0 { (t - a.t) / span } else { 0.0 };
        (a, b, a.easing.apply(raw))
    }

    /// Shader parameters at `t`.
    ///
    /// Interpolated values are snapped to the parameter's own step grid:
    /// imperceptible on fine-step sliders, and it keeps discrete parameters
    /// (toggles, pickers) on legal values instead of meaningless in-betweens.
    pub fn shader_params(&self, t: f64) -> BTreeMap<String, f32> {
        let (a, b, u) = self.segment(t);
        if u == 0.0 {
            return a.shader.clone();
        }
        let mut out = BTreeMap::new();
        for (name, va) in &a.shader {
            let vb = b.shader.get(name).copied().unwrap_or(*va);
            let mut v = va + (vb - va) * u as f32;
            if let Some(m) = self.shader_meta.get(name) {
                if m.step > 0.0 && m.step.is_finite() {
                    v = m.minimum + ((v - m.minimum) / m.step).round() * m.step;
                }
                if m.maximum > m.minimum {
                    v = v.clamp(m.minimum, m.maximum);
                }
            }
            out.insert(name.clone(), v);
        }
        out
    }

    /// ntsc-rs settings at `t`, in the same object shape the stage parses.
    pub fn ntsc_values(&self, t: f64) -> serde_json::Map<String, serde_json::Value> {
        let (a, b, u) = self.segment(t);
        if u == 0.0 {
            return a.ntsc.clone();
        }
        let mut out = serde_json::Map::with_capacity(a.ntsc.len());
        for (name, raw_a) in &a.ntsc {
            let interp = self.ntsc_interp.get(name).copied().unwrap_or(NtscInterp::Hold);

            // A bool is a number in JSON's eyes in some encoders; interpolating
            // one would give 0.4-style garbage for a toggle.
            let (Some(na), Some(nb)) = (
                raw_a.as_f64().filter(|_| !raw_a.is_boolean()),
                b.ntsc
                    .get(name)
                    .and_then(|v| v.as_f64().filter(|_| !v.is_boolean())),
            ) else {
                out.insert(name.clone(), raw_a.clone());
                continue;
            };
            if interp == NtscInterp::Hold {
                out.insert(name.clone(), raw_a.clone());
                continue;
            }

            let v = na + (nb - na) * u;
            let value = match interp {
                NtscInterp::LerpInt => serde_json::json!(v.round() as i64),
                _ => serde_json::json!(v),
            };
            out.insert(name.clone(), value);
        }
        out
    }

    /// ntsc-rs preset JSON at `t`, ready for `NtscStage::set_settings_json`.
    pub fn ntsc_json(&self, t: f64) -> String {
        serde_json::Value::Object(self.ntsc_values(t)).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: f64, easing: Easing, wave: f64, param: f32) -> Keyframe {
        let mut ntsc = serde_json::Map::new();
        ntsc.insert("vhs_edge_wave".into(), serde_json::json!(wave));
        ntsc.insert("chroma_noise".into(), serde_json::json!(true));
        ntsc.insert("chroma_noise_detail".into(), serde_json::json!(2));
        ntsc.insert("filter_type".into(), serde_json::json!(1));
        Keyframe {
            t,
            easing,
            shader: [("CURVATURE".to_string(), param)].into_iter().collect(),
            ntsc,
        }
    }

    fn evaluator(keys: Vec<Keyframe>) -> TimelineEvaluator {
        let tl = Timeline { duration: 2.0, fps: 24.0, enabled: true, keys };
        let interp: BTreeMap<String, NtscInterp> = [
            ("vhs_edge_wave".to_string(), NtscInterp::Lerp),
            ("chroma_noise".to_string(), NtscInterp::Hold),
            ("chroma_noise_detail".to_string(), NtscInterp::LerpInt),
            ("filter_type".to_string(), NtscInterp::Hold),
        ]
        .into_iter()
        .collect();
        TimelineEvaluator::new(&tl, BTreeMap::new(), interp).unwrap()
    }

    // ---- easing ----

    #[test]
    fn easing_endpoints_are_exact() {
        for e in Easing::ALL {
            assert_eq!(e.apply(0.0), 0.0, "{e:?} at 0");
            if e != Easing::Hold {
                assert!((e.apply(1.0) - 1.0).abs() < 1e-12, "{e:?} at 1");
            }
        }
    }

    #[test]
    fn hold_never_advances() {
        for u in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert_eq!(Easing::Hold.apply(u), 0.0);
        }
    }

    #[test]
    fn ease_in_starts_slow_and_ease_out_starts_fast() {
        assert!(Easing::EaseIn.apply(0.25) < 0.25);
        assert!(Easing::EaseOut.apply(0.25) > 0.25);
        // Ease in-out is symmetric about the midpoint.
        assert!((Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-12);
        assert!((Easing::EaseInOut.apply(0.25) + Easing::EaseInOut.apply(0.75) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn easing_is_monotonic_and_clamped() {
        for e in [Easing::Linear, Easing::EaseIn, Easing::EaseOut, Easing::EaseInOut] {
            let mut prev = -1.0;
            for i in 0..=20 {
                let v = e.apply(i as f64 / 20.0);
                assert!(v >= prev - 1e-12, "{e:?} not monotonic at {i}");
                assert!((0.0..=1.0).contains(&v), "{e:?} out of range: {v}");
                prev = v;
            }
        }
        // Out-of-range input clamps rather than extrapolating.
        assert_eq!(Easing::Linear.apply(-1.0), 0.0);
        assert_eq!(Easing::Linear.apply(2.0), 1.0);
    }

    #[test]
    fn easing_names_match_the_preset_files() {
        // These four appear in the bundled presets.
        for (e, s) in [
            (Easing::Linear, "\"Linear\""),
            (Easing::EaseIn, "\"Ease in\""),
            (Easing::EaseOut, "\"Ease out\""),
            (Easing::EaseInOut, "\"Ease in-out\""),
            (Easing::Hold, "\"Hold\""),
        ] {
            assert_eq!(serde_json::to_string(&e).unwrap(), s);
            assert_eq!(serde_json::from_str::<Easing>(s).unwrap(), e);
        }
    }

    // ---- interpolation ----

    #[test]
    fn a_midpoint_lerps_floats() {
        let ev = evaluator(vec![
            key(0.0, Easing::Linear, 0.0, 0.0),
            key(1.0, Easing::Linear, 10.0, 1.0),
        ]);
        let v = ev.ntsc_values(0.5);
        assert!((v["vhs_edge_wave"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        assert!((ev.shader_params(0.5)["CURVATURE"] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn booleans_and_enums_hold_rather_than_blending() {
        let mut a = key(0.0, Easing::Linear, 0.0, 0.0);
        let mut b = key(1.0, Easing::Linear, 10.0, 1.0);
        a.ntsc.insert("chroma_noise".into(), serde_json::json!(true));
        b.ntsc.insert("chroma_noise".into(), serde_json::json!(false));
        a.ntsc.insert("filter_type".into(), serde_json::json!(0));
        b.ntsc.insert("filter_type".into(), serde_json::json!(3));
        let ev = evaluator(vec![a, b]);

        let v = ev.ntsc_values(0.5);
        // A half-on toggle is meaningless, so it keeps the outgoing value.
        assert_eq!(v["chroma_noise"], serde_json::json!(true));
        // Likewise an enumeration: 1.5 is not a filter type.
        assert_eq!(v["filter_type"], serde_json::json!(0));
    }

    #[test]
    fn integer_settings_lerp_then_round() {
        let mut a = key(0.0, Easing::Linear, 0.0, 0.0);
        let mut b = key(1.0, Easing::Linear, 0.0, 0.0);
        a.ntsc.insert("chroma_noise_detail".into(), serde_json::json!(0));
        b.ntsc.insert("chroma_noise_detail".into(), serde_json::json!(4));
        let ev = evaluator(vec![a, b]);

        let v = ev.ntsc_values(0.5);
        assert_eq!(v["chroma_noise_detail"], serde_json::json!(2));
        assert!(v["chroma_noise_detail"].is_i64(), "must stay an integer");
        // And rounds rather than truncating.
        assert_eq!(ev.ntsc_values(0.4)["chroma_noise_detail"], serde_json::json!(2));
    }

    #[test]
    fn easing_shapes_the_segment_it_leaves() {
        let slow = evaluator(vec![
            key(0.0, Easing::EaseIn, 0.0, 0.0),
            key(1.0, Easing::Linear, 10.0, 1.0),
        ]);
        let fast = evaluator(vec![
            key(0.0, Easing::EaseOut, 0.0, 0.0),
            key(1.0, Easing::Linear, 10.0, 1.0),
        ]);
        let s = slow.ntsc_values(0.25)["vhs_edge_wave"].as_f64().unwrap();
        let f = fast.ntsc_values(0.25)["vhs_edge_wave"].as_f64().unwrap();
        // The *first* key's easing governs, not the second's.
        assert!(s < 2.5 && f > 2.5, "ease-in {s} should trail ease-out {f}");
    }

    #[test]
    fn hold_easing_keeps_the_value_until_the_next_key() {
        let ev = evaluator(vec![
            key(0.0, Easing::Hold, 1.0, 0.0),
            key(1.0, Easing::Linear, 9.0, 1.0),
        ]);
        for t in [0.1, 0.5, 0.9] {
            assert_eq!(ev.ntsc_values(t)["vhs_edge_wave"].as_f64().unwrap(), 1.0);
        }
        // And snaps at the key itself.
        assert_eq!(ev.ntsc_values(1.0)["vhs_edge_wave"].as_f64().unwrap(), 9.0);
    }

    #[test]
    fn outside_the_key_range_clamps_rather_than_extrapolating() {
        let ev = evaluator(vec![
            key(0.25, Easing::Linear, 2.0, 0.0),
            key(0.75, Easing::Linear, 6.0, 1.0),
        ]);
        assert_eq!(ev.ntsc_values(0.0)["vhs_edge_wave"].as_f64().unwrap(), 2.0);
        assert_eq!(ev.ntsc_values(1.0)["vhs_edge_wave"].as_f64().unwrap(), 6.0);
    }

    #[test]
    fn a_single_key_is_constant() {
        let ev = evaluator(vec![key(0.4, Easing::Linear, 3.0, 0.5)]);
        for t in [0.0, 0.4, 1.0] {
            assert_eq!(ev.ntsc_values(t)["vhs_edge_wave"].as_f64().unwrap(), 3.0);
        }
    }

    #[test]
    fn keys_are_sorted_so_file_order_does_not_matter() {
        let ev = evaluator(vec![
            key(1.0, Easing::Linear, 10.0, 1.0),
            key(0.0, Easing::Linear, 0.0, 0.0),
        ]);
        assert!((ev.ntsc_values(0.5)["vhs_edge_wave"].as_f64().unwrap() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn three_keys_pick_the_right_segment() {
        let ev = evaluator(vec![
            key(0.0, Easing::Linear, 0.0, 0.0),
            key(0.5, Easing::Linear, 10.0, 1.0),
            key(1.0, Easing::Linear, 0.0, 0.0),
        ]);
        assert!((ev.ntsc_values(0.25)["vhs_edge_wave"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        assert!((ev.ntsc_values(0.75)["vhs_edge_wave"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        assert!((ev.ntsc_values(0.5)["vhs_edge_wave"].as_f64().unwrap() - 10.0).abs() < 1e-9);
    }

    // ---- shader bounds ----

    #[test]
    fn shader_values_snap_to_their_step_and_stay_in_range() {
        let tl = Timeline {
            duration: 2.0,
            fps: 24.0,
            enabled: true,
            keys: vec![key(0.0, Easing::Linear, 0.0, 0.0), key(1.0, Easing::Linear, 0.0, 1.0)],
        };
        let meta: BTreeMap<String, ShaderMeta> = [(
            "CURVATURE".to_string(),
            ShaderMeta { minimum: 0.0, maximum: 1.0, step: 1.0 },
        )]
        .into_iter()
        .collect();
        let ev = TimelineEvaluator::new(&tl, meta, BTreeMap::new()).unwrap();

        // Step 1 makes this a toggle: it must never sit at 0.4.
        for t in [0.1, 0.3, 0.5, 0.7, 0.9] {
            let v = ev.shader_params(t)["CURVATURE"];
            assert!(v == 0.0 || v == 1.0, "got {v} at t={t}");
        }
    }

    // ---- timeline model ----

    #[test]
    fn frame_count_and_positions_span_the_whole_range() {
        let tl = Timeline { duration: 2.0, fps: 24.0, enabled: true, keys: vec![] };
        assert_eq!(tl.frame_count(), 48);
        assert_eq!(tl.t_for_frame(0), 0.0);
        assert_eq!(tl.t_for_frame(47), 1.0);
        // Past the end clamps.
        assert_eq!(tl.t_for_frame(9999), 1.0);
    }

    #[test]
    fn degenerate_timelines_do_not_divide_by_zero() {
        let tl = Timeline { duration: 0.0, fps: 0.0, enabled: true, keys: vec![] };
        assert!(tl.frame_count() >= 1);
        assert_eq!(tl.t_for_frame(0), 0.0);
    }

    #[test]
    fn the_interp_table_classifies_real_ntsc_settings() {
        let table = ntsc_interp_table();
        assert!(!table.is_empty());
        // A known float setting should lerp, a known toggle should hold.
        assert_eq!(table.get("vhs_edge_wave"), Some(&NtscInterp::Lerp));
        assert_eq!(table.get("chroma_noise"), Some(&NtscInterp::Hold));
        // Anything not in the schema (like the preset's own version marker)
        // holds by default at the call site.
        assert!(table.get("version").is_none());
    }

    #[test]
    fn an_empty_timeline_has_no_evaluator() {
        let tl = Timeline::default();
        assert!(TimelineEvaluator::new(&tl, BTreeMap::new(), BTreeMap::new()).is_none());
    }
}
