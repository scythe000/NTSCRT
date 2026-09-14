//! Source rotation.
//!
//! This has no macOS counterpart — it is a Windows addition, not a port.
//!
//! Rotation happens at the very front of the pipeline, on the source pixels,
//! before the signal stage sees them. That ordering matters and is not
//! arbitrary: NTSC is a *scanline* effect and a CRT draws horizontal lines,
//! so rotating afterwards would carry the scanlines round with the picture
//! and tilt the whole illusion. Rotating first means a portrait clip turned
//! landscape is degraded and scanned exactly as if it had been shot that way.
//!
//! The cost is one strided copy of the full-resolution frame. That is real,
//! but the signal stage downstream is orders of magnitude more expensive, so
//! it does not move the frame budget.

use serde::{Deserialize, Serialize};

/// Quarter-turn rotation, clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "i32", into = "i32")]
pub enum Rotation {
    #[default]
    None,
    /// 90 degrees clockwise.
    Cw90,
    Cw180,
    /// 270 clockwise, i.e. 90 anticlockwise.
    Cw270,
}

impl Rotation {
    pub const ALL: [Rotation; 4] =
        [Rotation::None, Rotation::Cw90, Rotation::Cw180, Rotation::Cw270];

    pub fn degrees(self) -> i32 {
        match self {
            Rotation::None => 0,
            Rotation::Cw90 => 90,
            Rotation::Cw180 => 180,
            Rotation::Cw270 => 270,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Rotation::None => "None",
            Rotation::Cw90 => "90\u{00B0} right",
            Rotation::Cw180 => "180\u{00B0}",
            Rotation::Cw270 => "90\u{00B0} left",
        }
    }

    /// Whether this rotation exchanges width and height.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::Cw90 | Rotation::Cw270)
    }

    /// Size after rotating a `width` x `height` source.
    pub fn output_size(self, width: u32, height: u32) -> (u32, u32) {
        if self.swaps_axes() {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// One more quarter turn clockwise — what the toolbar button does.
    pub fn next_cw(self) -> Rotation {
        match self {
            Rotation::None => Rotation::Cw90,
            Rotation::Cw90 => Rotation::Cw180,
            Rotation::Cw180 => Rotation::Cw270,
            Rotation::Cw270 => Rotation::None,
        }
    }
}

impl TryFrom<i32> for Rotation {
    type Error = String;
    /// Accepts any multiple of 90, including negatives, so a preset written
    /// by hand with -90 does the sane thing.
    fn try_from(deg: i32) -> Result<Self, Self::Error> {
        match deg.rem_euclid(360) {
            0 => Ok(Rotation::None),
            90 => Ok(Rotation::Cw90),
            180 => Ok(Rotation::Cw180),
            270 => Ok(Rotation::Cw270),
            _ => Err(format!("rotation must be a multiple of 90 degrees, got {deg}")),
        }
    }
}

impl From<Rotation> for i32 {
    fn from(r: Rotation) -> i32 {
        r.degrees()
    }
}

/// Rotate tightly packed RGBA8 pixels, returning the rotated buffer and its
/// new size.
///
/// `Rotation::None` still copies, so callers get an owned buffer either way;
/// see [`rotate_rgba_into`] to avoid that when it matters.
pub fn rotate_rgba(src: &[u8], width: u32, height: u32, rot: Rotation) -> (Vec<u8>, u32, u32) {
    let (dw, dh) = rot.output_size(width, height);
    let mut dst = vec![0u8; (dw as usize) * (dh as usize) * 4];
    rotate_rgba_into(src, width, height, rot, &mut dst);
    (dst, dw, dh)
}

/// Rotate into a caller-owned buffer, which must be `dw * dh * 4` bytes.
///
/// Silently does nothing if either buffer is the wrong size — the caller
/// sized them from [`Rotation::output_size`], so a mismatch is a bug at the
/// call site rather than something to surface to the user mid-frame.
pub fn rotate_rgba_into(src: &[u8], width: u32, height: u32, rot: Rotation, dst: &mut [u8]) {
    let (w, h) = (width as usize, height as usize);
    let (dw, dh) = rot.output_size(width, height);
    let (dw, dh) = (dw as usize, dh as usize);
    if src.len() < w * h * 4 || dst.len() < dw * dh * 4 {
        return;
    }

    match rot {
        Rotation::None => {
            dst[..w * h * 4].copy_from_slice(&src[..w * h * 4]);
        }
        Rotation::Cw180 => {
            // Reverse pixel order wholesale.
            for y in 0..h {
                let sy = h - 1 - y;
                for x in 0..w {
                    let sx = w - 1 - x;
                    let s = (sy * w + sx) * 4;
                    let d = (y * w + x) * 4;
                    dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
        }
        Rotation::Cw90 => {
            // dst(dx, dy) = src(dy, h - 1 - dx)
            for dy in 0..dh {
                for dx in 0..dw {
                    let s = ((h - 1 - dx) * w + dy) * 4;
                    let d = (dy * dw + dx) * 4;
                    dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
        }
        Rotation::Cw270 => {
            // dst(dx, dy) = src(w - 1 - dy, dx)
            for dy in 0..dh {
                for dx in 0..dw {
                    let s = (dx * w + (w - 1 - dy)) * 4;
                    let d = (dy * dw + dx) * 4;
                    dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x3 image whose pixels carry their own coordinates, so a rotation
    /// can be checked position by position rather than by eye.
    fn grid(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        v
    }

    fn at(buf: &[u8], w: u32, x: u32, y: u32) -> (u8, u8) {
        let i = ((y * w + x) * 4) as usize;
        (buf[i], buf[i + 1])
    }

    #[test]
    fn sizes_swap_only_on_quarter_turns() {
        assert_eq!(Rotation::None.output_size(320, 240), (320, 240));
        assert_eq!(Rotation::Cw90.output_size(320, 240), (240, 320));
        assert_eq!(Rotation::Cw180.output_size(320, 240), (320, 240));
        assert_eq!(Rotation::Cw270.output_size(320, 240), (240, 320));
        assert!(Rotation::Cw90.swaps_axes() && Rotation::Cw270.swaps_axes());
        assert!(!Rotation::None.swaps_axes() && !Rotation::Cw180.swaps_axes());
    }

    #[test]
    fn ninety_clockwise_moves_top_left_to_top_right() {
        let (w, h) = (2u32, 3u32);
        let src = grid(w, h);
        let (dst, dw, dh) = rotate_rgba(&src, w, h, Rotation::Cw90);
        assert_eq!((dw, dh), (3, 2));
        // Source (0,0) must land at the top-right of the result.
        assert_eq!(at(&dst, dw, dw - 1, 0), (0, 0));
        // And source (w-1, 0) — the top-right — lands at the bottom-right.
        assert_eq!(at(&dst, dw, dw - 1, dh - 1), (w as u8 - 1, 0));
    }

    #[test]
    fn two_seventy_is_ninety_the_other_way() {
        let (w, h) = (2u32, 3u32);
        let src = grid(w, h);
        let (dst, dw, dh) = rotate_rgba(&src, w, h, Rotation::Cw270);
        assert_eq!((dw, dh), (3, 2));
        // Source (0,0) lands at the bottom-left.
        assert_eq!(at(&dst, dw, 0, dh - 1), (0, 0));
    }

    #[test]
    fn one_eighty_reverses_both_axes() {
        let (w, h) = (2u32, 3u32);
        let src = grid(w, h);
        let (dst, dw, dh) = rotate_rgba(&src, w, h, Rotation::Cw180);
        assert_eq!((dw, dh), (w, h));
        assert_eq!(at(&dst, dw, dw - 1, dh - 1), (0, 0));
        assert_eq!(at(&dst, dw, 0, 0), (w as u8 - 1, h as u8 - 1));
    }

    #[test]
    fn none_is_an_exact_copy() {
        let src = grid(4, 5);
        let (dst, dw, dh) = rotate_rgba(&src, 4, 5, Rotation::None);
        assert_eq!((dw, dh), (4, 5));
        assert_eq!(dst, src);
    }

    #[test]
    fn four_quarter_turns_return_the_original() {
        let (w, h) = (3u32, 5u32);
        let src = grid(w, h);
        let (mut buf, mut bw, mut bh) = (src.clone(), w, h);
        for _ in 0..4 {
            let (n, nw, nh) = rotate_rgba(&buf, bw, bh, Rotation::Cw90);
            buf = n;
            bw = nw;
            bh = nh;
        }
        assert_eq!((bw, bh), (w, h));
        assert_eq!(buf, src, "four 90-degree turns should be identity");
    }

    #[test]
    fn opposite_turns_cancel() {
        let (w, h) = (3u32, 5u32);
        let src = grid(w, h);
        let (a, aw, ah) = rotate_rgba(&src, w, h, Rotation::Cw90);
        let (b, bw, bh) = rotate_rgba(&a, aw, ah, Rotation::Cw270);
        assert_eq!((bw, bh), (w, h));
        assert_eq!(b, src);
    }

    #[test]
    fn next_cw_cycles_through_all_four() {
        let mut r = Rotation::None;
        let seen: Vec<Rotation> = (0..4)
            .map(|_| {
                r = r.next_cw();
                r
            })
            .collect();
        assert_eq!(seen, vec![Rotation::Cw90, Rotation::Cw180, Rotation::Cw270, Rotation::None]);
    }

    #[test]
    fn degrees_round_trip_and_accept_negatives() {
        for r in Rotation::ALL {
            assert_eq!(Rotation::try_from(r.degrees()), Ok(r));
        }
        // -90 is 270 clockwise.
        assert_eq!(Rotation::try_from(-90), Ok(Rotation::Cw270));
        assert_eq!(Rotation::try_from(450), Ok(Rotation::Cw90));
        assert!(Rotation::try_from(45).is_err());
    }

    #[test]
    fn serialises_as_plain_degrees() {
        let json = serde_json::to_string(&Rotation::Cw90).unwrap();
        assert_eq!(json, "90");
        let back: Rotation = serde_json::from_str("270").unwrap();
        assert_eq!(back, Rotation::Cw270);
    }

    #[test]
    fn a_mis_sized_destination_is_ignored_rather_than_panicking() {
        let src = grid(4, 4);
        let mut too_small = vec![0u8; 8];
        rotate_rgba_into(&src, 4, 4, Rotation::Cw90, &mut too_small);
        assert!(too_small.iter().all(|b| *b == 0), "must not write past the end");
    }
}
