//! Downscale panel, ported from `Views/DownscalePanel.swift`.
//!
//! The retro horizontal resolution the CRT shader sees. Height always follows
//! the source's aspect ratio, so any input shape works.

use eframe::egui;
use ntscrt_core::DownscaleMethod;

use crate::app::{NtscrtApp, DOWNSCALE_PRESETS};

pub fn show(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Downscale")
        .default_open(true)
        .show(ui, |ui| {
            if ui
                .checkbox(&mut app.downscale_enabled, "Downscale before shader")
                .changed()
            {
                app.mark_chain_input_edited();
            }

            ui.add_enabled_ui(app.downscale_enabled, |ui| {
                ui.label(egui::RichText::new("Horizontal resolution").weak());

                let current_label = current_label(app);
                egui::ComboBox::from_id_salt("downscale_preset")
                    .selected_text(current_label)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (label, width) in DOWNSCALE_PRESETS {
                            if ui.selectable_label(false, *label).clicked() {
                                app.downscale_width = *width;
                                app.downscale_preset = label.to_string();
                                app.mark_chain_input_edited();
                            }
                        }
                        ui.separator();
                        if ui.selectable_label(false, "Custom").clicked() {
                            app.downscale_preset = "Custom".to_string();
                        }
                    });

                ui.horizontal(|ui| {
                    ui.label("W");
                    let resp = ui.add(
                        egui::DragValue::new(&mut app.downscale_width)
                            .range(16..=4096)
                            .speed(1),
                    );
                    if resp.changed() {
                        // Editing the width by hand demotes the selection to Custom.
                        if let Some((_, w)) = DOWNSCALE_PRESETS
                            .iter()
                            .find(|(l, _)| *l == app.downscale_preset)
                        {
                            if *w != app.downscale_width {
                                app.downscale_preset = "Custom".to_string();
                            }
                        }
                        app.mark_chain_input_edited();
                    }

                    if let Some(spec) = app.downscale_spec() {
                        ui.label(
                            egui::RichText::new(format!(
                                "\u{2192} {}\u{00D7}{}",
                                spec.width, spec.height
                            ))
                            .monospace()
                            .weak(),
                        )
                        .on_hover_text("Height follows the source's aspect ratio.");
                    }
                });

                ui.label(egui::RichText::new("Sampling").weak());
                egui::ComboBox::from_id_salt("downscale_method")
                    .selected_text(app.downscale_method.display_name())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for m in DownscaleMethod::ALL {
                            if ui
                                .selectable_label(app.downscale_method == m, m.display_name())
                                .clicked()
                            {
                                app.downscale_method = m;
                                app.mark_chain_input_edited();
                            }
                        }
                    });

                if app.downscale_method == DownscaleMethod::Nearest {
                    ui.label(
                        egui::RichText::new(
                            "Tip: on video, Nearest shimmers in detailed areas — Nearest+ \
                             keeps the punch without the flicker.",
                        )
                        .small()
                        .weak(),
                    );
                }
            });
        });
}

fn current_label(app: &NtscrtApp) -> String {
    if let Some((label, w)) = DOWNSCALE_PRESETS
        .iter()
        .find(|(l, _)| *l == app.downscale_preset)
    {
        if *w == app.downscale_width {
            return label.to_string();
        }
    }
    format!("Custom ({}px)", app.downscale_width)
}
