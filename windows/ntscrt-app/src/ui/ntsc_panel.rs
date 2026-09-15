//! NTSC (TV) panel, ported from `Views/NtscPanel.swift`.
//!
//! The analog signal stage: composite noise, chroma bleed, head switching,
//! tracking noise, tape speed, edge wave, and about sixty more. These are
//! ntsc-rs's own settings, and the controls are generated from ntsc-rs's
//! settings schema rather than hand-written, so the panel tracks the library.
//!
//! Preset JSON copy/pastes both ways with the ntsc-rs desktop app.

use eframe::egui;
use ntscrt_core::settings_ui::{
    descriptors, AnySetting, NtscEffectFullSettings, SettingDescriptor, SettingKind,
};

use crate::app::NtscrtApp;

pub fn show(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("NTSC (TV)")
        .default_open(true)
        .show(ui, |ui| {
            if ui
                .checkbox(&mut app.ntsc_enabled, "Apply NTSC/VHS signal stage")
                .changed()
            {
                app.mark_chain_input_edited();
            }

            ui.horizontal(|ui| {
                if ui.button("Copy preset").on_hover_text(
                    "ntsc-rs preset JSON — paste into the ntsc-rs desktop app.").clicked() {
                    match app.ntsc.settings_json() {
                        Ok(json) => {
                            ui.ctx().copy_text(json);
                            app.status = Some("Copied ntsc-rs preset JSON".to_string());
                        }
                        Err(e) => app.error = Some(format!("{e}")),
                    }
                }
                if ui.button("Paste preset").clicked() {
                    // egui cannot read the clipboard directly; load from a
                    // file instead, which also covers the bundled presets.
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ntsc-rs preset", &["json"])
                        .set_directory(
                            crate::presets::app_presets_dir().unwrap_or_else(|| ".".into()),
                        )
                        .pick_file()
                    {
                        match std::fs::read_to_string(&path)
                            .map_err(|e| format!("{e}"))
                            .and_then(|j| app.ntsc.set_settings_json(&j).map_err(|e| format!("{e}")))
                        {
                            Ok(()) => {
                                app.status = Some(format!(
                                    "Loaded preset {}",
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                ));
                                app.mark_chain_input_edited();
                            }
                            Err(e) => app.error = Some(format!("Preset failed to load: {e}")),
                        }
                    }
                }
            });

            ui.add_enabled_ui(app.ntsc_enabled, |ui| {
                ui.separator();
                let descs = descriptors();
                for d in &descs {
                    setting_widget(app, ui, d);
                }
            });
        });
}

/// One control per descriptor; groups recurse.
fn setting_widget(
    app: &mut NtscrtApp,
    ui: &mut egui::Ui,
    desc: &SettingDescriptor<NtscEffectFullSettings>,
) {
    let hover = desc.description.unwrap_or("");

    match &desc.kind {
        SettingKind::Group { children } => {
            // ntsc-rs groups carry their own enable/disable boolean.
            let enabled = matches!(app.ntsc.get_any(&desc.id), AnySetting::Bool(true));
            egui::CollapsingHeader::new(desc.label)
                .id_salt(desc.id.name)
                .default_open(false)
                .show(ui, |ui| {
                    let mut on = enabled;
                    if ui.checkbox(&mut on, "Enabled").on_hover_text(hover).changed() {
                        set(app, desc, AnySetting::Bool(on));
                    }
                    ui.add_enabled_ui(on, |ui| {
                        for c in children {
                            setting_widget(app, ui, c);
                        }
                    });
                });
        }

        SettingKind::Boolean => {
            let AnySetting::Bool(mut v) = app.ntsc.get_any(&desc.id) else { return };
            if ui.checkbox(&mut v, desc.label).on_hover_text(hover).changed() {
                set(app, desc, AnySetting::Bool(v));
            }
        }

        SettingKind::Percentage { logarithmic } => {
            let AnySetting::Float(mut v) = app.ntsc.get_any(&desc.id) else { return };
            if super::labelled_slider(ui, desc.label, &mut v, 0.0..=1.0, None, *logarithmic, hover)
                .changed()
            {
                set(app, desc, AnySetting::Float(v));
            }
        }

        SettingKind::FloatRange { range, logarithmic } => {
            let AnySetting::Float(mut v) = app.ntsc.get_any(&desc.id) else { return };
            let range = range.start().to_owned()..=range.end().to_owned();
            if super::labelled_slider(ui, desc.label, &mut v, range, None, *logarithmic, hover)
                .changed()
            {
                set(app, desc, AnySetting::Float(v));
            }
        }

        SettingKind::IntRange { range } => {
            let mut v = match app.ntsc.get_any(&desc.id) {
                AnySetting::Int(i) => i,
                AnySetting::Enum(e) => e as i32,
                _ => return,
            };
            let range = range.start().to_owned()..=range.end().to_owned();
            if super::labelled_slider(ui, desc.label, &mut v, range, Some(1.0), false, hover)
                .changed()
            {
                set(app, desc, AnySetting::Int(v));
            }
        }

        SettingKind::Enumeration { options } => {
            let current = match app.ntsc.get_any(&desc.id) {
                AnySetting::Enum(e) => e,
                AnySetting::Int(i) => i as u32,
                _ => return,
            };
            let selected = options
                .iter()
                .find(|o| o.index == current)
                .map(|o| o.label)
                .unwrap_or("\u{2014}");
            ui.horizontal(|ui| {
                ui.label(desc.label).on_hover_text(hover);
                egui::ComboBox::from_id_salt(desc.id.name)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for o in options {
                            let mut resp = ui.selectable_label(o.index == current, o.label);
                            if let Some(d) = o.description {
                                resp = resp.on_hover_text(d);
                            }
                            if resp.clicked() {
                                set(app, desc, AnySetting::Enum(o.index));
                            }
                        }
                    });
            });
        }
    }
}

fn set(
    app: &mut NtscrtApp,
    desc: &SettingDescriptor<NtscEffectFullSettings>,
    value: AnySetting,
) {
    if let Err(e) = app.ntsc.set_any(&desc.id, value) {
        app.error = Some(format!("{}: {e}", desc.label));
    } else {
        app.mark_chain_input_edited();
    }
}
