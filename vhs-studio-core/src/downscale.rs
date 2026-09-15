//! Downscale specification, ported from `Sources/CrtCore/Downscaler.swift`.
//!
//! The kernels themselves live in the renderer (WGSL, in `vhs-studio-app`); this
//! is the part the UI and the export path need to agree on. String forms match
//! the Swift `DownscaleMethod` raw values so presets stay compatible.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownscaleMethod {
    Nearest,
    /// "Stabilized nearest": a narrow Gaussian just wide enough to kill
    /// temporal shimmer on video, keeping most of nearest's punch.
    #[serde(rename = "nearest+")]
    NearestAA,
    Bilinear,
    Bicubic,
    Lanczos,
    Area,
}

impl DownscaleMethod {
    pub const ALL: [DownscaleMethod; 6] = [
        DownscaleMethod::Nearest,
        DownscaleMethod::NearestAA,
        DownscaleMethod::Bilinear,
        DownscaleMethod::Bicubic,
        DownscaleMethod::Lanczos,
        DownscaleMethod::Area,
    ];

    /// Matches the Swift raw values, which is what preset JSON stores.
    pub fn raw_value(self) -> &'static str {
        match self {
            DownscaleMethod::Nearest => "nearest",
            DownscaleMethod::NearestAA => "nearest+",
            DownscaleMethod::Bilinear => "bilinear",
            DownscaleMethod::Bicubic => "bicubic",
            DownscaleMethod::Lanczos => "lanczos",
            DownscaleMethod::Area => "area",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            DownscaleMethod::Nearest => "Nearest",
            DownscaleMethod::NearestAA => "Nearest+",
            DownscaleMethod::Bilinear => "Bilinear",
            DownscaleMethod::Bicubic => "Bicubic",
            DownscaleMethod::Lanczos => "Lanczos",
            DownscaleMethod::Area => "Area",
        }
    }

    /// `nearest` and `area` run as one dispatch; the rest are separable
    /// two-pass filters whose kernel support scales with the ratio.
    pub fn is_separable(self) -> bool {
        !matches!(self, DownscaleMethod::Nearest | DownscaleMethod::Area)
    }

    /// Filter radius in filter-space, matching the `DEF_DOWNSCALE_1D`
    /// instantiations in the Metal source: tent +/-1, Mitchell +/-2,
    /// lanczos3 +/-3.
    pub fn radius(self) -> f32 {
        match self {
            DownscaleMethod::NearestAA | DownscaleMethod::Bilinear => 1.0,
            DownscaleMethod::Bicubic => 2.0,
            DownscaleMethod::Lanczos => 3.0,
            DownscaleMethod::Nearest | DownscaleMethod::Area => 0.0,
        }
    }
}

impl std::str::FromStr for DownscaleMethod {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        DownscaleMethod::ALL
            .into_iter()
            .find(|m| m.raw_value() == s)
            .ok_or(())
    }
}

/// Optional pre-shader downscale step: the retro horizontal resolution the
/// CRT shader sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownscaleSpec {
    pub width: u32,
    pub height: u32,
    pub method: DownscaleMethod,
}

impl DownscaleSpec {
    /// Height always follows the source's aspect ratio, as in the Downscale
    /// panel: the user picks a width, the height is derived.
    pub fn for_width(width: u32, source_width: u32, source_height: u32, method: DownscaleMethod) -> Self {
        let height = if source_width == 0 {
            width
        } else {
            let h = (width as f64 * source_height as f64 / source_width as f64).round() as u32;
            h.max(1)
        };
        DownscaleSpec { width: width.max(1), height, method }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn raw_values_round_trip() {
        for m in DownscaleMethod::ALL {
            assert_eq!(DownscaleMethod::from_str(m.raw_value()), Ok(m));
        }
    }

    #[test]
    fn nearest_plus_keeps_its_swift_spelling() {
        assert_eq!(DownscaleMethod::NearestAA.raw_value(), "nearest+");
        assert_eq!(DownscaleMethod::from_str("nearest+"), Ok(DownscaleMethod::NearestAA));
    }

    #[test]
    fn separable_set_matches_the_metal_kernels() {
        assert!(!DownscaleMethod::Nearest.is_separable());
        assert!(!DownscaleMethod::Area.is_separable());
        for m in [DownscaleMethod::NearestAA, DownscaleMethod::Bilinear,
                  DownscaleMethod::Bicubic, DownscaleMethod::Lanczos] {
            assert!(m.is_separable(), "{m:?} should be separable");
            assert!(m.radius() > 0.0);
        }
    }

    #[test]
    fn derived_height_preserves_aspect_ratio() {
        // 1920x1080 narrowed to 320 wide -> 180 tall.
        let s = DownscaleSpec::for_width(320, 1920, 1080, DownscaleMethod::Area);
        assert_eq!((s.width, s.height), (320, 180));
        // 4:3 SNES width.
        let s = DownscaleSpec::for_width(256, 640, 480, DownscaleMethod::Nearest);
        assert_eq!((s.width, s.height), (256, 192));
    }

    #[test]
    fn degenerate_sources_do_not_produce_zero_sizes() {
        let s = DownscaleSpec::for_width(320, 0, 0, DownscaleMethod::Area);
        assert!(s.width >= 1 && s.height >= 1);
        let s = DownscaleSpec::for_width(0, 1920, 1080, DownscaleMethod::Area);
        assert!(s.width >= 1 && s.height >= 1);
    }

    #[test]
    fn spec_serialises_with_preset_compatible_names() {
        let s = DownscaleSpec { width: 320, height: 240, method: DownscaleMethod::NearestAA };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"nearest+\""), "got {json}");
        let back: DownscaleSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
