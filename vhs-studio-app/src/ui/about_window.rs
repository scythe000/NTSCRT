//! The About box: version, commit, build date and library versions, with a
//! Copy button so the whole thing can be pasted into a bug report. Opened
//! from the toolbar's About button or F1.
//!
//! Everything shown is known at compile time. An earlier version also
//! probed ffmpeg (a subprocess) on opening, which on Windows made the box
//! take a visible moment to appear; the live details are left to
//! `vhs-studio-smoke --version` and `--ffmpeg`.

use eframe::egui;

use crate::about::{BUILD, ORIGIN, ORIGIN_NAME, REPOSITORY};
use crate::app::VhsStudioApp;

pub fn show(app: &mut VhsStudioApp, ctx: &egui::Context) {
    if !app.about_open {
        return;
    }

    let mut open = app.about_open;
    egui::Window::new("About VHS-Studio")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            // Wide enough that the version line, the longest, stays on one
            // line; the grid used to set the width with the GPU row.
            ui.set_min_width(380.0);
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("VHS-Studio").heading().strong());
                ui.label(egui::RichText::new(format!("Version {}", BUILD.short())).monospace());
                ui.label(
                    egui::RichText::new("NTSC/VHS signal emulation and CRT shaders").weak(),
                );
                ui.hyperlink_to(format!("Based on {ORIGIN_NAME}, for macOS"), ORIGIN)
                    .on_hover_text(ORIGIN);
                ui.add_space(8.0);
            });

            egui::Grid::new("about_grid").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: &str| {
                    ui.label(egui::RichText::new(k).weak());
                    ui.label(egui::RichText::new(v).monospace());
                    ui.end_row();
                };
                row(ui, "Build", &format!("{} {}", BUILD.target, BUILD.profile));
                row(ui, "ntsc-rs", BUILD.ntsc_rs);
                row(ui, "librashader", BUILD.librashader);
                row(ui, "wgpu", BUILD.wgpu);
                row(ui, "egui", BUILD.egui);
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Copy")
                    .on_hover_text("Copy all of this as text, for a bug report.")
                    .clicked()
                {
                    match arboard::Clipboard::new().and_then(|mut c| c.set_text(BUILD.report(None))) {
                        Ok(()) => app.status = Some("Build details copied".into()),
                        Err(e) => app.error = Some(format!("Could not copy: {e}")),
                    }
                }
                ui.hyperlink_to("GitHub", REPOSITORY);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("MIT OR ISC OR Apache-2.0").small().weak());
                });
            });
        });
    app.about_open = open;
}
