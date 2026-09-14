//! CRT panel, ported from `Views/ShaderPanel.swift`.
//!
//! Seven RetroArch CRT presets with every runtime parameter exposed.
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

            ui.separator();

            // Parameter values are read before the loop so gate conditions
            // see a consistent snapshot rather than values changing mid-pass.
            let values = app.shader_params.clone();
            let (_, input_height) = app.chain_input_size();

            let mut names: Vec<String> = values.keys().cloned().collect();
            names.sort();

            if names.is_empty() {
                ui.label(egui::RichText::new("This preset exposes no runtime parameters.").weak());
            }

            for name in names {
                let description = app
                    .shader_param_descriptions
                    .get(&name)
                    .cloned()
                    .unwrap_or_default();
                let gate = param_gates::gate(&app.shader_id, &name, &description);

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

                let mut v = *values.get(&name).unwrap_or(&0.0);
                ui.add_enabled_ui(enabled, |ui| {
                    // librashader does not surface per-parameter ranges, so
                    // the control is a drag value rather than a bounded
                    // slider — typing an exact number is what the macOS
                    // NumericField is for anyway.
                    let mut resp = ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut v)
                                .speed(0.01)
                                .max_decimals(4),
                        );
                        ui.label(&name);
                    })
                    .response;
                    if let Some(h) = hint {
                        resp = resp.on_hover_text(h);
                    }
                    let _ = resp;
                });

                if (v - *values.get(&name).unwrap_or(&0.0)).abs() > f32::EPSILON {
                    app.set_shader_param(&name, v);
                }

                if let Some(h) = hint {
                    if !enabled {
                        ui.label(egui::RichText::new(format!("   {h}")).small().weak());
                    }
                }
            }
        });
}
