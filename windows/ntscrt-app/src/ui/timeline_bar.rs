//! Keyframe timeline, ported from `Sources/CrtApp/Views/TimelineBar.swift`.
//!
//! Docked under the preview when Animate is on: a time ruler, a track of
//! master keyframes, and an easing dropdown under each one. Keyframe times
//! are normalised, so the duration field rescales the whole animation
//! proportionally rather than stranding keys past the end.

use eframe::egui;
use ntscrt_core::Easing;

use crate::app::NtscrtApp;

const RULER_H: f32 = 22.0;
const TRACK_H: f32 = 56.0;
const DIAMOND: f32 = 11.0;

/// Deferred edits. The track is drawn from a borrow of the keyframes, so
/// nothing may mutate the app until that borrow is done.
enum Action {
    Scrub(f64),
    Select(usize),
    Move(usize, f64),
    Easing(usize, Easing),
}

pub fn show(app: &mut NtscrtApp, root: &mut egui::Ui) {
    if !app.timeline_open {
        return;
    }
    egui::Panel::bottom("timeline").show(root, |ui| {
        ui.add_space(6.0);
        controls(app, ui);
        ui.add_space(6.0);
        track(app, ui);
        ui.add_space(6.0);
    });
}

fn controls(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("ANIMATE").small().strong().weak());

        let playing = app.timeline_playing || app.is_playing();
        let icon = if playing { "\u{23F8}" } else { "\u{25B6}" };
        if ui
            .add_sized([26.0, 22.0], egui::Button::new(icon))
            .on_hover_text(if playing {
                "Pause"
            } else {
                "Preview the keyframe animation in the preview (loops)"
            })
            .clicked()
        {
            app.toggle_timeline_preview();
        }

        if ui
            .button("Keyframe")
            .on_hover_text(
                "Snapshot every NTSC and shader parameter at the playhead. Updates the \
                 keyframe under the playhead if there is one. Nothing is keyed until you \
                 press this.",
            )
            .clicked()
        {
            app.set_keyframe_at_playhead();
        }

        // Delete only offers itself when the playhead is parked on a key.
        if let Some(i) = app.keyframe_at_playhead() {
            if ui.button("Delete").on_hover_text("Delete the keyframe under the playhead").clicked() {
                app.delete_keyframe(i);
            }
        }
        if !app.timeline_keys().is_empty()
            && ui.button("Clear").on_hover_text("Remove every keyframe").clicked()
        {
            app.clear_keyframes();
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let (duration, fps) = app.effective_timeline();

            if app.video.is_some() {
                // A clip brings its own length and rate.
                ui.label(
                    egui::RichText::new(format!("{duration:.2} s  \u{00B7}  {fps:.0} fps"))
                        .monospace()
                        .weak(),
                )
                .on_hover_text(
                    "Length and frame rate come from the clip. Keyframes are positioned \
                     proportionally along it.",
                );
            } else {
                egui::ComboBox::from_id_salt("tl_fps")
                    .selected_text(format!("{fps:.0} fps"))
                    .width(78.0)
                    .show_ui(ui, |ui| {
                        for r in [12.0, 24.0, 30.0, 60.0] {
                            if ui.selectable_label(fps == r, format!("{r:.0} fps")).clicked() {
                                app.timeline_mut().fps = r;
                            }
                        }
                    });
                let mut d = duration;
                if ui
                    .add(egui::DragValue::new(&mut d).range(0.5..=600.0).speed(0.1).suffix(" s"))
                    .on_hover_text(
                        "Video length. Keyframes are proportional, so changing this stretches \
                         the whole animation.",
                    )
                    .changed()
                {
                    app.timeline_mut().duration = d;
                }
                ui.label(egui::RichText::new("Duration").small().weak());
            }

            ui.separator();
            ui.label(
                egui::RichText::new(format!("{:.2} s", app.playhead * duration))
                    .monospace()
                    .weak(),
            );
        });
    });
}

fn track(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    let width = ui.available_width() - 16.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, RULER_H + TRACK_H),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().faint_bg_color);

    let x_of = |t: f64| rect.left() + (t as f32) * rect.width();
    let t_of = |x: f32| ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
    let track_y = rect.top() + RULER_H + TRACK_H * 0.5;

    let (duration, _) = app.effective_timeline();
    let mut actions: Vec<Action> = Vec::new();

    // Ruler: a tick every half second, labelled every second.
    let seconds = duration.max(0.001);
    let step = if seconds > 20.0 { 5.0 } else if seconds > 8.0 { 1.0 } else { 0.5 };
    let mut s = 0.0;
    while s <= seconds + 1e-9 {
        let x = x_of(s / seconds);
        let tall = (s / step).round() as i64 % 2 == 0 || step >= 1.0;
        painter.line_segment(
            [
                egui::pos2(x, rect.top() + if tall { 4.0 } else { 9.0 }),
                egui::pos2(x, rect.top() + RULER_H - 4.0),
            ],
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
        if tall {
            painter.text(
                egui::pos2(x + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                format!("{s:.0}s"),
                egui::FontId::monospace(9.0),
                ui.visuals().weak_text_color(),
            );
        }
        s += step;
    }

    // Track line.
    painter.line_segment(
        [egui::pos2(rect.left(), track_y), egui::pos2(rect.right(), track_y)],
        egui::Stroke::new(3.0, ui.visuals().widgets.inactive.bg_fill),
    );

    let keys: Vec<(f64, Easing)> =
        app.timeline_keys().iter().map(|k| (k.t, k.easing)).collect();

    if keys.is_empty() {
        painter.text(
            egui::pos2(rect.center().x, track_y + 14.0),
            egui::Align2::CENTER_TOP,
            "Dial in a look, then press Keyframe to set one",
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
    }

    // Keyframe diamonds.
    let parked = app.keyframe_at_playhead();
    for (i, (t, _)) in keys.iter().enumerate() {
        let c = egui::pos2(x_of(*t), track_y);
        let selected = parked == Some(i);
        let colour = if selected {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.active.fg_stroke.color
        };
        let d = DIAMOND * if selected { 1.15 } else { 1.0 };
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(c.x, c.y - d),
                egui::pos2(c.x + d * 0.7, c.y),
                egui::pos2(c.x, c.y + d),
                egui::pos2(c.x - d * 0.7, c.y),
            ],
            colour,
            egui::Stroke::new(1.0, ui.visuals().extreme_bg_color),
        ));

        // Each diamond gets its own drag handle, so dragging one retimes it
        // rather than scrubbing the playhead underneath.
        let hit = egui::Rect::from_center_size(c, egui::vec2(d * 2.2, d * 2.2));
        let r = ui.interact(hit, ui.id().with(("kf", i)), egui::Sense::click_and_drag());
        if r.dragged() {
            if let Some(p) = r.interact_pointer_pos() {
                actions.push(Action::Move(i, t_of(p.x)));
            }
        } else if r.clicked() {
            actions.push(Action::Select(i));
        }
    }

    // Playhead.
    let px = x_of(app.playhead);
    painter.line_segment(
        [egui::pos2(px, rect.top()), egui::pos2(px, rect.top() + RULER_H + TRACK_H)],
        egui::Stroke::new(1.5, ui.visuals().warn_fg_color),
    );

    // Scrub anywhere on the band that isn't a diamond.
    if response.dragged() || response.clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            actions.push(Action::Scrub(t_of(p.x)));
        }
    }

    // Easing chips, one under each keyframe.
    if !keys.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(8.0);
            for (i, (t, easing)) in keys.iter().enumerate() {
                let mut chosen = *easing;
                egui::ComboBox::from_id_salt(("ease", i))
                    .selected_text(format!("{}: {}", i + 1, easing.display_name()))
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for e in Easing::ALL {
                            if ui.selectable_label(*easing == e, e.display_name()).clicked() {
                                chosen = e;
                            }
                        }
                    });
                if chosen != *easing {
                    actions.push(Action::Easing(i, chosen));
                }
                let _ = t;
            }
        });
    }

    for a in actions {
        match a {
            Action::Scrub(t) => app.scrub_timeline(t),
            Action::Select(i) => {
                let t = app.timeline_keys().get(i).map(|k| k.t);
                if let Some(t) = t {
                    app.scrub_timeline(t);
                }
            }
            Action::Move(i, t) => app.move_keyframe(i, t),
            Action::Easing(i, e) => app.set_keyframe_easing(i, e),
        }
    }
}
