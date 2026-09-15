//! The window: toolbar, sidebar panels, preview.
//!
//! Ported from `Sources/CrtApp/Views/`. The macOS build puts file actions in
//! the window toolbar and the creative pipeline in a sidebar, top to bottom
//! in signal order — that layout is kept here, with egui's idioms standing in
//! for SwiftUI's.

mod about_window;
mod downscale_panel;
mod grade_panel;
mod ntsc_panel;
mod preview_panel;
mod shader_panel;
mod timeline_bar;
mod transport_bar;

use eframe::egui;

use crate::app::{VhsStudioApp, RenderState};

/// A parameter slider laid out as the macOS panels do it: the label and an
/// editable value on one row, the slider full-width beneath. egui's own
/// slider puts the label after the value on the same row, and the longer
/// CRT parameter names ("Mask - Number of Triads Desired") then ran off the
/// edge of the sidebar. The returned response is `changed()` if either the
/// slider or the value field moved.
pub(super) fn labelled_slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: Option<f64>,
    logarithmic: bool,
    hover: &str,
) -> egui::Response {
    let (lo, hi) = (range.start().to_f64(), range.end().to_f64());
    // A step declared for the parameter is also the natural drag increment;
    // otherwise a drag across the field's width covers about the range.
    let speed = step.filter(|s| *s > 0.0).unwrap_or((hi - lo).abs() / 200.0).max(1e-6);

    ui.vertical(|ui| {
        let (_, field) = label_row(ui, label, hover, |ui| {
            let mut field = egui::DragValue::new(value).range(range.clone()).speed(speed);
            // Integers keep DragValue's own zero-decimal formatting.
            if !N::INTEGRAL {
                field = field.max_decimals(4);
            }
            ui.add(field)
        });

        ui.spacing_mut().slider_width = ui.available_width();
        let mut slider = egui::Slider::new(value, range)
            .show_value(false)
            .clamping(egui::SliderClamping::Edits);
        if let Some(s) = step.filter(|s| *s > 0.0 && s.is_finite()) {
            slider = slider.step_by(s);
        }
        if logarithmic {
            slider = slider.logarithmic(true);
        }
        let slider = ui.add(slider);
        let slider = if hover.is_empty() { slider } else { slider.on_hover_text(hover) };
        slider | field
    })
    .inner
}

/// A row with a left-aligned, truncating label and right-aligned controls.
///
/// The controls are laid out first, so a long label truncates to the space
/// they leave rather than running underneath them — laying the label out
/// first would have it measure against the whole row. Returns the label's
/// response and whatever `controls` returned.
pub(super) fn label_row<R>(
    ui: &mut egui::Ui,
    label: &str,
    hover: &str,
    controls: impl FnOnce(&mut egui::Ui) -> R,
) -> (egui::Response, R) {
    // `horizontal` sizes the row to one control's height; the nested layout
    // then fills it from the right.
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = controls(ui);
            let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
            let label = ui
                .allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let l = ui.add(egui::Label::new(label).truncate());
                    if hover.is_empty() { l } else { l.on_hover_text(hover) }
                })
                .inner;
            (label, r)
        })
        .inner
    })
    .inner
}

pub fn top_bar(app: &mut VhsStudioApp, root: &mut egui::Ui, rs: Option<&RenderState>) {
    let ctx = root.ctx().clone();
    egui::Panel::top("toolbar").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            open_control(app, ui);
            export_control(app, ui);

            ui.separator();
            preset_menu(app, ui, rs);
            ui.separator();

            if ui
                .add_sized([26.0, 22.0], egui::Button::new("\u{27F3}"))
                .on_hover_text(
                    "Rotate 90\u{00B0} right (Ctrl+R). Applied before the effect, so \
                     scanlines stay horizontal.",
                )
                .clicked()
            {
                app.rotate_cw();
            }
            if app.rotation != vhs_studio_core::Rotation::None {
                ui.label(egui::RichText::new(app.rotation.display_name()).small().weak());
            }
            ui.separator();

            if ui
                .selectable_label(app.timeline_open, "Timeline")
                .on_hover_text(
                    "Keyframe-animate the whole effect chain and render it as video.",
                )
                .clicked()
            {
                app.timeline_open = !app.timeline_open;
            }
            ui.checkbox(&mut app.animate, "Animate")
                .on_hover_text("Run the preview continuously so tape noise, jitter and \
                                interlacing actually move.");
            if ui.checkbox(&mut app.compare, "Compare").on_hover_text(
                "Full pipeline left of the line, untouched source right. Drag the line \
                 to move the split.").changed() {
                app.mark_dirty();
            }
            if ui
                .checkbox(&mut app.integer_scale, "Integer scale")
                .on_hover_text(
                    "Lock the preview to whole-pixel multiples of the downscale, so every \
                     scanline is the same height on screen (letterboxed).",
                )
                .changed()
            {
                app.mark_dirty();
            }

            ui.separator();
            ui.label("Zoom");
            if ui
                .add(
                    egui::Slider::new(
                        &mut app.zoom,
                        preview_panel::MIN_ZOOM..=preview_panel::MAX_ZOOM,
                    )
                    .logarithmic(true)
                    .show_value(false),
                )
                .on_hover_text(
                    "Magnify the preview (or Alt+scroll over it). Hold Space and drag to \
                     pan; double-click the preview to reset.",
                )
                .changed()
            {
                app.mark_dirty();
            }
            if ui
                .add(egui::Button::new(format!("{:.0}%", app.zoom * 100.0)).frame(false))
                .on_hover_text("Reset zoom")
                .clicked()
            {
                app.zoom = 1.0;
                app.pan = egui::Vec2::ZERO;
                app.mark_dirty();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if ui
                    .button("About")
                    .on_hover_text(format!("Version {} (F1)", crate::about::BUILD.short()))
                    .clicked()
                {
                    app.about_open = !app.about_open;
                }
                ui.separator();
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

    about_window::show(app, &ctx);
    if ctx.input(|i: &egui::InputState| i.key_pressed(egui::Key::F1)) {
        app.about_open = !app.about_open;
    }

    // Keyboard shortcuts, matching the macOS Command-key equivalents. Space
    // is play/pause, as it is in every player.
    if ctx.input(|i: &egui::InputState| i.modifiers.ctrl && i.key_pressed(egui::Key::R)) {
        app.rotate_cw();
    }
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
    // Not while a text field has focus: a space typed there is a space.
    let typing = ctx.memory(|m| m.focused().is_some());
    if play && !typing && app.video.is_some() {
        app.toggle_playback();
    }

    let _ = rs;
}

/// The toolbar's Open button and, while a file is being opened on the
/// loader thread, a spinner with the file's name beside it. Open stays
/// enabled: picking another file simply replaces the one in flight.
fn open_control(app: &mut VhsStudioApp, ui: &mut egui::Ui) {
    if ui.button("Open\u{2026}").on_hover_text("Ctrl+O").clicked() {
        pick_source(app);
    }
    if let Some(task) = &app.load_task {
        let name = task.file_name();
        let seconds = task.started.elapsed().as_secs_f32();
        ui.add(egui::Spinner::new().size(16.0));
        ui.label(egui::RichText::new(format!("Opening {name}\u{2026}")).small())
            .on_hover_text(format!("{} \u{2014} {seconds:.0} s so far", task.path.display()));
    }
}

/// The toolbar's Export button, which says what it will write, and — while
/// an export runs — turns into its progress and a Cancel, so the export
/// is visible with every panel closed, as the macOS toolbar does.
fn export_control(app: &mut VhsStudioApp, ui: &mut egui::Ui) {
    if let Some(task) = &app.export_task {
        let (done, total) = task.counts();
        let cancelling = task.is_cancelling();
        let text = if cancelling {
            "Cancelling\u{2026}".to_string()
        } else if total > 1 {
            format!("Exporting {}%", (task.fraction() * 100.0).round() as u32)
        } else {
            "Exporting\u{2026}".to_string()
        };
        ui.add(
            egui::ProgressBar::new(task.fraction())
                .desired_width(150.0)
                .desired_height(20.0)
                .text(egui::RichText::new(text).small())
                .animate(!cancelling),
        )
        .on_hover_text(format!("{done} / {total} frames"));
        ui.add_enabled_ui(!cancelling, |ui| {
            if ui.button("Cancel").on_hover_text("Stop the export and remove the partial file").clicked() {
                app.cancel_export();
            }
        });
        return;
    }

    let label = if app.exports_video() {
        format!("Export {}\u{2026}", app.export_job.format.button_name())
    } else {
        "Export PNG\u{2026}".to_string()
    };
    if ui.button(label).on_hover_text("Ctrl+E").clicked() {
        pick_export(app);
    }
}

/// Preset menu: save/load the whole configuration, then the bundled
/// presets in three sections. **Looks** and **Animated** are whole
/// snapshots — one at a time, the tick marks the one on screen. **Colour**
/// presets are grade-only and stack on top of either: their tick is a
/// checkbox, so picking one lays it over the current look, picking it again
/// takes it off, and switching the look underneath keeps it.
fn preset_menu(app: &mut VhsStudioApp, ui: &mut egui::Ui, rs: Option<&RenderState>) {
    ui.menu_button("Preset", |ui| {
        if ui.button("Load\u{2026}").clicked() {
            ui.close();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("VHS-Studio preset", &["json"])
                .set_directory(crate::presets::app_presets_dir().unwrap_or_else(|| ".".into()))
                .pick_file()
            {
                load_preset(app, &path, rs);
            }
        }
        if ui.button("Save as\u{2026}").clicked() {
            ui.close();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("VHS-Studio preset", &["json"])
                .set_file_name("My preset.json")
                .save_file()
            {
                save_preset(app, app.to_preset(), &path);
            }
        }
        if ui
            .button("Save colour as\u{2026}")
            .on_hover_text(
                "Save just the Colour panel as a stackable colour preset. The file also \
                 carries the rest of the current look, so older builds load it whole.",
            )
            .clicked()
        {
            ui.close();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("VHS-Studio preset", &["json"])
                .set_file_name("My colour.json")
                .save_file()
            {
                let mut preset = app.to_preset();
                preset.layer = Some(crate::app_preset::PresetLayer::Colour);
                save_preset(app, preset, &path);
            }
        }

        use crate::app_preset::PresetKind;
        let bundled = app.bundled_presets.clone();
        let mut section = |ui: &mut egui::Ui, kind: PresetKind, title: &str, hint: &str| {
            let items: Vec<_> = bundled.iter().filter(|b| b.kind == kind).collect();
            if items.is_empty() {
                return;
            }
            ui.separator();
            ui.label(egui::RichText::new(title).small().weak()).on_hover_text(hint);
            for b in items {
                // A tick marks the loaded preset. It clears the moment any
                // setting the preset controls is edited, so it never claims
                // a look you have since changed.
                //
                // Looks are a radio group — one on screen at a time. Colour
                // presets are checkboxes: ticked means stacked on top.
                let clicked = if kind == PresetKind::Colour {
                    let mut on = app.active_colour_preset() == Some(b.name.as_str());
                    ui.checkbox(&mut on, &b.name).clicked()
                } else {
                    let on = app.active_preset.as_deref() == Some(b.name.as_str());
                    ui.radio(on, &b.name).clicked()
                };
                if clicked {
                    ui.close();
                    if kind == PresetKind::Colour && app.active_colour_preset() == Some(b.name.as_str()) {
                        app.remove_colour_layer();
                        app.status = Some(format!("Removed colour '{}'", b.name));
                        app.error = None;
                    } else {
                        load_preset(app, &b.path, rs);
                    }
                }
            }
        };
        section(ui, PresetKind::Look, "Looks", "A whole look. Loading one replaces the look on screen.");
        section(
            ui,
            PresetKind::Animated,
            "Animated",
            "A whole look with keyframes. Loading one replaces the look on screen and opens the timeline.",
        );
        section(
            ui,
            PresetKind::Colour,
            "Colour \u{2014} stacks on any look",
            "Only the Colour panel. Pick one to lay it over the current look; pick it again to take it off. \
             Switching the look underneath keeps it.",
        );
    });
}

fn save_preset(app: &mut VhsStudioApp, preset: crate::app_preset::AppPreset, path: &std::path::Path) {
    match preset.save(path) {
        Ok(()) => {
            app.status = Some(format!("Saved preset {}", path.display()));
            app.error = None;
        }
        Err(e) => app.error = Some(format!("Could not save preset: {e}")),
    }
}

fn load_preset(app: &mut VhsStudioApp, path: &std::path::Path, rs: Option<&RenderState>) {
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let preset = match crate::app_preset::AppPreset::load(path) {
        Ok(p) => p,
        Err(e) => {
            app.error = Some(format!("Could not load '{name}': {e}"));
            return;
        }
    };

    // A colour preset touches only the grade, so it needs no GPU context
    // and leaves the base look's tick alone.
    if preset.is_colour_layer() {
        app.apply_colour_preset(&name, &preset);
        app.status = Some(match &app.active_preset {
            Some(base) => format!("Colour '{name}' over '{base}'"),
            None => format!("Colour '{name}' applied"),
        });
        app.error = None;
        return;
    }

    let Some(rs) = rs else {
        app.error = Some("No GPU context; cannot switch shaders".into());
        return;
    };
    let note = app.apply_preset(preset, &rs.device, &rs.queue);
    // Set after applying: apply_preset routes through the same edit
    // hooks the panels use, which clear the tick.
    app.active_preset = Some(name.clone());
    let colour = app.active_colour_preset().map(|c| format!(" with colour '{c}'")).unwrap_or_default();
    match note {
        // Anything the preset asked for that couldn't be applied is
        // surfaced rather than silently dropped.
        Some(note) => {
            app.status = Some(format!("Loaded '{name}'{colour} \u{2014} {note}"));
            app.error = None;
        }
        None => {
            app.status = Some(format!("Loaded preset '{name}'{colour}"));
            app.error = None;
        }
    }
}

fn pick_source(app: &mut VhsStudioApp) {
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

fn pick_export(app: &mut VhsStudioApp) {
    // The picker's filter and default extension follow whichever export the
    // current source and settings imply, so the saved file always matches
    // what the button said it would write.
    let video = app.exports_video();
    let ext = if video { app.export_job.format.file_extension() } else { "png" };
    let filter_name = if video { app.export_job.format.display_name() } else { "PNG" };

    let stem = app
        .source_path
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "vhs-studio".to_string());

    if let Some(path) = rfd::FileDialog::new()
        .add_filter(filter_name, &[ext])
        .set_file_name(format!("{stem}-vhs-studio.{ext}"))
        .save_file()
    {
        app.export_from_ui(path);
    }
}

pub fn sidebar(app: &mut VhsStudioApp, root: &mut egui::Ui, rs: Option<&RenderState>) {
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
                grade_panel::show(app, ui);
                ui.separator();
                shader_panel::show(app, ui, rs);
                ui.separator();
                export_panel(app, ui);
                ui.add_space(8.0);
            });
        });
}

fn source_panel(app: &mut VhsStudioApp, ui: &mut egui::Ui) {
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
            ui.horizontal(|ui| {
                ui.label("Rotate");
                let mut chosen = app.rotation;
                egui::ComboBox::from_id_salt("rotation")
                    .selected_text(app.rotation.display_name())
                    .show_ui(ui, |ui| {
                        for r in vhs_studio_core::Rotation::ALL {
                            if ui.selectable_label(app.rotation == r, r.display_name()).clicked() {
                                chosen = r;
                            }
                        }
                    });
                app.set_rotation(chosen);
            });

            ui.horizontal(|ui| {
                if ui.button("Open\u{2026}").clicked() {
                    pick_source(app);
                }
                if let Some(task) = &app.load_task {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(egui::RichText::new(format!("Opening {}\u{2026}", task.file_name())).small());
                }
            });
            ui.label(
                egui::RichText::new("Drag & drop a file onto the window works too.")
                    .small()
                    .weak(),
            );
        });
}

fn export_panel(app: &mut VhsStudioApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Export")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Long edge");
                ui.add(
                    egui::DragValue::new(&mut app.export_long_edge)
                        .range(64..=8192)
                        .speed(8)
                        .suffix(" px"),
                )
                .on_hover_text(
                    "Size of the output's longer side. The other side follows the \
                     source's aspect ratio.",
                );
            });

            let (_, ch) = app.chain_input_size();
            // The size the file will actually have — even dimensions for
            // movies, snapped when asked (see `export_output_size`).
            let (out_w, out_h) = app.export_output_size();
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

            // A running export owns this panel: the settings it is using
            // must not look editable while it runs.
            if let Some(task) = &app.export_task {
                let (done, total) = task.counts();
                let fraction = task.fraction();
                let cancelling = task.is_cancelling();

                ui.separator();
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .animate(!cancelling),
                );
                ui.label(
                    egui::RichText::new(if cancelling {
                        "Cancelling\u{2026}".to_string()
                    } else if total > 1 {
                        format!("{done} / {total} frames")
                    } else if total == 1 {
                        "Rendering the still\u{2026}".to_string()
                    } else {
                        "Starting\u{2026}".to_string()
                    })
                    .small()
                    .weak(),
                );
                ui.add_enabled_ui(!cancelling, |ui| {
                    if ui.button("Cancel").clicked() {
                        app.cancel_export();
                    }
                });
                return;
            }

            movie_controls(app, ui);

            let label = if app.exports_video() {
                format!("Export {}\u{2026}", app.export_job.format.button_name())
            } else {
                "Export PNG\u{2026}".to_string()
            };
            if ui.button(label).clicked() {
                pick_export(app);
            }
        });
}

/// Format, quality and the GIF-specific settings. Shown when the export will
/// be a movie: a loaded video always, or a still once it has been given a
/// frame count to animate for.
fn movie_controls(app: &mut VhsStudioApp, ui: &mut egui::Ui) {
    use crate::video::{ExportFormat, ExportQuality, GifSettings};

    ui.separator();

    // A still only becomes a movie when asked. Videos have no choice.
    if app.video.is_none() {
        let mut as_video = app.export_still_frames.is_some();
        if ui
            .checkbox(&mut as_video, "Export as video (VHS motion)")
            .on_hover_text(
                "Tape noise, jitter and interlacing animate on their own, so a still \
                 can export video even without a timeline.",
            )
            .changed()
        {
            app.export_still_frames = as_video.then_some(48);
        }
        if let Some(frames) = app.export_still_frames.as_mut() {
            ui.horizontal(|ui| {
                ui.label("Frames");
                ui.add(egui::DragValue::new(frames).range(1..=3600).speed(1));
                ui.label(
                    egui::RichText::new(format!("{:.1}s at 24 fps", *frames as f32 / 24.0)).weak(),
                );
            });
        }
    }

    if !app.exports_video() {
        return;
    }

    ui.horizontal(|ui| {
        ui.label("Format");
        egui::ComboBox::from_id_salt("export_format")
            .selected_text(app.export_job.format.display_name())
            .show_ui(ui, |ui| {
                for f in ExportFormat::ALL {
                    if ui
                        .selectable_label(app.export_job.format == f, f.display_name())
                        .clicked()
                    {
                        app.export_job.format = f;
                    }
                }
            });
    });

    if app.export_job.format.is_gif() {
        // GIF takes its own size and rate: 256 colours against full-frame
        // analog noise makes files far larger than the video codecs.
        ui.horizontal(|ui| {
            ui.label("GIF width");
            ui.add(egui::DragValue::new(&mut app.export_job.gif.width).range(64..=1920).speed(8));
        });
        ui.horizontal(|ui| {
            ui.label("GIF fps");
            egui::ComboBox::from_id_salt("gif_fps")
                .selected_text(format!("{}", app.export_job.gif.fps))
                .show_ui(ui, |ui| {
                    for r in GifSettings::RATES {
                        if ui
                            .selectable_label(app.export_job.gif.fps == r, format!("{r}"))
                            .clicked()
                        {
                            app.export_job.gif.fps = r;
                        }
                    }
                });
            ui.label(
                egui::RichText::new(format!(
                    "plays at {:.1}",
                    GifSettings::true_fps(app.export_job.gif.fps)
                ))
                .weak(),
            )
            .on_hover_text(
                "GIF stores delays in hundredths of a second, so the rates land on that grid.",
            );
        });

        // Estimate the file before writing it, as the macOS panel does.
        let (cw, ch) = app.chain_input_size();
        let w = app.export_job.gif.width;
        let h = (w as f32 * ch as f32 / cw.max(1) as f32).round() as u32;
        let frames = app
            .video
            .as_ref()
            .map(|v| {
                let secs = v.total_frames() as f32 / (v.frame_rate() as f32).max(1.0);
                (secs * app.export_job.gif.fps as f32).round() as u32
            })
            .or(app.export_still_frames)
            .unwrap_or(0);
        let bytes = GifSettings::estimated_bytes(w, h, frames);
        let mb = bytes as f64 / (1024.0 * 1024.0);
        let text = format!("~{mb:.1} MB estimated ({w}\u{00D7}{h}, {frames} frames)");
        if bytes > GifSettings::WARN_BYTES {
            ui.label(
                egui::RichText::new(format!("{text} \u{2014} past what most platforms accept"))
                    .small()
                    .color(egui::Color32::from_rgb(220, 170, 80)),
            );
        } else {
            ui.label(egui::RichText::new(text).small().weak());
        }
    } else {
        ui.horizontal(|ui| {
            ui.label("Quality");
            egui::ComboBox::from_id_salt("export_quality")
                .selected_text(app.export_job.quality.display_name())
                .show_ui(ui, |ui| {
                    for q in ExportQuality::ALL {
                        if ui
                            .selectable_label(app.export_job.quality == q, q.display_name())
                            .clicked()
                        {
                            app.export_job.quality = q;
                        }
                    }
                });
        });
        if matches!(app.export_job.quality, ExportQuality::Standard) {
            ui.label(
                egui::RichText::new(
                    "Scanline detail is brutal on lossy codecs \u{2014} use High or above, \
                     or ProRes when it's headed into an edit.",
                )
                .small()
                .weak(),
            );
        }

        ui.horizontal(|ui| {
            ui.label("Loop");
            ui.add(egui::DragValue::new(&mut app.export_job.loop_count).range(1..=20).speed(1))
                .on_hover_text(
                    "Repeat the content in the file, for places that don't loop video on \
                     playback. 3 turns a 6-second clip into an 18-second file.",
                );
        });
    }
}

pub fn status_bar(app: &mut VhsStudioApp, root: &mut egui::Ui) {
    egui::Panel::bottom("status").show(root, |ui| {
        ui.horizontal(|ui| {
            // The toolbar carries export progress; this line says what is
            // being written.
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

pub fn timeline(app: &mut VhsStudioApp, root: &mut egui::Ui) {
    timeline_bar::show(app, root);
}

pub fn preview(app: &mut VhsStudioApp, root: &mut egui::Ui, rs: Option<&RenderState>) {
    preview_panel::show(app, root, rs);
}

/// Video transport, docked between the status bar and the preview. Draws
/// nothing when the source is a still.
pub fn transport_bar(app: &mut VhsStudioApp, root: &mut egui::Ui) {
    transport_bar::show(app, root);
}
