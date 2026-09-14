//! Preview, ported from `Views/PreviewView.swift`.
//!
//! Compare divides the preview: full pipeline on the left of the line,
//! untouched original on the right — drag the line to move the split.
//!
//! The CRT pass always renders at a whole multiple of the downscale and is
//! then fitted to the window, so what you see never depends on how big the
//! window is, and zooming in inspects the real rendered pixels.

use eframe::egui;

use crate::app::{NtscrtApp, RenderState};

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

        // Fit the rendered image into the panel, preserving aspect ratio.
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let mut draw = egui::vec2(available.x, available.x / aspect);
        if draw.y > available.y {
            draw = egui::vec2(available.y * aspect, available.y);
        }

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available.x, available.y),
            egui::Sense::click_and_drag(),
        );
        let image_rect = egui::Rect::from_center_size(rect.center(), draw);

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(16));

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
                    egui::pos2(split_x, image_rect.min.y),
                    egui::pos2(split_x, image_rect.max.y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
            );

            // Drag the line to move the split.
            if response.dragged() {
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

        // Alt+scroll zooms, matching the macOS preview.
        let (scroll, alt) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.alt));
        if alt && scroll.abs() > 0.0 && response.hovered() {
            app.zoom = (app.zoom * (1.0 + scroll * 0.002)).clamp(0.25, 4.0);
            app.mark_dirty();
        }

        painter.text(
            rect.left_bottom() + egui::vec2(6.0, -6.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{}\u{00D7}{} rendered", size.0, size.1),
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
