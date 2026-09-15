//! The About box: version, commit, build date, GPU, ffmpeg, library
//! versions, with a Copy button so the whole thing can be pasted into a bug
//! report. Opened from the toolbar's About button or F1.

use eframe::egui;

use crate::about::{describe_adapter, describe_ffmpeg, BUILD, REPOSITORY};
use crate::app::{NtscrtApp, RenderState};

pub fn show(app: &mut NtscrtApp, ctx: &egui::Context, rs: Option<&RenderState>) {
    if !app.about_open {
        return;
    }
    // ffmpeg is a subprocess; probe once per opening, not per frame.
    if app.about_ffmpeg.is_none() {
        app.about_ffmpeg = Some(describe_ffmpeg());
    }
    let gpu = rs.map(|rs| describe_adapter(&rs.adapter.get_info()));
    let ffmpeg = app.about_ffmpeg.clone().unwrap_or_default();

    let mut open = app.about_open;
    egui::Window::new("About NTSCRT")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("NTSCRT for Windows").heading().strong());
                ui.label(egui::RichText::new(format!("Version {}", BUILD.short())).monospace());
                ui.label(
                    egui::RichText::new("NTSC/VHS signal emulation and CRT shaders").weak(),
                );
                ui.add_space(8.0);
            });

            egui::Grid::new("about_grid").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: &str| {
                    ui.label(egui::RichText::new(k).weak());
                    ui.label(egui::RichText::new(v).monospace());
                    ui.end_row();
                };
                row(ui, "Build", &format!("{} {}", BUILD.target, BUILD.profile));
                row(ui, "GPU", gpu.as_deref().unwrap_or("no GPU context"));
                row(ui, "ffmpeg", &ffmpeg);
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
                    let text = BUILD.report(gpu.as_deref(), Some(&ffmpeg));
                    match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
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
    if !open {
        // Probe afresh next time: the user may have just installed ffmpeg.
        app.about_ffmpeg = None;
    }
}
