//! What build this is. The macOS app gets an About box from AppKit for free;
//! here it is ours, and it exists because "which exe am I running?" was a
//! real question — the file name is the same for every build.
//!
//! The constants come from `build.rs` (`cargo:rustc-env`). The runtime facts
//! that need a GPU or a subprocess (adapter, ffmpeg) are gathered by the
//! caller and passed in, so this module stays usable from the headless tool.

/// Facts fixed at compile time.
pub struct BuildInfo {
    pub version: &'static str,
    /// Short commit hash, `-dirty` appended when the tree had local edits;
    /// "unknown" when built outside git.
    pub commit: &'static str,
    pub date: &'static str,
    pub target: &'static str,
    pub profile: &'static str,
    pub ntsc_rs: &'static str,
    pub librashader: &'static str,
    pub wgpu: &'static str,
    pub egui: &'static str,
}

pub const BUILD: BuildInfo = BuildInfo {
    version: env!("CARGO_PKG_VERSION"),
    commit: env!("VHS_STUDIO_GIT_HASH"),
    date: env!("VHS_STUDIO_BUILD_DATE"),
    target: env!("VHS_STUDIO_TARGET"),
    profile: if cfg!(debug_assertions) { "debug" } else { "release" },
    ntsc_rs: env!("VHS_STUDIO_DEP_NTSC_RS"),
    librashader: env!("VHS_STUDIO_DEP_LIBRASHADER"),
    wgpu: env!("VHS_STUDIO_DEP_WGPU"),
    egui: env!("VHS_STUDIO_DEP_EGUI"),
};

pub const REPOSITORY: &str = "https://github.com/scythe000/NTSCRT";
/// The macOS app this grew out of. Credit where the pipeline came from.
pub const ORIGIN: &str = "https://github.com/finnmckenty/NTSCRT";
pub const ORIGIN_NAME: &str = "NTSCRT by Finn McKenty";

impl BuildInfo {
    /// `1.0.0 (6518ac9, 2026-09-15)` — the one line to quote in a bug report.
    pub fn short(&self) -> String {
        format!("{} ({}, {})", self.version, self.commit, self.date)
    }

    /// The report as text, for the clipboard and `vhs-studio-smoke
    /// --version`. Everything in it is known at compile time except
    /// `ffmpeg`, which the caller may have probed; the About box passes
    /// None (it stays static so it opens at once), the smoke tool probes.
    pub fn report(&self, ffmpeg: Option<&str>) -> String {
        let mut s = String::new();
        s.push_str(&format!("VHS-Studio {}\n", self.short()));
        s.push_str(&format!("target      {} ({})\n", self.target, self.profile));
        if let Some(f) = ffmpeg {
            s.push_str(&format!("ffmpeg      {f}\n"));
        }
        s.push_str(&format!("ntsc-rs     {}\n", self.ntsc_rs));
        s.push_str(&format!("librashader {}\n", self.librashader));
        s.push_str(&format!("wgpu        {}\n", self.wgpu));
        s.push_str(&format!("egui        {}\n", self.egui));
        s.push_str(REPOSITORY);
        s.push('\n');
        s.push_str(&format!("based on {ORIGIN_NAME}, {ORIGIN}\n"));
        s
    }
}

/// ffmpeg's `-version` first line, shortened to the version itself, plus
/// where it came from. Errors pass through as the message the probe gives.
pub fn describe_ffmpeg() -> String {
    use crate::video::ffmpeg::{probe_tools, resolve_tool};
    match probe_tools() {
        Ok(line) => {
            // "ffmpeg version n8.1.2-52-g5a03dfa0f6-20260914 Copyright ..." → "n8.1.2-52-g5a03dfa0f6-20260914"
            let version = line
                .strip_prefix("ffmpeg version ")
                .and_then(|r| r.split_whitespace().next())
                .unwrap_or(&line);
            let (_, source) = resolve_tool("VHS_STUDIO_FFMPEG", "ffmpeg");
            format!("{version}, {}", source.describe())
        }
        Err(e) => format!("not available — {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_is_described() {
        assert!(!BUILD.version.is_empty());
        assert!(!BUILD.commit.is_empty());
        // build.rs writes YYYY-MM-DD.
        assert_eq!(BUILD.date.len(), 10, "{}", BUILD.date);
        assert_eq!(BUILD.date.as_bytes()[4], b'-');
        // The lockfile has these, so "unknown" here means build.rs lost them.
        for (name, v) in [
            ("ntsc-rs", BUILD.ntsc_rs),
            ("librashader", BUILD.librashader),
            ("wgpu", BUILD.wgpu),
            ("egui", BUILD.egui),
        ] {
            assert_ne!(v, "unknown", "{name}");
            assert!(v.chars().next().is_some_and(|c| c.is_ascii_digit()), "{name} = {v}");
        }
    }

    #[test]
    fn the_report_carries_everything_a_bug_report_needs() {
        let r = BUILD.report(None);
        assert!(r.starts_with(&format!("VHS-Studio {}", BUILD.short())));
        assert!(!r.contains("ffmpeg"), "nothing unprobed is reported");
        assert!(BUILD.report(Some("n8.1.2, bundled")).contains("ffmpeg      n8.1.2, bundled"));
        assert!(r.contains(BUILD.librashader));
        assert!(r.contains(REPOSITORY));
        assert!(r.trim_end().ends_with(ORIGIN), "the origin is credited");
    }
}
