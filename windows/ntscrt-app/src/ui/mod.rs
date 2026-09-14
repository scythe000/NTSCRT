//! The window: toolbar, sidebar panels, preview.
//!
//! Ported from `Sources/CrtApp/Views/`. The macOS build puts file actions in
//! the window toolbar and the creative pipeline in a sidebar, top to bottom
//! in signal order — that layout is kept here, with egui's idioms standing in
//! for SwiftUI's.

mod downscale_panel;
mod ntsc_panel;
mod preview_panel;
mod shader_panel;

use eframe::egui;

use crate::app::{NtscrtApp, RenderState};

pub fn top_bar(app: &mut NtscrtApp, root: &mut egui::Ui, rs: Option<&RenderState>) {
    let ctx = root.ctx().clone();
    egui::Panel::top("toolbar").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            if ui.button("Open\u{2026}").on_hover_text("Ctrl+O").clicked() {
                pick_source(app);
            }
            if ui.button("Export PNG\u{2026}").on_hover_text("Ctrl+E").clicked() {
                pick_export(app);
            }

            ui.separator();
            preset_menu(app, ui, rs);
            ui.separator();

            ui.checkbox(&mut app.animate, "Animate")
                .on_hover_text("Run the preview continuously so tape noise, jitter and \
                                interlacing actually move.");
            if ui.checkbox(&mut app.compare, "Compare").on_hover_text(
                "Full pipeline left of the line, untouched source right.").changed() {
                app.mark_dirty();
            }

            ui.separator();
            ui.label("Zoom");
            if ui.add(egui::Slider::new(&mut app.zoom, 0.25..=4.0).show_value(false)).changed() {
                app.mark_dirty();
            }
            ui.label(format!("{:.0}%", app.zoom * 100.0));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                let (cw, ch) = app.chain_input_size();
                ui.label(
                    egui::RichText::new(format!("chain input {cw}\u{00D7}{ch}"))
                        .monospace()
                        .weak(),
                );
            });
        });
        ui.add_space(2.0);
    });

    // Keyboard shortcuts, matching the macOS Command-key equivalents. Space
    // is play/pause, as it is in every player.
    let (open, export, play) = ctx.input(|i: &egui::InputState| {
        (
            i.modifiers.ctrl && i.key_pressed(egui::Key::O),
            i.modifiers.ctrl && i.key_pressed(egui::Key::E),
            i.key_pressed(egui::Key::Space),
        )
    });
    if open {
        pick_source(app);
    }
    if export {
        pick_export(app);
    }
    if play && app.video.is_some() {
        app.toggle_playback();
    }

    let _ = rs;
}

/// Preset menu: save/load the whole configuration, plus the bundled presets
/// listed underneath — the same arrangement as the macOS toolbar.
fn preset_menu(app: &mut NtscrtApp, ui: &mut egui::Ui, rs: Option<&RenderState>) {
    ui.menu_button("Preset", |ui| {
        if ui.button("Load\u{2026}").clicked() {
            ui.close();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("NTSCRT preset", &["json"])
                .set_directory(crate::presets::app_presets_dir().unwrap_or_else(|| ".".into()))
                .pick_file()
            {
                load_preset(app, &path, rs);
            }
        }
        if ui.button("Save as\u{2026}").clicked() {
            ui.close();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("NTSCRT preset", &["json"])
                .set_file_name("My preset.json")
                .save_file()
            {
                match app.to_preset().save(&path) {
                    Ok(()) => {
                        app.status = Some(format!("Saved preset {}", path.display()));
                        app.error = None;
                    }
                    Err(e) => app.error = Some(format!("Could not save preset: {e}")),
                }
            }
        }

        let bundled = crate::app_preset::bundled();
        if !bundled.is_empty() {
            ui.separator();
            for (name, path) in bundled {
                if ui.button(&name).clicked() {
                    ui.close();
                    load_preset(app, &path, rs);
                }
            }
        }
    });
}

fn load_preset(app: &mut NtscrtApp, path: &std::path::Path, rs: Option<&RenderState>) {
    let Some(rs) = rs else {
        app.error = Some("No GPU context; cannot switch shaders".into());
        return;
    };
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    match crate::app_preset::AppPreset::load(path) {
        Ok(preset) => match app.apply_preset(preset, &rs.device, &rs.queue) {
            // Anything the preset asked for that couldn't be applied is
            // surfaced rather than silently dropped.
            Some(note) => {
                app.status = Some(format!("Loaded '{name}' \u{2014} {note}"));
                app.error = None;
            }
            None => {
                app.status = Some(format!("Loaded preset '{name}'"));
                app.error = None;
            }
        },
        Err(e) => app.error = Some(format!("Could not load '{name}': {e}")),
    }
}

fn pick_source(app: &mut NtscrtApp) {
    // One "everything" filter first, so Open finds either kind without the
    // user having to know which list a file is in.
    let everything: Vec<&str> = crate::image_io::SourceImage::EXTENSIONS
        .iter()
        .chain(crate::video::VIDEO_EXTENSIONS.iter())
        .copied()
        .collect();
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("Images & video", &everything)
        .add_filter("Images", crate::image_io::SourceImage::EXTENSIONS)
        .add_filter("Video", crate::video::VIDEO_EXTENSIONS)
        .pick_file()
    {
        app.load_source(path);
    }
}

fn pick_export(app: &mut NtscrtApp) {
    let suggested = app
        .source_path
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| format!("{}-ntscrt.png", s.to_string_lossy())))
        .unwrap_or_else(|| "ntscrt.png".to_string());
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("PNG", &["png"])
        .set_file_name(suggested)
        .save_file()
    {
        app.export_png(path);
    }
}

pub fn sidebar(app: &mut NtscrtApp, root: &mut egui::Ui, rs: Option<&RenderState>) {
    egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(320.0)
        .size_range(260.0..=520.0)
        .show(root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                source_panel(app, ui);
                ui.separator();
                downscale_panel::show(app, ui);
                ui.separator();
                ntsc_panel::show(app, ui);
                ui.separator();
                shader_panel::show(app, ui, rs);
                ui.separator();
                export_panel(app, ui);
                ui.add_space(8.0);
            });
        });
}

fn source_panel(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Source")
        .default_open(true)
        .show(ui, |ui| {
            match &app.source_path {
                Some(p) => {
                    ui.label(
                        egui::RichText::new(
                            p.file_name().unwrap_or_default().to_string_lossy().to_string(),
                        )
                        .strong(),
                    );
                }
                None => {
                    ui.label(egui::RichText::new("No file loaded").weak());
                }
            }
            ui.label(
                egui::RichText::new(format!("{}\u{00D7}{}", app.source.width, app.source.height))
                    .monospace()
                    .weak(),
            );
            if ui.button("Open image\u{2026}").clicked() {
                pick_source(app);
            }
            ui.label(
                egui::RichText::new("Drag & drop a file onto the window works too.")
                    .small()
                    .weak(),
            );
        });
}

fn export_panel(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Export")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut app.export_height).range(16..=8192).speed(8));
            });

            let (cw, ch) = app.chain_input_size();
            let (out_w, out_h) = if app.snap_to_scanline_grid {
                ntscrt_core::ScanlineGrid::snapped_size(cw, ch, app.export_height)
            } else {
                let w = (app.export_height as f64 * cw as f64 / ch.max(1) as f64).round() as u32;
                (w.max(1), app.export_height.max(1))
            };
            ui.label(
                egui::RichText::new(format!("\u{2192} {out_w}\u{00D7}{out_h}"))
                    .monospace()
                    .weak(),
            );

            ui.checkbox(&mut app.snap_to_scanline_grid, "Snap size to scanline grid")
                .on_hover_text(
                    "Rounds the output so every source line gets the same whole number of \
                     rows — crispest scanlines, but it changes your dimensions. Without it, \
                     exports render at a whole multiple and average down instead.",
                );

            let rows_per_line = out_h as f32 / ch.max(1) as f32;
            if rows_per_line < 3.0 {
                ui.label(
                    egui::RichText::new(format!(
                        "Only {rows_per_line:.1} output rows per source line — crisp scanlines \
                         want 3+. Try a taller export."
                    ))
                    .small()
                    .color(egui::Color32::from_rgb(220, 170, 80)),
                );
            }

            if ui.button("Export PNG\u{2026}").clicked() {
                pick_export(app);
            }
        });
}

pub fn status_bar(app: &mut NtscrtApp, root: &mut egui::Ui) {
    egui::Panel::bottom("status").show(root, |ui| {
        ui.horizontal(|ui| {
            if let Some(err) = &app.error {
                ui.colored_label(egui::Color32::from_rgb(230, 100, 100), err);
            } else if let Some(status) = &app.status {
                ui.label(egui::RichText::new(status).weak());
            } else {
                ui.label(egui::RichText::new("Ready").weak());
            }
        });
    });
}

pub fn preview(app: &mut NtscrtApp, root: &mut egui::Ui, rs: Option<&RenderState>) {
    preview_panel::show(app, root, rs);
}
