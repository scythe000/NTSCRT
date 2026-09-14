//! Image loading and saving, replacing the macOS build's
//! `Sources/CrtCore/ImageIO.swift` (CoreGraphics/ImageIO).
//!
//! The `image` crate covers PNG/JPEG/BMP/TIFF/WebP. HEIC, which the Mac build
//! accepts through ImageIO, has no pure-Rust decoder and is not supported —
//! see the Limitations section of the Windows README.

use std::path::Path;

/// A decoded source image: tightly packed RGBA8, no row padding.
#[derive(Clone)]
pub struct SourceImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl SourceImage {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let img = image::open(path)?.to_rgba8();
        let (width, height) = img.dimensions();
        Ok(Self { width, height, pixels: img.into_raw() })
    }

    /// Extensions the file picker offers, matching what `image` can decode.
    pub const EXTENSIONS: &'static [&'static str] =
        &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"];

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Solid-colour image, used by tests and as the startup placeholder.
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&rgba);
        }
        Self { width, height, pixels }
    }
}

/// Write tightly packed RGBA8 pixels to a PNG.
pub fn save_png(
    path: &Path,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let buf = image::RgbaImage::from_raw(width, height, pixels.to_vec())
        .ok_or("pixel buffer does not match the given dimensions")?;
    buf.save(path)?;
    Ok(())
}

/// Strip the row padding wgpu requires on readback.
///
/// `copy_texture_to_buffer` needs each row aligned to 256 bytes, so a
/// readback of a width that isn't a multiple of 64 pixels comes back with
/// gaps between rows.
pub fn unpad_rows(padded: &[u8], padded_bytes_per_row: u32, width: u32, height: u32) -> Vec<u8> {
    let row = (width * 4) as usize;
    let stride = padded_bytes_per_row as usize;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * stride;
        out.extend_from_slice(&padded[start..start + row]);
    }
    out
}

/// Round `bytes_per_row` up to wgpu's 256-byte copy alignment.
pub fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(align) * align
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_image_has_the_expected_shape() {
        let img = SourceImage::solid(4, 3, [1, 2, 3, 255]);
        assert_eq!(img.size(), (4, 3));
        assert_eq!(img.pixels.len(), 4 * 3 * 4);
        assert_eq!(&img.pixels[0..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn padding_rounds_up_to_the_copy_alignment() {
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
        // Already aligned widths are left alone.
        assert_eq!(padded_bytes_per_row(128), 512);
    }

    #[test]
    fn unpadding_drops_only_the_gap_bytes() {
        // 2x2 image, rows padded from 8 to 12 bytes.
        let padded: Vec<u8> = vec![
            1, 1, 1, 1, 2, 2, 2, 2, 9, 9, 9, 9, // row 0 + padding
            3, 3, 3, 3, 4, 4, 4, 4, 9, 9, 9, 9, // row 1 + padding
        ];
        let out = unpad_rows(&padded, 12, 2, 2);
        assert_eq!(out, vec![1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4]);
    }

    #[test]
    fn png_round_trips_through_disk() {
        let dir = std::env::temp_dir();
        let path = dir.join("ntscrt-io-test.png");
        let img = SourceImage::solid(8, 5, [10, 20, 30, 255]);
        save_png(&path, &img.pixels, 8, 5).unwrap();

        let back = SourceImage::load(&path).unwrap();
        assert_eq!(back.size(), (8, 5));
        assert_eq!(back.pixels, img.pixels);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_rejects_a_mismatched_buffer() {
        let path = std::env::temp_dir().join("ntscrt-io-bad.png");
        let too_small = vec![0u8; 4];
        assert!(save_png(&path, &too_small, 8, 5).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
