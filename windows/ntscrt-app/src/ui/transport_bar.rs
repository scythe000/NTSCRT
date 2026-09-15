//! Video transport, ported from `Sources/CrtApp/Views/TransportBar.swift`.
//!
//! Full-width, docked under the preview and above the status bar: play/pause,
//! a frame scrubber spanning the pane (maximum scrubbing precision), the
//! RAM-preview render bar under it, and a frame/time/fps readout. Drawn only
//! when a video source is loaded, exactly as the SwiftUI original is.
//!
//! The scrubber works in whole frames rather than in time, so a drag always
//! lands on a frame boundary and the readout can never disagree with what is
//! on screen. Left/right step one frame at a time for the same reason.

use eframe::egui;

use crate::app::NtscrtApp;

/// Height of the render bar under the slider, and how far it is inset to
/// line up with the slider thumb's travel.
const BAR_HEIGHT: f32 = 3.0;
const BAR_INSET: f32 = 8.0;

pub fn show(app: &mut NtscrtApp, root: &mut egui::Ui) {
    // On a video the timeline replaces the transport, as on macOS — it has
    // the same play button and scrubber, plus the keyframes.
    if app.video.is_none() || app.timeline_open {
        return;
    }
    let ctx = root.ctx().clone();

    egui::Panel::bottom("transport").show(root, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);

            let playing = app.is_playing();
            let icon = if playing { "\u{23F8}" } else { "\u{25B6}" };
            let hint = if playing {
                "Pause"
            } else {
                "Play the video in the preview, with all effects applied"
            };
            if ui
                .add_sized([26.0, 22.0], egui::Button::new(icon))
                .on_hover_text(hint)
                .clicked()
            {
                app.toggle_playback();
            }

            let Some(video) = app.video.as_ref() else { return };
            let total = video.total_frames();
            let current = video.current_frame_index;
            let readout = format!(
                "{}/{}  \u{00B7}  {:.2}s  \u{00B7}  {:.0} fps",
                current + 1,
                total,
                video.current_time(),
                video.frame_rate()
            );
            let cached_ranges = video.cached_ranges.clone();
            let prerendering = video.prerender_active;

            // Measure the readout so the scrubber can take exactly the rest
            // of the pane — it is the control that wants every pixel.
            let font = egui::FontId::monospace(11.0);
            let readout_width = ui
                .painter()
                .layout_no_wrap(readout.clone(), font.clone(), egui::Color32::PLACEHOLDER)
                .size()
                .x;

            let scrubber_width =
                (ui.available_width() - readout_width - 3.0 * ui.spacing().item_spacing.x).max(60.0);

            let mut seek_to = None;
            ui.vertical(|ui| {
                ui.spacing_mut().slider_width = scrubber_width;
                let mut position = current as f64;
                let changed = ui
                    .add(
                        egui::Slider::new(&mut position, 0.0..=(total.saturating_sub(1)).max(1) as f64)
                            .step_by(1.0)
                            .show_value(false),
                    )
                    .changed();
                if changed {
                    seek_to = Some(position.round().max(0.0) as usize);
                }

                // RAM-preview render bar: which frames are already baked.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(scrubber_width, BAR_HEIGHT), egui::Sense::hover());
                paint_render_bar(ui, rect, &cached_ranges, total, prerendering);
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(readout).font(font).weak());
            });

            if let Some(frame) = seek_to {
                app.seek_to_frame(frame);
            }
        });
        ui.add_space(4.0);
    });

    // Arrow keys step a frame at a time, which is the other half of "frame
    // accurate": a drag gets you close, these put you exactly where you want.
    // Not while a text field has focus (they move its caret), and not while
    // the timeline has a keyframe parked — there they nudge the key.
    if ctx.memory(|m| m.focused().is_some()) || app.timeline_owns_arrows() {
        return;
    }
    // Counted per event, so key repeat and several presses in one slow
    // frame each step a frame; Shift steps ten.
    let mut steps: i64 = 0;
    ctx.input(|i: &egui::InputState| {
        for e in &i.events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                let n = if modifiers.shift { 10 } else { 1 };
                match key {
                    egui::Key::ArrowLeft => steps -= n,
                    egui::Key::ArrowRight => steps += n,
                    _ => {}
                }
            }
        }
    });
    if steps != 0 {
        if let Some(video) = app.video.as_ref() {
            let total = video.total_frames() as i64;
            let current = video.current_frame_index as i64;
            let next = (current + steps).rem_euclid(total) as usize;
            app.stop_playback();
            app.seek_to_frame(next);
        }
    }
}

/// Draw the cached runs as a strip under the scrubber — the After Effects
/// render bar. Inset to roughly match the slider thumb's travel, as the
/// SwiftUI version is.
fn paint_render_bar(
    ui: &egui::Ui,
    rect: egui::Rect,
    ranges: &[std::ops::Range<usize>],
    total: usize,
    prerendering: bool,
) {
    let painter = ui.painter_at(rect);
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left() + BAR_INSET, rect.top()),
        egui::pos2((rect.right() - BAR_INSET).max(rect.left() + BAR_INSET + 1.0), rect.bottom()),
    );
    painter.rect_filled(track, 0.0, ui.visuals().faint_bg_color);
    paint_cached_runs(&painter, track, ranges, total, prerendering);
}

/// The cached runs themselves, mapped by frame index onto `track`. Shared
/// with the timeline bar's strip under its ruler, so the two bars agree.
pub(super) fn paint_cached_runs(
    painter: &egui::Painter,
    track: egui::Rect,
    ranges: &[std::ops::Range<usize>],
    total: usize,
    prerendering: bool,
) {
    // Dimmer while the pre-render is still working, so a bar that is still
    // growing reads differently from one that is finished.
    let colour = if prerendering {
        egui::Color32::from_rgba_unmultiplied(80, 180, 90, 130)
    } else {
        egui::Color32::from_rgba_unmultiplied(80, 200, 90, 160)
    };

    let width = track.width();
    let total = total.max(1) as f32;
    for range in ranges {
        let x = track.left() + range.start as f32 / total * width;
        let w = (range.len() as f32 / total * width).max(1.0);
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, track.top()), egui::vec2(w, track.height())),
            0.0,
            colour,
        );
    }
}
