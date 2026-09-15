//! CRT panel, ported from `Views/ShaderPanel.swift`.
//!
//! Seven RetroArch CRT presets with every runtime parameter exposed, in the
//! order the shader author declared them. Controls are built from each
//! parameter's own `#pragma parameter` line, so a 0..1 step-1 parameter
//! becomes a checkbox and everything else a slider over its declared range.
//!
//! Grayed-out controls tell you which switch activates them — many CRT
//! parameters only apply when their feature (curvature, mask, geometry
//! mode...) is on. See `param_gates.rs` for the rules.

use eframe::egui;

use crate::app::{NtscrtApp, RenderState};
use crate::param_gates;

pub fn show(app: &mut NtscrtApp, ui: &mut egui::Ui, rs: Option<&RenderState>) {
    egui::CollapsingHeader::new("CRT")
        .default_open(true)
        .show(ui, |ui| {
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
                app.shader_id = id;
                if let Some(rs) = rs {
                    app.reload_chain(&rs.device, &rs.queue);
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
                ui.label(
                    egui::RichText::new("This preset exposes no runtime parameters.").weak(),
                );
                return;
            }

            ui.separator();

            if ui.button("Reset to preset defaults").clicked() {
                let defaults: Vec<(String, f32)> = app
                    .shader_param_meta
                    .iter()
                    .map(|p| (p.name.clone(), p.initial))
                    .collect();
                for (name, v) in defaults {
                    app.set_shader_param(&name, v);
                }
            }

            // Gate conditions are evaluated against a snapshot so every
            // control in this pass sees the same values, rather than some
            // reading state a sibling changed mid-loop.
            let values = app.shader_params.clone();
            let (_, input_height) = app.chain_input_size();
            let meta = app.shader_param_meta.clone();
            let shader_id = app.shader_id.clone();

            let mut changes: Vec<(String, f32)> = Vec::new();

            for p in &meta {
                let gate = param_gates::gate(&shader_id, &p.name, &p.description);
                let (enabled, hint) = match &gate {
                    Some(g) => match &g.condition {
                        Some(c) => (
                            param_gates::is_satisfied(c, &values, Some(input_height)),
                            Some(g.hint),
                        ),
                        // Informational only: the control stays enabled.
                        None => (true, Some(g.hint)),
                    },
                    None => (true, None),
                };

                let mut v = *values.get(&p.name).unwrap_or(&p.initial);
                // The shader's own label reads better than the raw uniform
                // name, but keep the name in the tooltip so preset JSON stays
                // findable.
                let label = if p.description.is_empty() { &p.name } else { &p.description };

                ui.add_enabled_ui(enabled, |ui| {
                    let resp = if p.is_toggle() {
                        let mut on = v >= 0.5;
                        let r = ui.checkbox(&mut on, label);
                        if r.changed() {
                            v = if on { 1.0 } else { 0.0 };
                        }
                        r
                    } else if p.has_usable_range() {
                        super::labelled_slider(
                            ui,
                            label,
                            &mut v,
                            p.minimum..=p.maximum,
                            Some(p.step as f64),
                            false,
                            // The hover below covers the whole control.
                            "",
                        )
                    } else {
                        // Degenerate declared range — fall back to a plain
                        // number field rather than a slider that can't move.
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::DragValue::new(&mut v).speed(0.01).max_decimals(4));
                            ui.label(label);
                            r
                        })
                        .inner
                    };

                    let mut resp = resp.on_hover_text(hover_text(p, hint));
                    if let Some(h) = hint {
                        if !enabled {
                            resp = resp.on_disabled_hover_text(h);
                        }
                    }
                    let _ = resp;
                });

                if (v - *values.get(&p.name).unwrap_or(&p.initial)).abs() > f32::EPSILON {
                    changes.push((p.name.clone(), v));
                }

                if !enabled {
                    if let Some(h) = hint {
                        ui.label(egui::RichText::new(format!("    {h}")).small().weak());
                    }
                }
            }

            for (name, v) in changes {
                app.set_shader_param(&name, v);
            }
        });
}

/// Tooltip: the uniform name (so it can be matched against preset JSON), the
/// declared range, and the gate hint when there is one.
fn hover_text(p: &crate::gpu::chain::ShaderParamMeta, hint: Option<&str>) -> String {
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
