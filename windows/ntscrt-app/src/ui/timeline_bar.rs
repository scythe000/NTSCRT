//! Keyframe timeline, ported from `Sources/CrtApp/Views/TimelineBar.swift`.
//!
//! Docked under the preview when Animate is on: a time ruler, a track of
//! master keyframes, and an easing dropdown centred under each one. Keyframe
//! times are normalised, so the duration field rescales the whole animation
//! proportionally rather than stranding keys past the end.
//!
//! The bar keeps a constant height for a given key layout: the easing-chip
//! row is reserved even with no keys, so setting the first keyframe does not
//! shift every control up — with the Keyframe button directly above the
//! scrub band, that jump turned a second press into a scrub.

use eframe::egui;
use ntscrt_core::Easing;

use crate::app::NtscrtApp;

const RULER_H: f32 = 24.0;
const TRACK_H: f32 = 64.0;
/// Space between the track band and the easing chips.
const CHIP_GAP: f32 = 8.0;
const CHIP_ROW_H: f32 = 26.0;
const CHIP_W: f32 = 104.0;
const DIAMOND: f32 = 11.0;
const PLAYHEAD_KNOB: f32 = 4.5;

/// Deferred edits. The track is drawn from a borrow of the keyframes, so
/// nothing may mutate the app until that borrow is done.
enum Action {
    Scrub(f64),
    Select(usize),
    /// `(index, t)`; the drag in progress is re-pointed at the key's new
    /// index once the move has been applied.
    Move(usize, f64),
    Easing(usize, Easing),
    Delete(usize),
}

pub fn show(app: &mut NtscrtApp, root: &mut egui::Ui) {
    if !app.timeline_open {
        return;
    }
    let ctx = root.ctx().clone();
    egui::Panel::bottom("timeline").show(root, |ui| {
        ui.add_space(6.0);
        controls(app, ui);
        ui.add_space(6.0);
        track(app, ui);
        ui.add_space(6.0);
    });
    keyboard(app, &ctx);
}

fn controls(app: &mut NtscrtApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("ANIMATE").small().strong().weak());

        let playing = app.timeline_playing || app.is_playing();
        let icon = if playing { "\u{23F8}" } else { "\u{25B6}" };
        // Nothing to preview until there is a key or a clip.
        let can_play = !app.timeline_keys().is_empty() || app.video.is_some();
        if ui
            .add_enabled(can_play, egui::Button::new(icon).min_size(egui::vec2(26.0, 22.0)))
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
            if ui
                .button("Delete")
                .on_hover_text("Delete the keyframe under the playhead (Delete key)")
                .clicked()
            {
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

            if let Some(video) = app.video.as_ref() {
                // A clip brings its own length and rate. The frame counter
                // is the transport bar's, which this bar stands in for.
                ui.label(
                    egui::RichText::new(format!(
                        "{}/{}  \u{00B7}  {duration:.2} s  \u{00B7}  {fps:.0} fps",
                        video.current_frame_index + 1,
                        video.total_frames()
                    ))
                    .monospace()
                    .weak(),
                )
                .on_hover_text(
                    "Frame, length and frame rate come from the clip. Keyframes are \
                     positioned proportionally along it.",
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
    let keys: Vec<(f64, Easing)> =
        app.timeline_keys().iter().map(|k| (k.t, k.easing)).collect();
    let key_xs: Vec<f32> = keys.iter().map(|(t, _)| (*t as f32) * width).collect();
    let rows = chip_rows(&key_xs, CHIP_W + 6.0);
    let row_count = rows.iter().copied().max().map_or(1, |r| r + 1);

    // The scrub band: ruler + track. Chips get their own space below it so
    // their dropdowns stay clickable and a click there never scrubs.
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

    ruler(ui, &painter, rect, duration);

    // Frames already in the RAM-preview cache, as a thin green line under
    // the ruler — the After Effects render bar. It fills in while the video
    // sits paused; playback inside green costs no NTSC work per frame.
    if let Some(video) = app.video.as_ref() {
        let strip = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + RULER_H - 3.0),
            egui::pos2(rect.right(), rect.top() + RULER_H - 1.0),
        );
        super::transport_bar::paint_cached_runs(
            &painter,
            strip,
            &video.cached_ranges,
            video.total_frames(),
            video.prerender_active,
        );
    }

    // Track line.
    painter.line_segment(
        [egui::pos2(rect.left(), track_y), egui::pos2(rect.right(), track_y)],
        egui::Stroke::new(3.0, ui.visuals().widgets.inactive.bg_fill),
    );

    if keys.is_empty() {
        painter.text(
            egui::pos2(rect.center().x, track_y + 14.0),
            egui::Align2::CENTER_TOP,
            "Dial in a look, then press Keyframe to set one",
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
    }

    // A drag follows the *key*, not the slot it started in: the list is
    // re-sorted on every move, so the index of the key under the pointer
    // changes as it crosses a neighbour. The index is kept in egui memory
    // and re-pointed after each move.
    let drag_id = ui.id().with("kf_drag");
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    if dragging.is_some_and(|i| i >= keys.len()) {
        dragging = None;
    }

    // Keyframe diamonds.
    let parked = app.keyframe_at_playhead();
    for (i, (t, easing)) in keys.iter().enumerate() {
        let c = egui::pos2(x_of(*t), track_y);
        let selected = parked == Some(i) || dragging == Some(i);
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

        // Each diamond gets its own handle, tall enough to be an easy
        // target, so dragging one retimes it rather than scrubbing the
        // playhead underneath.
        let hit = egui::Rect::from_center_size(c, egui::vec2(d * 2.6, TRACK_H));
        let r = ui.interact(hit, ui.id().with(("kf", i)), egui::Sense::click_and_drag());
        if r.drag_started() {
            dragging = Some(i);
        } else if r.clicked() {
            actions.push(Action::Select(i));
        }
        let r = r.on_hover_text(format!(
            "Keyframe at {:.2}s \u{2014} drag to move (Alt: no frame snap), click to jump \
             here, right-click for options",
            t * duration
        ));

        // Right-click: the easing and delete, as the macOS diamond offers.
        r.context_menu(|ui| {
            ui.label(egui::RichText::new("Interpolation").small().weak());
            for e in Easing::ALL {
                if ui.selectable_label(*easing == e, e.display_name()).clicked() {
                    actions.push(Action::Easing(i, e));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Delete keyframe").clicked() {
                actions.push(Action::Delete(i));
                ui.close();
            }
        });
    }

    // Playhead: a line with a knob at the top, so it reads as a handle
    // distinct from the keyframe diamonds it passes over.
    let px = x_of(app.playhead);
    let playhead_colour = egui::Color32::from_rgb(235, 82, 70);
    painter.line_segment(
        [egui::pos2(px, rect.top()), egui::pos2(px, rect.bottom())],
        egui::Stroke::new(1.5, playhead_colour),
    );
    painter.circle_filled(egui::pos2(px, rect.top() + PLAYHEAD_KNOB), PLAYHEAD_KNOB, playhead_colour);

    // Drive the drag from the pointer directly rather than from the
    // per-diamond response — see `dragging` above.
    if let Some(i) = dragging {
        let (down, pos, alt) = ui.input(|inp| {
            (inp.pointer.primary_down(), inp.pointer.interact_pos(), inp.modifiers.alt)
        });
        if down {
            if let Some(p) = pos {
                let t = t_of(p.x);
                let t = if alt { t } else { app.snap_to_frame(t) };
                actions.push(Action::Move(i, t));
            }
        } else {
            // Released: park the playhead on the key so it stays selected
            // and the preview shows the look it holds.
            if let Some(t) = keys.get(i).map(|k| k.0) {
                actions.push(Action::Scrub(t));
            }
            dragging = None;
        }
    } else if response.dragged() || response.clicked() {
        // Scrub anywhere on the band that isn't a diamond.
        if let Some(p) = response.interact_pointer_pos() {
            actions.push(Action::Scrub(t_of(p.x)));
        }
    }

    // Easing chips, one centred under each keyframe. The row is reserved
    // even with no keys so the bar's height does not jump when the first
    // one is set; a second row appears when neighbours sit too close for
    // their dropdowns to fit side by side.
    let (chip_area, _) = ui.allocate_exact_size(
        egui::vec2(width, CHIP_GAP + CHIP_ROW_H * row_count as f32),
        egui::Sense::hover(),
    );
    for (i, ((_, easing), row)) in keys.iter().zip(rows.iter()).enumerate() {
        let x = (key_xs[i] - CHIP_W / 2.0).clamp(0.0, (width - CHIP_W).max(0.0));
        let chip_rect = egui::Rect::from_min_size(
            egui::pos2(chip_area.left() + x, chip_area.top() + CHIP_GAP + *row as f32 * CHIP_ROW_H),
            egui::vec2(CHIP_W, CHIP_ROW_H - 4.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(chip_rect), |ui| {
            let mut chosen = *easing;
            egui::ComboBox::from_id_salt(("ease", i))
                .selected_text(easing.display_name())
                .width(CHIP_W)
                .show_ui(ui, |ui| {
                    for e in Easing::ALL {
                        if ui.selectable_label(*easing == e, e.display_name()).clicked() {
                            chosen = e;
                        }
                    }
                })
                .response
                .on_hover_text("Interpolation leaving this keyframe");
            if chosen != *easing {
                actions.push(Action::Easing(i, chosen));
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
            Action::Move(i, t) => {
                if let Some(now_at) = app.move_keyframe(i, t) {
                    if dragging == Some(i) {
                        dragging = Some(now_at);
                    }
                }
            }
            Action::Easing(i, e) => app.set_keyframe_easing(i, e),
            Action::Delete(i) => {
                app.delete_keyframe(i);
                dragging = None;
            }
        }
    }

    ui.data_mut(|d| match dragging {
        Some(i) => {
            d.insert_temp(drag_id, i);
        }
        None => {
            d.remove_temp::<usize>(drag_id);
        }
    });
}

/// Ticks at a whole number of seconds (or a clean fraction) chosen so labels
/// have room, with an unlabelled minor tick halfway between.
fn ruler(ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, duration: f64) {
    let seconds = duration.max(0.001);
    let px_per_second = rect.width() as f64 / seconds;
    let step = [0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]
        .into_iter()
        .find(|s| s * px_per_second >= 120.0)
        .unwrap_or(60.0);
    let colour = ui.visuals().weak_text_color();

    let mut s = 0.0;
    while s <= seconds + 1e-9 {
        let x = rect.left() + ((s / seconds) as f32) * rect.width();
        painter.line_segment(
            [egui::pos2(x, rect.top() + 4.0), egui::pos2(x, rect.top() + RULER_H - 5.0)],
            egui::Stroke::new(1.0, colour),
        );
        let label = if s.fract() == 0.0 { format!("{s:.0}s") } else { format!("{s:.1}s") };
        // Keep the last label inside the band rather than clipping it.
        let (anchor, align) = if x + 30.0 > rect.right() {
            (egui::pos2(x - 3.0, rect.top() + 2.0), egui::Align2::RIGHT_TOP)
        } else {
            (egui::pos2(x + 3.0, rect.top() + 2.0), egui::Align2::LEFT_TOP)
        };
        painter.text(anchor, align, label, egui::FontId::monospace(9.0), colour);

        let half = s + step / 2.0;
        if half < seconds {
            let hx = rect.left() + ((half / seconds) as f32) * rect.width();
            painter.line_segment(
                [egui::pos2(hx, rect.top() + 10.0), egui::pos2(hx, rect.top() + RULER_H - 5.0)],
                egui::Stroke::new(1.0, colour.gamma_multiply(0.6)),
            );
        }
        s += step;
    }
}

/// Keyboard: with a key parked under the playhead, Left/Right nudge it a
/// frame (Shift: ten) and Delete removes it. Otherwise Left/Right step the
/// playhead a frame on a still (the transport bar owns that on a video).
/// Nothing fires while a text field has focus.
fn keyboard(app: &mut NtscrtApp, ctx: &egui::Context) {
    if ctx.memory(|m| m.focused().is_some()) {
        return;
    }
    // Read the key events rather than the per-frame summary: several
    // presses can land in one frame (key repeat, or a slow frame) and each
    // should count, and a chord's Shift may already be up again by the time
    // the frame's modifier state is sampled — the event carries its own.
    let (mut frames, mut delete) = (0i64, false);
    ctx.input(|i| {
        for e in &i.events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                let step = if modifiers.shift { 10 } else { 1 };
                match key {
                    egui::Key::ArrowLeft => frames -= step,
                    egui::Key::ArrowRight => frames += step,
                    egui::Key::Delete | egui::Key::Backspace => delete = true,
                    _ => {}
                }
            }
        }
    });
    if frames == 0 && !delete {
        return;
    }

    if let Some(i) = app.keyframe_at_playhead() {
        if delete {
            app.delete_keyframe(i);
            return;
        }
        if let Some(now_at) = app.nudge_keyframe(i, frames) {
            // Follow the key, so it stays selected and the preview shows it.
            if let Some(t) = app.timeline_keys().get(now_at).map(|k| k.t) {
                app.scrub_timeline(t);
            }
        }
        return;
    }

    if frames != 0 && app.video.is_none() {
        let total = app.effective_frame_count();
        if total > 1 {
            let last = (total - 1) as f64;
            let frame = ((app.playhead * last).round() as i64 + frames).clamp(0, last as i64);
            app.scrub_timeline(frame as f64 / last);
        }
    }
}

/// Assign each keyframe's chip to row 0 or 1, so the dropdowns of two keys
/// set close together stagger instead of piling up. `xs` are the keys'
/// pixel positions in time order; `min_gap` is a chip width plus a margin.
fn chip_rows(xs: &[f32], min_gap: f32) -> Vec<usize> {
    let mut last_x_in_row = [f32::NEG_INFINITY; 2];
    xs.iter()
        .map(|&x| {
            let row = if x - last_x_in_row[0] >= min_gap { 0 } else { 1 };
            last_x_in_row[row] = x;
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_spaced_chips_share_the_first_row() {
        assert_eq!(chip_rows(&[0.0, 200.0, 400.0], 110.0), vec![0, 0, 0]);
    }

    #[test]
    fn a_close_neighbour_drops_to_the_second_row() {
        // The second key is too close to the first; the third is far enough
        // from the *first-row* key to come back up.
        assert_eq!(chip_rows(&[100.0, 150.0, 300.0], 110.0), vec![0, 1, 0]);
    }

    #[test]
    fn a_cluster_only_ever_uses_two_rows() {
        // The macOS bar stops at two rows; a third close key overlaps rather
        // than growing the bar indefinitely.
        assert_eq!(chip_rows(&[100.0, 120.0, 140.0], 110.0), vec![0, 1, 1]);
    }

    #[test]
    fn no_keys_means_no_rows() {
        assert!(chip_rows(&[], 110.0).is_empty());
    }
}
