//! The colour-grade stage: the one part of the pipeline the macOS app never
//! had.
//!
//! ```text
//! source → NTSC/VHS → downscale → **grade** → CRT shader → output
//! ```
//!
//! It sits on the chain input — after the signal stage has degraded the
//! picture and it has been brought down to retro resolution, before the CRT
//! draws it. That is where a tint reads as the *set* being lit that way and
//! where a selective-colour highlight stays clean instead of being smeared
//! by chroma noise; and it is downstream of everything the frame cache
//! stores, so dialling a colour while a video plays costs one small GPU pass
//! and invalidates nothing.
//!
//! The parameters are a flat, named set of floats — the same shape as
//! shader parameters — so keyframes, presets and the panel treat them
//! generically. Colours are three floats apiece so they interpolate.
//!
//! This module holds the model; `ntscrt-app`'s `gpu::grade` runs it on the
//! GPU. The math lives in one place, `grade.wgsl`; the order of operations
//! there is the one documented on [`GRADE_PARAMS`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One grade control.
#[derive(Debug, Clone, Copy)]
pub struct GradeParam {
    pub name: &'static str,
    pub label: &'static str,
    pub minimum: f32,
    pub maximum: f32,
    pub default: f32,
    pub step: f32,
}

/// Every grade control, in panel order. The shader applies them in this
/// order too:
///
/// 1. hue shift (rotation in YIQ, so it stays in the NTSC family)
/// 2. colour highlight — desaturate everything *outside* a hue band
/// 3. saturation
/// 4. contrast and brightness about mid-grey
/// 5. gamma
/// 6. tint — a duotone from the shadow colour to the highlight colour by
///    luma, mixed in by amount
/// 7. solarize — channels above the threshold invert, mixed in by amount
/// 8. invert
pub const GRADE_PARAMS: &[GradeParam] = &[
    GradeParam { name: "hue_shift", label: "Hue shift", minimum: -180.0, maximum: 180.0, default: 0.0, step: 1.0 },
    GradeParam { name: "keep", label: "Keep colour", minimum: 0.0, maximum: 1.0, default: 0.0, step: 0.01 },
    GradeParam { name: "keep_hue", label: "Kept hue", minimum: 0.0, maximum: 360.0, default: 20.0, step: 1.0 },
    GradeParam { name: "keep_width", label: "Hue width", minimum: 5.0, maximum: 180.0, default: 30.0, step: 1.0 },
    GradeParam { name: "saturation", label: "Saturation", minimum: 0.0, maximum: 2.0, default: 1.0, step: 0.01 },
    GradeParam { name: "contrast", label: "Contrast", minimum: 0.0, maximum: 2.0, default: 1.0, step: 0.01 },
    GradeParam { name: "brightness", label: "Brightness", minimum: -1.0, maximum: 1.0, default: 0.0, step: 0.01 },
    GradeParam { name: "gamma", label: "Gamma", minimum: 0.25, maximum: 3.0, default: 1.0, step: 0.01 },
    GradeParam { name: "tint", label: "Tint", minimum: 0.0, maximum: 1.0, default: 0.0, step: 0.01 },
    GradeParam { name: "tint_shadow_r", label: "Shadow colour R", minimum: 0.0, maximum: 1.0, default: 0.0, step: 0.01 },
    GradeParam { name: "tint_shadow_g", label: "Shadow colour G", minimum: 0.0, maximum: 1.0, default: 0.35, step: 0.01 },
    GradeParam { name: "tint_shadow_b", label: "Shadow colour B", minimum: 0.0, maximum: 1.0, default: 0.45, step: 0.01 },
    GradeParam { name: "tint_highlight_r", label: "Highlight colour R", minimum: 0.0, maximum: 1.0, default: 1.0, step: 0.01 },
    GradeParam { name: "tint_highlight_g", label: "Highlight colour G", minimum: 0.0, maximum: 1.0, default: 0.6, step: 0.01 },
    GradeParam { name: "tint_highlight_b", label: "Highlight colour B", minimum: 0.0, maximum: 1.0, default: 0.2, step: 0.01 },
    GradeParam { name: "solarize", label: "Solarize", minimum: 0.0, maximum: 1.0, default: 0.0, step: 0.01 },
    GradeParam { name: "solarize_threshold", label: "Solarize threshold", minimum: 0.0, maximum: 1.0, default: 0.5, step: 0.01 },
    GradeParam { name: "invert", label: "Invert", minimum: 0.0, maximum: 1.0, default: 0.0, step: 0.01 },
];

/// Look a control up by name.
pub fn grade_param(name: &str) -> Option<&'static GradeParam> {
    GRADE_PARAMS.iter().find(|p| p.name == name)
}

/// Number of `f32`s in the GPU uniform: five `vec4`s.
pub const GRADE_UNIFORM_LEN: usize = 20;

/// The stage's settings: an on/off switch and a value per control. Missing
/// values read as the control's default, so a preset or keyframe that
/// predates a control still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grade {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, rename = "params")]
    pub values: BTreeMap<String, f32>,
}

impl Default for Grade {
    fn default() -> Self {
        Self { enabled: false, values: BTreeMap::new() }
    }
}

impl Grade {
    /// A control's current value, or its default when unset.
    pub fn get(&self, name: &str) -> f32 {
        self.values
            .get(name)
            .copied()
            .or_else(|| grade_param(name).map(|p| p.default))
            .unwrap_or(0.0)
    }

    /// Set a control, clamped to its range. Unknown names are ignored.
    pub fn set(&mut self, name: &str, value: f32) {
        if let Some(p) = grade_param(name) {
            self.values.insert(name.to_string(), value.clamp(p.minimum, p.maximum));
        }
    }

    /// Every control back to its default (the switch is left alone).
    pub fn reset(&mut self) {
        self.values.clear();
    }

    /// Every control's value, defaults filled in — what a keyframe snapshots.
    pub fn all_values(&self) -> BTreeMap<String, f32> {
        GRADE_PARAMS.iter().map(|p| (p.name.to_string(), self.get(p.name))).collect()
    }

    /// Whether running the stage would change any pixel. Off, or every
    /// control at a value that is a no-op, means the pass can be skipped —
    /// which is the common case for every preset that predates the stage.
    pub fn is_identity(&self) -> bool {
        if !self.enabled {
            return true;
        }
        let eq = |name: &str, v: f32| (self.get(name) - v).abs() < 1e-6;
        eq("hue_shift", 0.0)
            && eq("keep", 0.0)
            && eq("saturation", 1.0)
            && eq("contrast", 1.0)
            && eq("brightness", 0.0)
            && eq("gamma", 1.0)
            && eq("tint", 0.0)
            && eq("solarize", 0.0)
            && eq("invert", 0.0)
    }

    /// The controls packed for the GPU, in the layout `grade.wgsl` declares:
    ///
    /// ```text
    /// vec4  hue_shift(rad)  keep        keep_hue(rad)   keep_width(rad)
    /// vec4  saturation      contrast    brightness      gamma
    /// vec4  tint            shadow.rgb
    /// vec4  highlight.rgb                               solarize
    /// vec4  solarize_thr    invert      0               0
    /// ```
    pub fn uniform(&self) -> [f32; GRADE_UNIFORM_LEN] {
        let g = |n: &str| self.get(n);
        [
            g("hue_shift").to_radians(),
            g("keep"),
            g("keep_hue").to_radians(),
            g("keep_width").to_radians(),
            g("saturation"),
            g("contrast"),
            g("brightness"),
            g("gamma").max(0.01),
            g("tint"),
            g("tint_shadow_r"),
            g("tint_shadow_g"),
            g("tint_shadow_b"),
            g("tint_highlight_r"),
            g("tint_highlight_g"),
            g("tint_highlight_b"),
            g("solarize"),
            g("solarize_threshold"),
            g("invert"),
            0.0,
            0.0,
        ]
    }
}

/// CPU reference of the shader, pixel by pixel — the same math as
/// `grade.wgsl`, kept here so the two can be checked against each other and
/// so presets can be reasoned about in tests without a GPU. `rgb` in 0..1.
pub fn grade_pixel(u: &[f32; GRADE_UNIFORM_LEN], rgb: [f32; 3]) -> [f32; 3] {
    fn luma(c: [f32; 3]) -> f32 {
        0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
    }
    fn mix(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }
    fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
        [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
    }
    fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    let [hue, keep, keep_hue, keep_width, sat, con, bri, gam, tint, sr, sg, sb, hr, hg, hb, sol, thr, inv, _, _] =
        *u;
    let mut c = rgb;

    // 1. hue shift: rotate the chroma plane of YIQ.
    let y = luma(c);
    let i = 0.596 * c[0] - 0.274 * c[1] - 0.322 * c[2];
    let q = 0.211 * c[0] - 0.523 * c[1] + 0.312 * c[2];
    let (s, co) = hue.sin_cos();
    let (i2, q2) = (i * co - q * s, i * s + q * co);
    c = [
        y + 0.956 * i2 + 0.621 * q2,
        y - 0.272 * i2 - 0.647 * q2,
        y - 1.106 * i2 + 1.703 * q2,
    ];

    // 2. colour highlight: keep the band around keep_hue, grey the rest.
    if keep > 0.0 {
        let pixel_hue = q2.atan2(i2); // -pi..pi, 0 at the I axis (orange-red)
        let mut d = (pixel_hue - keep_hue).abs() % std::f32::consts::TAU;
        if d > std::f32::consts::PI {
            d = std::f32::consts::TAU - d;
        }
        // Fully kept inside half the width, fully greyed past the width.
        let inside = 1.0 - smoothstep(keep_width * 0.5, keep_width, d);
        let chroma = (i2 * i2 + q2 * q2).sqrt();
        // Grey pixels have no hue to keep; don't let them flicker between bands.
        let inside = inside * smoothstep(0.0, 0.08, chroma);
        let l = luma(c);
        c = mix3([l, l, l], c, mix(1.0 - keep, 1.0, inside));
    }

    // 3. saturation
    let l = luma(c);
    c = mix3([l, l, l], c, sat);

    // 4. contrast and brightness
    c = [
        (c[0] - 0.5) * con + 0.5 + bri,
        (c[1] - 0.5) * con + 0.5 + bri,
        (c[2] - 0.5) * con + 0.5 + bri,
    ];

    // 5. gamma
    let inv_g = 1.0 / gam;
    c = [
        c[0].max(0.0).powf(inv_g),
        c[1].max(0.0).powf(inv_g),
        c[2].max(0.0).powf(inv_g),
    ];

    // 6. tint (duotone)
    if tint > 0.0 {
        let l = luma(c).clamp(0.0, 1.0);
        let duo = mix3([sr, sg, sb], [hr, hg, hb], l);
        c = mix3(c, duo, tint);
    }

    // 7. solarize
    if sol > 0.0 {
        let solar = |v: f32| if v > thr { 1.0 - v } else { v };
        c = mix3(c, [solar(c[0]), solar(c[1]), solar(c[2])], sol);
    }

    // 8. invert
    c = mix3(c, [1.0 - c[0], 1.0 - c[1], 1.0 - c[2]], inv);

    [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
    }

    #[test]
    fn defaults_are_an_identity() {
        let mut g = Grade::default();
        assert!(g.is_identity(), "off is identity");
        g.enabled = true;
        assert!(g.is_identity(), "on with defaults is identity");
        let u = g.uniform();
        for rgb in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.8, 0.2, 0.1], [0.3, 0.6, 0.9]] {
            assert!(close(grade_pixel(&u, rgb), rgb), "{rgb:?} -> {:?}", grade_pixel(&u, rgb));
        }
        g.set("saturation", 0.0);
        assert!(!g.is_identity());
    }

    #[test]
    fn black_and_white_leaves_only_luma() {
        let mut g = Grade { enabled: true, ..Default::default() };
        g.set("saturation", 0.0);
        let out = grade_pixel(&g.uniform(), [0.8, 0.2, 0.1]);
        let l = 0.299 * 0.8 + 0.587 * 0.2 + 0.114 * 0.1;
        assert!(close(out, [l, l, l]), "{out:?}");
    }

    #[test]
    fn invert_and_solarize_do_what_they_say() {
        let mut g = Grade { enabled: true, ..Default::default() };
        g.set("invert", 1.0);
        assert!(close(grade_pixel(&g.uniform(), [0.8, 0.2, 0.1]), [0.2, 0.8, 0.9]));

        let mut g = Grade { enabled: true, ..Default::default() };
        g.set("solarize", 1.0);
        g.set("solarize_threshold", 0.5);
        // Above the threshold flips, below stays.
        assert!(close(grade_pixel(&g.uniform(), [0.8, 0.2, 0.6]), [0.2, 0.2, 0.4]));
    }

    #[test]
    fn colour_highlight_keeps_the_chosen_hue_and_greys_the_rest() {
        let mut g = Grade { enabled: true, ..Default::default() };
        g.set("keep", 1.0);
        // Hue of pure red on the I/Q plane.
        let (i, q) = (0.596f32, 0.211f32);
        g.set("keep_hue", q.atan2(i).to_degrees());
        g.set("keep_width", 40.0);
        let u = g.uniform();
        let red = grade_pixel(&u, [0.9, 0.1, 0.1]);
        assert!(close(red, [0.9, 0.1, 0.1]), "red should survive: {red:?}");
        let blue = grade_pixel(&u, [0.1, 0.2, 0.9]);
        assert!((blue[0] - blue[1]).abs() < 1e-3 && (blue[1] - blue[2]).abs() < 1e-3, "blue should grey: {blue:?}");
    }

    #[test]
    fn tint_pulls_shadows_and_highlights_to_their_colours() {
        let mut g = Grade { enabled: true, ..Default::default() };
        g.set("tint", 1.0);
        for (n, v) in [("tint_shadow_r", 0.0), ("tint_shadow_g", 0.0), ("tint_shadow_b", 1.0),
                       ("tint_highlight_r", 1.0), ("tint_highlight_g", 1.0), ("tint_highlight_b", 0.0)] {
            g.set(n, v);
        }
        let u = g.uniform();
        assert!(close(grade_pixel(&u, [0.0, 0.0, 0.0]), [0.0, 0.0, 1.0]));
        assert!(close(grade_pixel(&u, [1.0, 1.0, 1.0]), [1.0, 1.0, 0.0]));
    }

    #[test]
    fn values_clamp_and_missing_ones_read_as_defaults() {
        let mut g = Grade::default();
        g.set("saturation", 9.0);
        assert_eq!(g.get("saturation"), 2.0);
        g.set("no_such_control", 1.0);
        assert!(g.values.get("no_such_control").is_none());
        assert_eq!(g.get("gamma"), 1.0);
        assert_eq!(g.all_values().len(), GRADE_PARAMS.len());
    }

    #[test]
    fn serialises_as_enabled_plus_params() {
        let mut g = Grade { enabled: true, ..Default::default() };
        g.set("invert", 1.0);
        let json = serde_json::to_value(&g).unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["params"]["invert"], 1.0);
        let back: Grade = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(back.enabled && back.values.is_empty());
    }
}
