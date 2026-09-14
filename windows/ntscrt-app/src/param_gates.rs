//! Why a shader parameter can look "dead", ported verbatim from
//! `Sources/CrtApp/ParamGates.swift`.
//!
//! Most CRT parameters only take effect when another parameter opens their
//! code path, a few need particular input resolutions, and a couple are
//! compile-time disabled in the shader source itself (they do nothing in
//! RetroArch either). The UI uses these rules to gray out inactive controls
//! and say what would activate them.
//!
//! Every rule here was verified empirically upstream with `crt-sweep`: the
//! parameter shows ~zero pixel diff across its whole range with the gate
//! closed, and a substantial diff with it open. They are shader facts, not
//! platform facts, so they carry over to Windows unchanged.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum GateCondition {
    /// Another parameter must be >= the threshold.
    ParamAtLeast(&'static str, f32),
    /// Another parameter must be < the threshold.
    ParamBelow(&'static str, f32),
    /// The chain input (downscaled size if enabled, else source) must be at
    /// least this many lines tall.
    InputHeightAtLeast(u32),
    /// Compile-time disabled in this build of the shader source — behaves
    /// identically in RetroArch.
    Never,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamGate {
    /// None = informational only: the control stays enabled, the hint shows.
    pub condition: Option<GateCondition>,
    pub hint: &'static str,
}

impl ParamGate {
    const fn new(condition: GateCondition, hint: &'static str) -> Self {
        Self { condition: Some(condition), hint }
    }
    const fn note(hint: &'static str) -> Self {
        Self { condition: None, hint }
    }
}

/// The gate for a parameter, if any.
///
/// `description` is the shader's own description string, needed for the
/// hyllian wildcard rule below.
pub fn gate(preset_id: &str, param_name: &str, description: &str) -> Option<ParamGate> {
    if let Some(g) = lookup(preset_id, param_name) {
        return Some(g);
    }
    // Hyllian marks preset-overridden params with a leading "*":
    // "// Presets greater than 0 disable options with '*'."
    if preset_id == "hyllian" && description.trim_start().starts_with('*') {
        return Some(ParamGate::new(
            GateCondition::ParamBelow("PRESET_OPTION", 0.5),
            "Overridden unless Mask Preset is CUSTOM",
        ));
    }
    None
}

pub fn is_satisfied(
    condition: &GateCondition,
    param_values: &HashMap<String, f32>,
    input_height: Option<u32>,
) -> bool {
    match condition {
        GateCondition::ParamAtLeast(name, threshold) => {
            param_values.get(*name).copied().unwrap_or(0.0) >= *threshold
        }
        GateCondition::ParamBelow(name, threshold) => {
            param_values.get(*name).copied().unwrap_or(0.0) < *threshold
        }
        GateCondition::InputHeightAtLeast(lines) => {
            input_height.is_some_and(|h| h >= *lines)
        }
        GateCondition::Never => false,
    }
}

// ---- rules ----

/// The two CRT Glow variants share one parameter set.
const GLOW_GATES: &[(&str, ParamGate)] = &[
    ("warpX",        ParamGate::new(GateCondition::ParamAtLeast("CURVATURE", 0.5), "Requires Curvature")),
    ("warpY",        ParamGate::new(GateCondition::ParamAtLeast("CURVATURE", 0.5), "Requires Curvature")),
    ("cornersize",   ParamGate::new(GateCondition::ParamAtLeast("CURVATURE", 0.5), "Requires Curvature")),
    ("cornersmooth", ParamGate::new(GateCondition::ParamAtLeast("CURVATURE", 0.5), "Requires Curvature")),
    ("noise_amt",    ParamGate::new(GateCondition::ParamAtLeast("CURVATURE", 0.5), "Requires Curvature")),
    ("maskDark",     ParamGate::new(GateCondition::ParamAtLeast("shadowMask", 0.5), "Requires Mask Effect > 0")),
    ("maskLight",    ParamGate::new(GateCondition::ParamAtLeast("shadowMask", 0.5), "Requires Mask Effect > 0")),
];

const HYLLIAN_GATES: &[(&str, ParamGate)] = &[
    ("GLOW_WHITEPOINT", ParamGate::new(GateCondition::ParamAtLeast("GLOW_ENABLE", 0.5), "Requires Enable Glow")),
    ("GLOW_ROLLOFF",    ParamGate::new(GateCondition::ParamAtLeast("GLOW_ENABLE", 0.5), "Requires Enable Glow")),
    ("GLOW_RADIUS",     ParamGate::new(GateCondition::ParamAtLeast("GLOW_ENABLE", 0.5), "Requires Enable Glow")),
    ("GLOW_STRENGTH",   ParamGate::new(GateCondition::ParamAtLeast("GLOW_ENABLE", 0.5), "Requires Enable Glow")),
    ("h_shape",         ParamGate::new(GateCondition::ParamAtLeast("h_curvature", 0.5), "Requires Curvature")),
    ("h_radius",        ParamGate::new(GateCondition::ParamAtLeast("h_curvature", 0.5), "Requires Curvature")),
    ("h_cornersize",    ParamGate::new(GateCondition::ParamAtLeast("h_curvature", 0.5), "Requires Curvature")),
    ("h_cornersmooth",  ParamGate::new(GateCondition::ParamAtLeast("h_curvature", 0.5), "Requires Curvature")),
    ("DISPLAY_RES",     ParamGate::new(GateCondition::ParamAtLeast("PRESET_OPTION", 0.5), "Only used by non-CUSTOM mask presets")),
    ("MASK_STRENGTH",   ParamGate::new(GateCondition::ParamBelow("PRESET_OPTION", 0.5), "Overridden unless Mask Preset is CUSTOM")),
    // Only mask layouts with asymmetric RGB triads respond to a subpixel-order
    // swap; magenta/green layouts are symmetric.
    ("MONITOR_SUBPIXELS", ParamGate::note("Only affects RGB-triad mask layouts")),
];

const ROYALE_GATES: &[(&str, ParamGate)] = &[
    ("geom_tilt_angle_x", ParamGate::new(GateCondition::ParamAtLeast("geom_mode_runtime", 0.5), "Requires Geometry Mode != flat")),
    ("geom_tilt_angle_y", ParamGate::new(GateCondition::ParamAtLeast("geom_mode_runtime", 0.5), "Requires Geometry Mode != flat")),
    ("geom_view_dist",    ParamGate::new(GateCondition::ParamAtLeast("geom_mode_runtime", 0.5), "Requires Geometry Mode != flat")),
    ("geom_radius",       ParamGate::new(GateCondition::ParamAtLeast("geom_mode_runtime", 0.5), "Requires Geometry Mode != flat")),
    ("aa_cubic_c",        ParamGate::new(GateCondition::ParamAtLeast("geom_mode_runtime", 0.5), "Requires Geometry Mode != flat")),
    ("mask_num_triads_desired", ParamGate::new(GateCondition::ParamAtLeast("mask_specify_num_triads", 0.5), "Requires Specify Number of Triads")),
    ("interlace_detect_toggle", ParamGate::new(GateCondition::InputHeightAtLeast(480), "Needs a >=480-line input")),
    ("interlace_bff",     ParamGate::new(GateCondition::InputHeightAtLeast(480), "Needs a >=480-line input")),
    ("interlace_1080i",   ParamGate::new(GateCondition::InputHeightAtLeast(1080), "Needs a 1080-line input")),
    // Compile-time static in user-settings.h (RUNTIME_* undefined) — these do
    // nothing in RetroArch with this shader build either.
    ("aa_subpixel_r_offset_x_runtime", ParamGate::new(GateCondition::Never, "Static in this shader build")),
    ("aa_subpixel_r_offset_y_runtime", ParamGate::new(GateCondition::Never, "Static in this shader build")),
    ("aa_gauss_sigma",    ParamGate::new(GateCondition::Never, "Static in this shader build")),
    ("beam_horiz_sigma",  ParamGate::new(GateCondition::Never, "Static in this shader build")),
];

const EASYMODE_GATES: &[(&str, ParamGate)] = &[
    ("MASK_DOT_HEIGHT", ParamGate::new(GateCondition::ParamAtLeast("MASK_STAGGER", 1.0), "Requires Mask Stagger > 0")),
];

const APERTURE_GATES: &[(&str, ParamGate)] = &[
    // The shader computes the half-line offset only when
    // floor(outputHeight / inputHeight) is even — at odd scale factors the
    // toggle is a byte-identical no-op (same in RetroArch). Integer scale
    // makes the factor explicit.
    ("SCANLINE_OFFSET", ParamGate::note("Only shifts at even output/input scale factors")),
];

const SIM_GATES: &[(&str, ParamGate)] = &[
    ("animate_artifacts", ParamGate::note("Visible with Animate on")),
];

fn lookup(preset_id: &str, param_name: &str) -> Option<ParamGate> {
    let table: &[(&str, ParamGate)] = match preset_id {
        "glow_gauss" | "glow_lanczos" => GLOW_GATES,
        "hyllian" => HYLLIAN_GATES,
        "royale" => ROYALE_GATES,
        "easymode" => EASYMODE_GATES,
        "aperture" => APERTURE_GATES,
        "sim" => SIM_GATES,
        _ => return None,
    };
    table.iter().find(|(n, _)| *n == param_name).map(|(_, g)| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, f32)]) -> HashMap<String, f32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn both_glow_variants_share_the_same_gates() {
        let a = gate("glow_gauss", "warpX", "");
        let b = gate("glow_lanczos", "warpX", "");
        assert_eq!(a, b);
        assert!(a.is_some());
    }

    #[test]
    fn curvature_gate_opens_and_closes() {
        let g = gate("glow_gauss", "warpX", "").unwrap();
        let c = g.condition.unwrap();
        assert!(!is_satisfied(&c, &values(&[("CURVATURE", 0.0)]), None));
        assert!(is_satisfied(&c, &values(&[("CURVATURE", 1.0)]), None));
        // A missing parameter reads as 0, so the gate stays shut.
        assert!(!is_satisfied(&c, &values(&[]), None));
    }

    #[test]
    fn hyllian_star_prefix_is_treated_as_preset_overridden() {
        // Not in the explicit table, but starred in its description.
        let g = gate("hyllian", "SOME_STARRED_PARAM", "  *Mask Layout").unwrap();
        assert_eq!(
            g.condition,
            Some(GateCondition::ParamBelow("PRESET_OPTION", 0.5))
        );
        // Unstarred params get no gate.
        assert!(gate("hyllian", "SOME_OTHER_PARAM", "Mask Layout").is_none());
    }

    #[test]
    fn explicit_table_wins_over_the_star_rule() {
        // MASK_STRENGTH is in the table; a starred description must not
        // replace its specific gate.
        let g = gate("hyllian", "MASK_STRENGTH", "*whatever").unwrap();
        assert_eq!(g.hint, "Overridden unless Mask Preset is CUSTOM");
    }

    #[test]
    fn input_height_gates_respect_the_chain_input() {
        let g = gate("royale", "interlace_1080i", "").unwrap();
        let c = g.condition.unwrap();
        assert!(!is_satisfied(&c, &values(&[]), Some(240)));
        assert!(!is_satisfied(&c, &values(&[]), Some(480)));
        assert!(is_satisfied(&c, &values(&[]), Some(1080)));
        // No source loaded yet.
        assert!(!is_satisfied(&c, &values(&[]), None));
    }

    #[test]
    fn never_gates_are_always_closed() {
        let g = gate("royale", "aa_gauss_sigma", "").unwrap();
        let c = g.condition.unwrap();
        assert!(!is_satisfied(&c, &values(&[("anything", 1.0)]), Some(2160)));
        assert_eq!(g.hint, "Static in this shader build");
    }

    #[test]
    fn informational_gates_carry_a_hint_but_no_condition() {
        let g = gate("aperture", "SCANLINE_OFFSET", "").unwrap();
        assert!(g.condition.is_none());
        assert!(!g.hint.is_empty());
    }

    #[test]
    fn unknown_presets_and_params_have_no_gates() {
        assert!(gate("royale", "not_a_real_param", "").is_none());
        assert!(gate("not_a_preset", "warpX", "").is_none());
    }
}
