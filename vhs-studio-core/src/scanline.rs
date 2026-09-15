//! Scanline-grid math, ported from `Sources/CrtCore/ScanlineGrid.swift`.
//!
//! CRT shaders draw their scanline and mask structure in *output* pixels.
//! When the output height isn't a whole multiple of the chain input height,
//! one source line covers a fractional number of rows (416 lines into 702
//! rows = 1.69), so some lines land on two pixels and some on one and the
//! phase drifts down the frame — visible as horizontal bands of scanlines.
//!
//! Two ways out, both offered:
//!  - render at a whole multiple and integrate down ([`ScanlineGrid::supersample_factor`]),
//!    which keeps the requested size;
//!  - snap the requested size onto the grid ([`ScanlineGrid::snapped_size`]),
//!    which keeps the scanlines exact but changes the dimensions.

use crate::downscale::DownscaleSpec;

pub struct ScanlineGrid;

impl ScanlineGrid {
    /// Multiple of the chain input to render at before downsampling to the
    /// requested size. 1 = render straight to the target.
    pub fn supersample_factor(input_height: u32, target_height: u32) -> u32 {
        if input_height == 0 || target_height == 0 {
            return 1;
        }
        // Already an exact multiple: the pattern is periodic, leave it alone
        // (this is what snapping produces, and supersampling would only
        // soften it).
        if target_height % input_height == 0 {
            return 1;
        }

        let rows_per_line = target_height as f64 / input_height as f64;
        // At 3+ rows per source line the scanlines are already well
        // resolved; supersampling would only cost time.
        if rows_per_line >= 3.0 {
            return 1;
        }

        // Render 4 rows per source line — enough to resolve the scanline
        // profile — using an even multiple (odd ones put the beam boundary
        // exactly on a pixel edge in the glow shaders and reintroduce row
        // jitter). Step down if that would be disproportionate: a large
        // input with the downscale stage off (4K → a 540px GIF) is already
        // minifying and must not be blown up to 8640 rows.
        let too_big = |k: u32| k * input_height > 4 * target_height || k * input_height > 4096;
        let mut k = 4u32;
        while k > 2 && too_big(k) {
            k -= 2;
        }
        if too_big(k) {
            return 1;
        }
        k
    }

    /// What the shader actually sees: the downscale output when enabled,
    /// otherwise the source itself.
    pub fn chain_input_size(width: u32, height: u32, downscale: Option<&DownscaleSpec>) -> (u32, u32) {
        match downscale {
            Some(d) => (d.width, d.height),
            None => (width, height),
        }
    }

    /// Nearest output size whose height is a whole multiple of the chain
    /// input height, so every source line gets the same number of rows.
    /// Multiples of 2 and up are rounded to even (see above); 1x is kept as
    /// is, since at native size there's no scanline stretching to alias.
    pub fn snapped_size(input_width: u32, input_height: u32, target_height: u32) -> (u32, u32) {
        if input_width == 0 || input_height == 0 || target_height == 0 {
            return (input_width, input_height);
        }
        let exact = target_height as f64 / input_height as f64;
        let mut k = (exact.round() as i64).max(1);
        if k >= 2 && k % 2 != 0 {
            // Pick whichever even multiple is closer to what was asked for.
            k = if (exact - (k - 1) as f64) < ((k + 1) as f64 - exact) {
                k - 1
            } else {
                k + 1
            };
        }
        let k = k.max(1) as u32;
        (input_width * k, input_height * k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_multiples_are_left_alone() {
        assert_eq!(ScanlineGrid::supersample_factor(240, 960), 1);
        assert_eq!(ScanlineGrid::supersample_factor(240, 240), 1);
    }

    #[test]
    fn zero_dimensions_are_safe() {
        assert_eq!(ScanlineGrid::supersample_factor(0, 960), 1);
        assert_eq!(ScanlineGrid::supersample_factor(240, 0), 1);
        assert_eq!(ScanlineGrid::snapped_size(0, 0, 600), (0, 0));
    }

    #[test]
    fn well_resolved_scanlines_skip_supersampling() {
        // 240 into 800 = 3.33 rows per line, already past the 3-row bar.
        assert_eq!(ScanlineGrid::supersample_factor(240, 800), 1);
    }

    #[test]
    fn fractional_low_ratios_supersample_evenly() {
        // 416 into 702 = 1.69 rows per line — the banding case from the docs.
        let k = ScanlineGrid::supersample_factor(416, 702);
        assert!(k > 1, "expected supersampling, got {k}");
        assert_eq!(k % 2, 0, "factor must be even, got {k}");
    }

    #[test]
    fn oversized_inputs_refuse_to_blow_up() {
        // A 4K-tall source headed for a 540px GIF is already minifying.
        assert_eq!(ScanlineGrid::supersample_factor(2160, 540), 1);
        // And nothing may exceed the 4096-row ceiling.
        for input in [1200u32, 1500, 2000, 3000] {
            let k = ScanlineGrid::supersample_factor(input, input * 2 + 1);
            assert!(k * input <= 4096, "input {input} * k {k} exceeded 4096");
        }
    }

    #[test]
    fn snapping_lands_on_whole_multiples() {
        let (w, h) = ScanlineGrid::snapped_size(320, 240, 700);
        assert_eq!(h % 240, 0, "height {h} is not a multiple of 240");
        assert_eq!((w / 320), (h / 240), "width and height must share a factor");
    }

    #[test]
    fn snapping_rounds_to_even_multiples_above_one() {
        // 240 * 3 = 720 is the nearest, but odd multiples are rejected.
        let (_, h) = ScanlineGrid::snapped_size(320, 240, 720);
        let k = h / 240;
        assert_eq!(k % 2, 0, "expected an even multiple, got {k}");
    }

    #[test]
    fn snapping_keeps_native_size_at_one_x() {
        assert_eq!(ScanlineGrid::snapped_size(320, 240, 240), (320, 240));
        // Below native still clamps to 1x rather than collapsing to zero.
        assert_eq!(ScanlineGrid::snapped_size(320, 240, 10), (320, 240));
    }

    #[test]
    fn chain_input_follows_the_downscale_stage() {
        use crate::downscale::DownscaleMethod;
        assert_eq!(ScanlineGrid::chain_input_size(1920, 1080, None), (1920, 1080));
        let spec = DownscaleSpec { width: 320, height: 240, method: DownscaleMethod::Area };
        assert_eq!(ScanlineGrid::chain_input_size(1920, 1080, Some(&spec)), (320, 240));
    }
}
