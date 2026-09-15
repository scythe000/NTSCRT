//! Colour panel: the controls of the grade stage (`ntscrt_core::grade`).
//!
//! No macOS counterpart — this stage is the Windows build's own. The
//! controls are the flat parameter set the stage defines, laid out in
//! groups; the two colours and the highlight hue get colour pickers rather
//! than three sliders apiece, converting to and from the stored floats.

use eframe::egui;
use ntscrt_core::{grade_param, GRADE_PARAMS};

use crate::app::NtscrtApp;

pub fn show(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Colour")
        .default_open(true)
        .show(ui, |ui| {
            let mut on = app.grade.enabled;
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut on, "Apply colour grade")
                    .on_hover_text(
                        "Grades the degraded, downscaled picture before the CRT draws it: \
                         black and white, tints, solarize, invert, or keep one colour and \
                         grey the rest.",
                    )
                    .changed()
                {
                    app.set_grade_enabled(on);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("Reset")
                        .on_hover_text("Every colour control back to neutral.")
                        .clicked()
                    {
                        app.reset_grade();
                    }
                });
            });
            ui.add_enabled_ui(app.grade.enabled, |ui| controls(app, ui));
        });
}

fn controls(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    let mut changes: Vec<(&'static str, f32)> = Vec::new();

    slider(ui, app, "saturation", "0 is black and white; above 1 pushes colour.", &mut changes);
    slider(ui, app, "contrast", "About mid-grey.", &mut changes);
    slider(ui, app, "brightness", "", &mut changes);
    slider(ui, app, "gamma", "Above 1 lifts the midtones, below 1 sinks them.", &mut changes);
    slider(ui, app, "hue_shift", "Rotates every colour around the wheel, in degrees.", &mut changes);

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Tint").strong().small());
    slider(ui, app, "tint", "How far toward the two-colour version: shadows take the shadow colour, highlights the highlight colour.", &mut changes);
    colour_row(ui, app, "Shadow colour", "tint_shadow", &mut changes);
    colour_row(ui, app, "Highlight colour", "tint_highlight", &mut changes);

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Colour highlight").strong().small());
    slider(ui, app, "keep", "Greys everything except the chosen colour, by this much.", &mut changes);
    hue_row(ui, app, &mut changes);
    slider(ui, app, "keep_width", "How far either side of the chosen colour still counts, in degrees.", &mut changes);

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Effects").strong().small());
    slider(ui, app, "solarize", "Tones above the threshold flip, as in an over-exposed print.", &mut changes);
    slider(ui, app, "solarize_threshold", "", &mut changes);
    slider(ui, app, "invert", "Negative.", &mut changes);

    if !changes.is_empty() {
        for (name, v) in changes {
            app.set_grade_param(name, v);
        }
        app.auto_key_if_parked();
    }
}

fn slider(
    ui: &mut egui::Ui,
    app: &NtscrtApp,
    name: &'static str,
    hover: &str,
    changes: &mut Vec<(&'static str, f32)>,
) {
    let Some(p) = grade_param(name) else { return };
    let mut v = app.grade.get(name);
    let resp = super::labelled_slider(
        ui,
        p.label,
        &mut v,
        p.minimum..=p.maximum,
        Some(p.step as f64),
        false,
        hover,
    );
    if resp.changed() {
        changes.push((name, v));
    }
}

/// A colour picker over three `<prefix>_r/g/b` controls.
fn colour_row(
    ui: &mut egui::Ui,
    app: &NtscrtApp,
    label: &str,
    prefix: &str,
    changes: &mut Vec<(&'static str, f32)>,
) {
    let names: [&'static str; 3] = match prefix {
        "tint_shadow" => ["tint_shadow_r", "tint_shadow_g", "tint_shadow_b"],
        _ => ["tint_highlight_r", "tint_highlight_g", "tint_highlight_b"],
    };
    let mut rgb = [app.grade.get(names[0]), app.grade.get(names[1]), app.grade.get(names[2])];
    let (_, changed) = super::label_row(ui, label, "", |ui| ui.color_edit_button_rgb(&mut rgb).changed());
    if changed {
        for (name, v) in names.iter().zip(rgb) {
            changes.push((name, v));
        }
    }
}

/// The highlight hue as a colour swatch. Stored as an angle on the I/Q
/// (NTSC chroma) plane, which the shader compares hues on; the picker shows
/// a saturated colour at that angle and reads the angle back from whatever
/// is picked.
fn hue_row(ui: &mut egui::Ui, app: &NtscrtApp, changes: &mut Vec<(&'static str, f32)>) {
    let hue = app.grade.get("keep_hue");
    let mut rgb = colour_for_hue(hue);
    let (_, changed) = super::label_row(
        ui,
        "Kept colour",
        "Pick the colour to keep; everything else goes grey by the amount above.",
        |ui| ui.color_edit_button_rgb(&mut rgb).changed(),
    );
    if changed {
        if let Some(h) = hue_for_colour(rgb) {
            changes.push(("keep_hue", h));
        }
    }
}

/// A saturated, mid-luma colour at a given I/Q angle (degrees).
pub(crate) fn colour_for_hue(deg: f32) -> [f32; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    let (y, i, q) = (0.5f32, 0.35 * c, 0.35 * s);
    [
        (y + 0.956 * i + 0.621 * q).clamp(0.0, 1.0),
        (y - 0.272 * i - 0.647 * q).clamp(0.0, 1.0),
        (y - 1.106 * i + 1.703 * q).clamp(0.0, 1.0),
    ]
}

/// The I/Q angle of a colour, in 0..360; None for a grey, which has none.
pub(crate) fn hue_for_colour(rgb: [f32; 3]) -> Option<f32> {
    let [r, g, b] = rgb;
    let i = 0.596 * r - 0.274 * g - 0.322 * b;
    let q = 0.211 * r - 0.523 * g + 0.312 * b;
    if (i * i + q * q).sqrt() < 0.02 {
        return None;
    }
    let deg = q.atan2(i).to_degrees();
    Some(if deg < 0.0 { deg + 360.0 } else { deg })
}

#[allow(dead_code)]
fn all_names_have_controls() -> bool {
    // Every declared control appears in the panel; a new control added to
    // GRADE_PARAMS without a row here would be reachable only from presets.
    const SHOWN: &[&str] = &[
        "saturation", "contrast", "brightness", "gamma", "hue_shift", "tint",
        "tint_shadow_r", "tint_shadow_g", "tint_shadow_b",
        "tint_highlight_r", "tint_highlight_g", "tint_highlight_b",
        "keep", "keep_hue", "keep_width", "solarize", "solarize_threshold", "invert",
    ];
    GRADE_PARAMS.iter().all(|p| SHOWN.contains(&p.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grade_control_has_a_row() {
        assert!(all_names_have_controls());
    }

    #[test]
    fn hue_round_trips_through_a_colour() {
        for deg in [0.0f32, 45.0, 120.0, 200.0, 300.0, 359.0] {
            let back = hue_for_colour(colour_for_hue(deg)).unwrap();
            let d = (back - deg).abs().min(360.0 - (back - deg).abs());
            assert!(d < 1.5, "{deg} -> {back}");
        }
        assert!(hue_for_colour([0.5, 0.5, 0.5]).is_none());
    }

    #[test]
    fn pure_red_reads_as_the_reference_hue() {
        // The colour-highlight test in ntscrt-core uses this angle for red.
        let h = hue_for_colour([1.0, 0.0, 0.0]).unwrap();
        assert!((h - 0.211f32.atan2(0.596).to_degrees()).abs() < 0.5, "{h}");
    }
}
