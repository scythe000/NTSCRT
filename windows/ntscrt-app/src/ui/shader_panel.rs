//! CRT panel, ported from `Views/ShaderPanel.swift`.
//!
//! Seven RetroArch CRT presets with every runtime parameter exposed, in the
//! order the shader author declared them. Each control is chosen from the
//! parameter's own `#pragma parameter` line by [`presentation`]: a
//! zero-width range is a section header, a small enumeration a picker (with
//! names when the description carries a "[A, B, C]" legend), an
//! integer-stepped moderate range a stepper, an unnamed 0/1 a checkbox, and
//! everything else a slider.
//!
//! Grayed-out controls tell you which switch activates them — many CRT
//! parameters only apply when their feature (curvature, mask, geometry
//! mode...) is on. See `param_gates.rs` for the rules.

use eframe::egui;

use crate::app::{NtscrtApp, RenderState};
use crate::gpu::chain::ShaderParamMeta;
use crate::param_gates;

pub fn show(app: &mut NtscrtApp, ui: &mut egui::Ui, rs: Option<&RenderState>) {
    egui::CollapsingHeader::new("CRT")
        .default_open(true)
        .show(ui, |ui| {
            let mut on = app.shader_enabled;
            if ui
                .checkbox(&mut on, "Apply CRT shader")
                .on_hover_text(
                    "Off shows the signal stage on its own: the (optionally downscaled) \
                     source, each retro pixel a hard block.",
                )
                .changed()
            {
                app.set_shader_enabled(on);
            }

            // Everything below is dimmed while the shader is off, but still
            // readable: you can see what will come back.
            ui.add_enabled_ui(app.shader_enabled, |ui| shader_config(app, ui, rs));
        });
}

fn shader_config(app: &mut NtscrtApp, ui: &mut egui::Ui, rs: Option<&RenderState>) {
    let current = crate::presets::find(&app.shader_id)
        .map(|p| p.display_name)
        .unwrap_or("\u{2014}");

    let mut pending: Option<String> = None;
    egui::ComboBox::from_id_salt("shader_preset")
        .selected_text(current)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for p in crate::presets::ALL {
                if ui
                    .selectable_label(app.shader_id == p.id, p.display_name)
                    .clicked()
                {
                    pending = Some(p.id.to_string());
                }
            }
        });
    if let Some(id) = pending {
        match rs {
            Some(rs) => app.select_shader(&id, &rs.device, &rs.queue),
            None => app.shader_id = id,
        }
    }

    if !app.has_chain() {
        ui.label(
            egui::RichText::new("Shader not loaded.")
                .small()
                .color(egui::Color32::from_rgb(230, 100, 100)),
        );
        return;
    }

    if app.shader_param_meta.is_empty() {
        ui.label(egui::RichText::new("This preset exposes no runtime parameters.").weak());
        return;
    }

    ui.separator();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Parameters ({})", app.shader_param_meta.len())).weak(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Reset")
                .on_hover_text("Every parameter back to this shader's defaults.")
                .clicked()
            {
                app.reset_shader_params();
            }
        });
    });

    // Gate conditions are evaluated against a snapshot so every control in
    // this pass sees the same values, rather than some reading state a
    // sibling changed mid-loop.
    let values = app.shader_params.clone();
    let (_, input_height) = app.chain_input_size();
    let meta = app.shader_param_meta.clone();
    let shader_id = app.shader_id.clone();

    let mut changes: Vec<(String, f32)> = Vec::new();
    let mut ctx = Controls { values: &values, input_height, shader_id: &shader_id, changes: &mut changes };

    let (pre, sections) = split_sections(&meta);
    for p in pre {
        ctx.control(ui, p);
    }
    for section in sections {
        // Hyllian-style "SCANLINES SETTINGS:" groups collapse per shader.
        egui::CollapsingHeader::new(egui::RichText::new(section.title).small().weak())
            .id_salt(("shader_section", &shader_id, section.title))
            .default_open(true)
            .show(ui, |ui| {
                for p in &section.params {
                    ctx.control(ui, p);
                }
            });
    }

    if !changes.is_empty() {
        for (name, v) in changes {
            app.set_shader_param(&name, v);
        }
        app.leave_preset();
        app.auto_key_if_parked();
    }
}

/// One frame's worth of control state, shared by every parameter row.
struct Controls<'a> {
    values: &'a std::collections::HashMap<String, f32>,
    input_height: u32,
    shader_id: &'a str,
    changes: &'a mut Vec<(String, f32)>,
}

impl Controls<'_> {
    fn control(&mut self, ui: &mut egui::Ui, p: &ShaderParamMeta) {
        let pres = presentation(p);

        if let Kind::Header = pres.kind {
            header(ui, &pres.title);
            return;
        }

        let gate = param_gates::gate(self.shader_id, &p.name, &p.description);
        let (enabled, hint) = match &gate {
            Some(g) => match &g.condition {
                Some(c) => (
                    param_gates::is_satisfied(c, self.values, Some(self.input_height)),
                    Some(g.hint),
                ),
                // Informational only: the control stays enabled.
                None => (true, Some(g.hint)),
            },
            None => (true, None),
        };

        let before = *self.values.get(&p.name).unwrap_or(&p.initial);
        let mut v = before;
        let hover = hover_text(p, hint);

        ui.add_enabled_ui(enabled, |ui| {
            let resp = match &pres.kind {
                Kind::Header => unreachable!(),
                Kind::Toggle => {
                    let mut on = v >= 0.5;
                    let r = ui.checkbox(&mut on, pres.title.as_str());
                    if r.changed() {
                        v = if on { 1.0 } else { 0.0 };
                    }
                    r
                }
                Kind::Picker { values, labels, segmented } => {
                    picker(ui, &pres.title, values, labels.as_deref(), *segmented, &mut v)
                }
                Kind::Stepper { step } => stepper(ui, &pres.title, p, *step, &mut v),
                Kind::Slider => super::labelled_slider(
                    ui,
                    &pres.title,
                    &mut v,
                    p.minimum..=p.maximum,
                    (p.step > 0.0 && p.step.is_finite()).then_some(p.step as f64),
                    false,
                    // The hover below covers the whole control.
                    "",
                ),
            };
            let mut resp = resp.on_hover_text(&hover);
            if let (Some(h), false) = (hint, enabled) {
                resp = resp.on_disabled_hover_text(h);
            }
            let _ = resp;
        });

        if let Some(caption) = &pres.caption {
            ui.label(egui::RichText::new(format!("    {caption}")).small().weak());
        }
        if let Some(h) = hint {
            if !enabled {
                ui.label(egui::RichText::new(format!("    {h}")).small().weak());
            }
        }

        if (v - before).abs() > f32::EPSILON {
            self.changes.push((p.name.clone(), v));
        }
    }
}

fn header(ui: &mut egui::Ui, title: &str) {
    if !title.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(title).small().weak());
    }
}

/// A small enumeration: a row of segments for a few short choices, a
/// drop-down otherwise. The selection snaps to the nearest declared value.
fn picker(
    ui: &mut egui::Ui,
    title: &str,
    values: &[f32],
    labels: Option<&[String]>,
    segmented: bool,
    v: &mut f32,
) -> egui::Response {
    let label_of = |i: usize| -> String {
        labels
            .and_then(|l| l.get(i).cloned())
            .unwrap_or_else(|| format_choice(values[i]))
    };
    let mut selected = nearest_index(values, *v);

    let mut resp = ui.add(egui::Label::new(title).truncate());
    if segmented {
        let row = ui.horizontal_wrapped(|ui| {
            let mut any: Option<egui::Response> = None;
            for i in 0..values.len() {
                let r = ui.selectable_value(&mut selected, i, label_of(i));
                any = Some(match any {
                    Some(a) => a | r,
                    None => r,
                });
            }
            any
        });
        if let Some(r) = row.inner {
            resp |= r;
        }
    } else {
        let combo = egui::ComboBox::from_id_salt(("shader_picker", title))
            .selected_text(label_of(selected))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for i in 0..values.len() {
                    ui.selectable_value(&mut selected, i, label_of(i));
                }
            });
        resp |= combo.response;
    }
    if let Some(chosen) = values.get(selected) {
        *v = *chosen;
    }
    resp
}

/// An integer with a moderate range: a number field with step buttons.
fn stepper(
    ui: &mut egui::Ui,
    title: &str,
    p: &ShaderParamMeta,
    step: i32,
    v: &mut f32,
) -> egui::Response {
    let (lo, hi) = (p.minimum.round() as i32, p.maximum.round() as i32);
    let mut i = v.round() as i32;
    let step = step.max(1);
    let (label, controls) = super::label_row(ui, title, "", |ui| {
        let plus = ui.small_button("+");
        let field = ui.add(egui::DragValue::new(&mut i).range(lo..=hi).speed(0.05));
        let minus = ui.small_button("\u{2212}");
        if plus.clicked() {
            i = (i + step).min(hi);
        }
        if minus.clicked() {
            i = (i - step).max(lo);
        }
        plus | field | minus
    });
    *v = i as f32;
    label | controls
}

fn nearest_index(values: &[f32], v: f32) -> usize {
    values
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (*a - v).abs().partial_cmp(&(*b - v).abs()).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Integer values render without ".0"; others to two places.
fn format_choice(v: f32) -> String {
    if v == v.round() {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

/// Tooltip: the uniform name (so it can be matched against preset JSON), the
/// declared range, and the gate hint when there is one.
fn hover_text(p: &ShaderParamMeta, hint: Option<&str>) -> String {
    let mut s = p.name.clone();
    if p.has_usable_range() && !p.is_toggle() {
        s.push_str(&format!("\nrange {} .. {}", p.minimum, p.maximum));
        if p.step > 0.0 && p.step.is_finite() {
            s.push_str(&format!("  step {}", p.step));
        }
    }
    s.push_str(&format!("\ndefault {}", p.initial));
    if let Some(h) = hint {
        s.push('\n');
        s.push_str(h);
    }
    s
}

// ---- classification -------------------------------------------------------

/// What kind of control fits a declared parameter best.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Non-interactive label row. Shaders declare params with min == max as
    /// section headers / comments (e.g. Hyllian's "COLOR SETTINGS:").
    Header,
    /// Unnamed 0/1 switch.
    Toggle,
    /// Small fixed enumeration. `labels` names the choices when the
    /// description carried a matching "[A, B, C]" legend; `segmented` lays
    /// the few short choices out in a row rather than a drop-down.
    Picker { values: Vec<f32>, labels: Option<Vec<String>>, segmented: bool },
    /// Integer-stepped value with a moderate range.
    Stepper { step: i32 },
    /// Continuous value.
    Slider,
}

/// How a parameter is presented: the control, the title (description with
/// any consumed "[...]" legend stripped), and an optional caption — a legend
/// that names ranges rather than one choice per value, e.g. PHOSPHOR_LAYOUT's
/// "[1-6 APERT, 7-10 DOT, 11-14 SLOT, 15-17 LOTTES]".
#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    pub kind: Kind,
    pub title: String,
    pub caption: Option<String>,
}

/// Port of `presentation(for:)` in `ShaderPanel.swift`; the rules are the
/// same so the two apps present each shader identically.
pub fn presentation(p: &ShaderParamMeta) -> Presentation {
    let lo = p.minimum as f64;
    let hi = p.maximum as f64;
    let step = p.step as f64;
    let range = hi - lo;

    let desc = p.description.trim();
    let fallback_title = if desc.is_empty() { p.name.as_str() } else { desc };

    // Label pseudo-params: nothing to adjust, the description is the point.
    // NaN ranges land here too, as a header: there is nothing to adjust.
    if range.is_nan() || range <= 0.0 {
        return Presentation { kind: Kind::Header, title: desc.to_string(), caption: None };
    }
    if step.is_nan() || step <= 0.0 || !step.is_finite() {
        return Presentation { kind: Kind::Slider, title: fallback_title.to_string(), caption: None };
    }

    let (stripped, legend) = extract_legend(desc);
    let title = if stripped.is_empty() { p.name.clone() } else { stripped };

    // Discrete iff the step divides the range into a whole number of
    // intervals — this also catches fractional-step enums like crt-royale's
    // subpixel offsets (-0.333…0.333, step 0.333 → three choices).
    let steps = range / step;
    let is_discrete = (steps - steps.round()).abs() < 1e-3;
    let count = steps.round() as usize + 1;
    let is_int_step = step == step.round() && lo == lo.round() && hi == hi.round();

    if is_discrete && (2..=8).contains(&count) {
        // A comma-separated legend naming exactly one choice per value
        // becomes the picker labels ("[SPHERE, CYLINDER]" etc.).
        let labels: Option<Vec<String>> = legend.as_deref().and_then(|l| {
            let tokens: Vec<String> = l.split(',').map(|t| t.trim().to_string()).collect();
            (tokens.len() == count && tokens.iter().all(|t| !t.is_empty())).then_some(tokens)
        });

        // Plain unnamed on/off → toggle.
        if count == 2 && lo == 0.0 && hi == 1.0 && is_int_step && labels.is_none() {
            return Presentation { kind: Kind::Toggle, title, caption: legend };
        }

        let values: Vec<f32> = (0..count).map(|i| (lo + i as f64 * step) as f32).collect();
        // Segments need room: only for few choices with short labels.
        let combined = labels.as_ref().map(|l| l.iter().map(|s| s.chars().count()).sum()).unwrap_or(0);
        let segmented = count <= 4 && combined <= 24;
        let caption = if labels.is_none() { legend } else { None };
        return Presentation { kind: Kind::Picker { values, labels, segmented }, title, caption };
    }

    // Not a small enum: any legend stays visible as a caption under the control.
    if is_int_step && is_discrete && count <= 32 {
        return Presentation { kind: Kind::Stepper { step: step as i32 }, title, caption: legend };
    }
    Presentation { kind: Kind::Slider, title, caption: legend }
}

/// Split the first "[...]" group out of a description.
/// "Curvature Shape [SPHERE, CYLINDER]" → ("Curvature Shape", "SPHERE, CYLINDER").
fn extract_legend(desc: &str) -> (String, Option<String>) {
    let Some(open) = desc.find('[') else { return (desc.to_string(), None) };
    let Some(close_rel) = desc[open..].find(']') else { return (desc.to_string(), None) };
    let close = open + close_rel;
    let legend = &desc[open + 1..close];
    if legend.trim().is_empty() {
        return (desc.to_string(), None);
    }
    let title = format!("{}{}", &desc[..open], &desc[close + 1..])
        .replace("  ", " ")
        .trim()
        .to_string();
    (title, Some(legend.to_string()))
}

/// A run of parameters under one shader-declared section header.
struct Section<'a> {
    title: &'a str,
    params: Vec<&'a ShaderParamMeta>,
}

/// Split the flat parameter list at section headers. Params before the
/// first header stay flat; an empty header closes the current section, and
/// "//"-comment headers stay inline as captions rather than opening one.
fn split_sections(all: &[ShaderParamMeta]) -> (Vec<&ShaderParamMeta>, Vec<Section<'_>>) {
    let mut pre: Vec<&ShaderParamMeta> = Vec::new();
    let mut sections: Vec<Section<'_>> = Vec::new();
    let mut current: Option<Section<'_>> = None;

    for p in all {
        let pres = presentation(p);
        if pres.kind == Kind::Header {
            let title = p.description.trim();
            if title.is_empty() {
                sections.extend(current.take());
            } else if title.starts_with("//") {
                match current.as_mut() {
                    Some(c) => c.params.push(p),
                    None => pre.push(p),
                }
            } else {
                sections.extend(current.take());
                current = Some(Section { title, params: Vec::new() });
            }
        } else {
            match current.as_mut() {
                Some(c) => c.params.push(p),
                None => pre.push(p),
            }
        }
    }
    sections.extend(current.take());
    (pre, sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(desc: &str, lo: f32, hi: f32, step: f32) -> ShaderParamMeta {
        ShaderParamMeta {
            name: "P".into(),
            description: desc.into(),
            initial: lo,
            minimum: lo,
            maximum: hi,
            step,
        }
    }

    #[test]
    fn zero_width_range_is_a_header() {
        let p = presentation(&param("SCANLINES SETTINGS:", 0.0, 0.0, 0.0));
        assert_eq!(p.kind, Kind::Header);
        assert_eq!(p.title, "SCANLINES SETTINGS:");
    }

    #[test]
    fn unnamed_zero_one_is_a_toggle() {
        assert_eq!(presentation(&param("Enable Glow", 0.0, 1.0, 1.0)).kind, Kind::Toggle);
    }

    #[test]
    fn named_two_way_choice_is_a_segmented_picker_not_a_toggle() {
        let p = presentation(&param("Curvature Shape [SPHERE, CYLINDER]", 0.0, 1.0, 1.0));
        assert_eq!(p.title, "Curvature Shape");
        assert_eq!(p.caption, None);
        assert_eq!(
            p.kind,
            Kind::Picker {
                values: vec![0.0, 1.0],
                labels: Some(vec!["SPHERE".into(), "CYLINDER".into()]),
                segmented: true,
            }
        );
    }

    #[test]
    fn fractional_step_enum_is_a_picker() {
        let p = presentation(&param("Subpixel offset", -0.333, 0.333, 0.333));
        match p.kind {
            Kind::Picker { values, labels: None, .. } => assert_eq!(values.len(), 3),
            k => panic!("expected picker, got {k:?}"),
        }
    }

    #[test]
    fn legend_that_does_not_match_the_choices_becomes_a_caption() {
        let p = presentation(&param(
            "Phosphor Layout [1-6 APERT, 7-10 DOT, 11-14 SLOT, 15-17 LOTTES]",
            0.0, 17.0, 1.0,
        ));
        assert_eq!(p.title, "Phosphor Layout");
        assert_eq!(p.kind, Kind::Stepper { step: 1 });
        assert_eq!(p.caption.as_deref(), Some("1-6 APERT, 7-10 DOT, 11-14 SLOT, 15-17 LOTTES"));
    }

    #[test]
    fn many_choices_use_a_drop_down() {
        let p = presentation(&param("Mask type", 0.0, 7.0, 1.0));
        assert!(matches!(p.kind, Kind::Picker { segmented: false, .. }));
    }

    #[test]
    fn wide_integer_range_is_a_slider() {
        assert_eq!(presentation(&param("Lines", 0.0, 1000.0, 1.0)).kind, Kind::Slider);
    }

    #[test]
    fn continuous_range_is_a_slider_and_empty_description_falls_back_to_the_name() {
        let p = presentation(&param("", 0.0, 2.0, 0.01));
        assert_eq!(p.kind, Kind::Slider);
        assert_eq!(p.title, "P");
    }

    #[test]
    fn zero_step_is_a_plain_slider() {
        assert_eq!(presentation(&param("Gamma", 1.0, 3.0, 0.0)).kind, Kind::Slider);
    }

    #[test]
    fn sections_split_at_headers_and_comments_stay_inline() {
        let all = vec![
            param("Brightness", 0.0, 2.0, 0.01),
            param("SCANLINES:", 0.0, 0.0, 0.0),
            param("Beam min", 0.0, 2.0, 0.01),
            param("// try 1.0 for sharp", 0.0, 0.0, 0.0),
            param("Beam max", 0.0, 2.0, 0.01),
            param("", 0.0, 0.0, 0.0),
            param("After blank", 0.0, 1.0, 0.1),
        ];
        let (pre, sections) = split_sections(&all);
        assert_eq!(pre.len(), 2, "Brightness, then After blank once the section closed");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "SCANLINES:");
        assert_eq!(sections[0].params.len(), 3, "the comment stays inline");
    }

    #[test]
    fn choice_formatting() {
        assert_eq!(format_choice(2.0), "2");
        assert_eq!(format_choice(0.333), "0.33");
        assert_eq!(nearest_index(&[0.0, 0.5, 1.0], 0.6), 1);
    }
}
