//! Preview, ported from `Views/PreviewView.swift`, with the framing rules of
//! `PreviewScaling.swift` / `PreviewGeometry.swift`.
//!
//! The CRT pass always renders at a whole multiple of the chain input and is
//! then fitted to the panel, so what you see never depends on how big the
//! window is. On top of that fit:
//!
//! - **Integer scale** snaps the displayed size to a whole multiple of the
//!   chain input, so every scanline is the same height on screen — a
//!   display snap, letterboxed, nothing to do with the render.
//! - **Zoom** magnifies about the panel centre; **pan** slides the enlarged
//!   image. Alt+scroll zooms around the cursor, Space-drag or middle-drag
//!   pans (a plain drag pans too when Compare is off — with it on, the drag
//!   moves the split), double-click resets both.
//! - **Compare** divides the image: full pipeline on the left of the line,
//!   untouched original on the right.
//!
//! The framing is a pure function (`frame`) so it can be tested without a
//! window, as the macOS build's geometry is.

use eframe::egui;

use crate::app::{NtscrtApp, RenderState};

pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 8.0;

/// Where the image lands in the panel.
///
/// `panel` is the area available; `image` is the aspect ratio's pixel size
/// (the chain input — its ratio is what matters, and for integer scale its
/// size too); `zoom` and `pan` are the view state; `integer` asks for the
/// whole-multiple snap. Returns the rect to draw the render into and the pan
/// actually used, clamped so the image cannot be pushed out of the panel.
pub fn frame(
    panel: egui::Rect,
    image: (u32, u32),
    zoom: f32,
    pan: egui::Vec2,
    integer: bool,
) -> (egui::Rect, egui::Vec2) {
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    let aspect = iw / ih;

    // Aspect-fit, then snap to a whole multiple of the image if asked. The
    // snap rounds *down* so the image always fits; a fit smaller than 1× is
    // left alone rather than snapped to nothing.
    let mut size = egui::vec2(panel.width(), panel.width() / aspect);
    if size.y > panel.height() {
        size = egui::vec2(panel.height() * aspect, panel.height());
    }
    if integer {
        let k = (size.y / ih).floor().max(1.0);
        if k * ih <= panel.height() + 0.5 && k * iw <= panel.width() + 0.5 {
            size = egui::vec2(k * iw, k * ih);
        }
    }
    size *= zoom.max(0.01);

    // Panning is only meaningful along axes where the image overflows the
    // panel; along the others it stays centred.
    let slack = ((size - panel.size()) * 0.5).max(egui::Vec2::ZERO);
    let pan = egui::vec2(pan.x.clamp(-slack.x, slack.x), pan.y.clamp(-slack.y, slack.y));

    (egui::Rect::from_center_size(panel.center() + pan, size), pan)
}

pub fn show(app: &mut NtscrtApp, root: &mut egui::Ui, rs: Option<&RenderState>) {
    let ctx = root.ctx().clone();
    egui::CentralPanel::default().show(root, |ui| {
        let Some(rs) = rs else {
            ui.centered_and_justified(|ui| ui.label("wgpu render state unavailable"));
            return;
        };

        let available = ui.available_size();
        let Some((texture_id, size)) = app.preview_image(rs, available) else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("No shader loaded").weak())
            });
            return;
        };

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available.x, available.y),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(16));

        // Alt+scroll zooms about the cursor: the image point under it stays
        // put, which is what makes zooming into a detail feel right.
        let (scroll, alt, hover) =
            ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.alt, i.pointer.hover_pos()));
        if alt && scroll.abs() > 0.0 && response.hovered() {
            let old = app.zoom;
            let new = (old * (1.0 + scroll * 0.002)).clamp(MIN_ZOOM, MAX_ZOOM);
            if let Some(p) = hover {
                // The cursor's offset from the image centre scales with zoom;
                // move the centre so the point under the cursor is unchanged.
                let centre = rect.center() + app.pan;
                app.pan += (centre - p) * (new / old - 1.0);
            }
            app.zoom = new;
            app.mark_dirty();
        }
        if response.double_clicked() {
            app.zoom = 1.0;
            app.pan = egui::Vec2::ZERO;
            app.mark_dirty();
        }

        let chain = app.chain_input_size();
        let (image_rect, pan) = frame(rect, chain, app.zoom, app.pan, app.integer_scale);
        app.pan = pan;

        // Pan: Space-drag, middle-drag, or a plain drag when Compare is off.
        let (space, middle) = ui.input(|i| {
            (i.key_down(egui::Key::Space), i.pointer.button_down(egui::PointerButton::Middle))
        });
        let panning = response.dragged() && (space || middle || !app.compare);
        if panning {
            app.pan += response.drag_delta();
        }

        if app.compare {
            let split = app.compare_split.clamp(0.0, 1.0);
            let split_x = image_rect.left() + image_rect.width() * split;

            // Left of the line: the full pipeline.
            let left = egui::Rect::from_min_max(
                image_rect.min,
                egui::pos2(split_x, image_rect.max.y),
            );
            painter.image(
                texture_id,
                left,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(split, 1.0)),
                egui::Color32::WHITE,
            );

            // Right of the line: the untouched source.
            let source_id = source_texture(app, &ctx);
            let right = egui::Rect::from_min_max(
                egui::pos2(split_x, image_rect.min.y),
                image_rect.max,
            );
            painter.image(
                source_id,
                right,
                egui::Rect::from_min_max(egui::pos2(split, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            painter.line_segment(
                [
                    egui::pos2(split_x, image_rect.min.y.max(rect.min.y)),
                    egui::pos2(split_x, image_rect.max.y.min(rect.max.y)),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
            );

            // Drag the line to move the split.
            if response.dragged() && !panning {
                if let Some(pos) = response.interact_pointer_pos() {
                    app.compare_split =
                        ((pos.x - image_rect.left()) / image_rect.width().max(1.0)).clamp(0.0, 1.0);
                }
            }
        } else {
            painter.image(
                texture_id,
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        let scale = image_rect.height() / chain.1.max(1) as f32;
        painter.text(
            rect.left_bottom() + egui::vec2(6.0, -6.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{}\u{00D7}{} rendered  \u{00B7}  {scale:.2}\u{00D7} on screen", size.0, size.1),
            egui::FontId::monospace(11.0),
            egui::Color32::from_white_alpha(110),
        );
    });
}

/// The untouched source, uploaded as a plain egui texture for the compare
/// split's right-hand side.
///
/// The handle is cached on the app and keyed by `source_version`: egui frees
/// a texture when its last handle drops, so holding it here both avoids
/// re-uploading the image every frame and keeps it alive while it is drawn.
fn source_texture(app: &mut NtscrtApp, ctx: &egui::Context) -> egui::TextureId {
    if !matches!(&app.source_texture, Some((v, _)) if *v == app.source_version) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [app.source.width as usize, app.source.height as usize],
            &app.source.pixels,
        );
        let handle = ctx.load_texture("ntscrt.source", image, egui::TextureOptions::NEAREST);
        app.source_texture = Some((app.source_version, handle));
    }
    app.source_texture.as_ref().unwrap().1.id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h))
    }

    #[test]
    fn fit_fills_the_limiting_axis_and_centres() {
        let (r, _) = frame(panel(1600.0, 900.0), (320, 240), 1.0, egui::Vec2::ZERO, false);
        assert_eq!(r.height(), 900.0);
        assert_eq!(r.width(), 1200.0);
        assert_eq!(r.center(), egui::pos2(800.0, 450.0));
    }

    #[test]
    fn integer_scale_snaps_down_to_a_whole_multiple() {
        // 900 / 240 = 3.75 → 3×: 960×720, letterboxed on both axes.
        let (r, _) = frame(panel(1600.0, 900.0), (320, 240), 1.0, egui::Vec2::ZERO, true);
        assert_eq!(r.size(), egui::vec2(960.0, 720.0));
        assert_eq!(r.center(), egui::pos2(800.0, 450.0));
    }

    #[test]
    fn integer_scale_below_one_to_one_is_left_as_a_fit() {
        // A panel smaller than the chain input cannot show a whole multiple;
        // snapping to 1× would overflow, so it stays an aspect fit.
        let (r, _) = frame(panel(200.0, 150.0), (320, 240), 1.0, egui::Vec2::ZERO, true);
        assert_eq!(r.size(), egui::vec2(200.0, 150.0));
    }

    #[test]
    fn zoom_scales_the_framing_and_pan_is_clamped_to_the_overflow() {
        let (r, pan) =
            frame(panel(1600.0, 900.0), (320, 240), 2.0, egui::vec2(10_000.0, -10_000.0), false);
        assert_eq!(r.size(), egui::vec2(2400.0, 1800.0));
        // Overflow is (2400-1600)/2 = 400 wide and (1800-900)/2 = 450 tall;
        // the pan cannot exceed that, so an edge of the image always
        // reaches an edge of the panel.
        assert_eq!(pan, egui::vec2(400.0, -450.0));
        assert_eq!(r.left(), 0.0);
        assert_eq!(r.bottom(), 900.0);
    }

    #[test]
    fn pan_is_ignored_along_an_axis_that_does_not_overflow() {
        let (r, pan) = frame(panel(1600.0, 900.0), (320, 240), 1.0, egui::vec2(50.0, 50.0), false);
        assert_eq!(pan, egui::Vec2::ZERO);
        assert_eq!(r.center(), egui::pos2(800.0, 450.0));
    }
}
