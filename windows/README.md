# NTSCRT for Windows

A native Windows build of [NTSCRT](../README.md): [ntsc-rs](https://github.com/ntsc-rs/ntsc-rs)
analog signal emulation followed by RetroArch's CRT shaders through
[librashader](https://github.com/SnowflakePowered/librashader), same pipeline
as the macOS app —

```
your image → NTSC/VHS signal degradation (full res) → downscale to retro resolution → CRT shader → screen
```

## Why this is a rewrite, not a port

The two projects doing the actual image work are already cross-platform Rust,
so they carry over directly. The app around them does not: the macOS build is
~9,000 lines of Swift against SwiftUI, AppKit, Metal, AVFoundation, CoreImage
and ImageIO, none of which exist on Windows. Swift has a Windows toolchain,
but that gets you the language, not the frameworks.

So the libraries are reused as-is and the shell is rebuilt on **wgpu** (D3D12
or Vulkan) and **egui**. In one respect this is simpler than the original: the
macOS build reaches both libraries through an Objective-C bridge over a C ABI
(`Sources/CrtAppBridge`, `Vendor/ntscrs-capi`) because its host language is
Swift. Here the host is Rust, so both are ordinary crate dependencies — no
bridge, no dylib to ship, no header to keep in sync.

What was ported rather than rewritten: the scanline-grid math
(`ScanlineGrid.swift`), the downscale kernels (the MSL in `Downscaler.swift`,
transliterated to WGSL with the same weight functions and ratio-scaled
support), and the shader parameter gating rules (`ParamGates.swift`) — those
last are facts about the shaders, not about the platform.

## Requirements

- Windows 10 1809 or later (the D3D12 backend needs render passes), x64
- A GPU with D3D12 or Vulkan drivers

Building additionally needs:

- [Rust](https://rustup.rs/) (stable, `x86_64-pc-windows-msvc`)
- Visual Studio Build Tools with the C++ workload — MSVC provides the linker

## Build

```powershell
# From the repository root. Both submodules are required: ntsc-rs is the
# signal stage, slang-shaders holds the CRT presets themselves.
git submodule update --init --depth 1 Vendor/ntsc-rs Vendor/slang-shaders

cd windows
cargo build --release
```

Two binaries land in `windows/target/release/`:

- `ntscrt.exe` — the app
- `ntscrt-smoke.exe` — headless verifier, the counterpart of the macOS `crt-smoke`

Use `--release`. The NTSC stage is CPU-bound and unusably slow unoptimised;
dependencies are optimised even in debug builds for the same reason.

## Using the app

**Toolbar** — **Open** (Ctrl+O) an image or video, **Export** (Ctrl+E), the
**Preset** menu, and the view controls. **Animate** runs the preview
continuously so tape noise, jitter and interlacing actually move — leave it on
for the real experience. **Compare** splits the preview: full pipeline left of
the line, untouched source right; drag the line to move the split. Alt+scroll
zooms.

**Presets** — save or load your entire configuration (downscale, NTSC, shader
and every shader parameter) as JSON, with the 17 bundled presets listed
underneath, with a tick against the loaded one that clears as soon as you
change anything it controls. Loading a preset turns **Animate** on whatever
the preset itself stores: these looks are built out of tape noise and
tracking error, and frozen on one frame a preset shows you a still that
happens to be noisy rather than the effect it is for. The format is the
macOS build's, so presets move between the two.
Eight of the bundled presets carry keyframes; this build has no timeline, so
it says so on load and **preserves them untouched when you save** rather than
quietly dropping someone's animation.

**Sidebar** — the creative pipeline, top to bottom in signal order:

- **Source** — the loaded file, and **Rotate** (also the toolbar button, or
  Ctrl+R). Rotation is applied before the effect, so a portrait clip turned
  landscape is degraded and scanned as if it had been shot that way, with
  scanlines still horizontal. Drag & drop onto the window works too.
- **Downscale** — the retro horizontal resolution the CRT shader sees (SNES
  256px, VGA 320px, or any custom width — height always follows your source's
  aspect ratio) and the resampling method. Nearest keeps pixels crunchy,
  Nearest+ keeps the punch without shimmering, Area is the smooth neutral choice.
- **NTSC (TV)** — the analog signal stage: composite noise, chroma bleed, head
  switching, tracking noise, tape speed, edge wave, and about sixty more. These
  controls are generated from ntsc-rs's own settings schema, so they track the
  library. Preset JSON is interchangeable with the
  [ntsc-rs desktop app](https://github.com/ntsc-rs/ntsc-rs/releases) — the 17
  presets in `presets/` load here.
- **CRT** — the seven bundled RetroArch presets with their runtime parameters.
  Grayed-out controls tell you which switch activates them; many CRT parameters
  only apply when their feature (curvature, mask, geometry mode) is on.
- **Export** — output height, with a scanline-grid warning when the result
  would band. A loaded video exports as H.264, HEVC, ProRes 422 / 422 HQ or
  GIF; a still exports a PNG, or video if you tick **Export as video (VHS
  motion)** — the signal stage animates on its own, so a still can make a
  clip without a timeline. GIF gets its own width and rate and estimates the
  file size before writing it. Movie exports run on their own thread with a
  progress bar in the panel and the status bar, and can be cancelled — a
  cancelled export removes its partial file.

**Video** — open a clip and a transport bar docks under the preview:
play/pause (Space), a frame-accurate scrubber, and a render bar showing how
far the RAM preview has got. Playback decodes and runs the signal stage on a
background thread and plays in real time, dropping a frame when a spike hits
rather than slowing down. Finished frames are cached, so a covered loop
replays with no per-frame CPU work and scrubbing inside it is instant. Any
change to the NTSC, downscale or shader settings starts it over.

**Scanline banding.** CRT shaders draw scanlines in *output* pixels, so if the
export height isn't a whole multiple of the downscale height, one source line
covers a fractional number of rows and the scanlines group into visible bands.
Exports handle this automatically by rendering at a whole multiple and
averaging down. **Snap size to scanline grid** takes the other route: it rounds
the output to the nearest size where every source line gets the same whole
number of rows, keeping scanlines crispest but changing your dimensions. As a
rule of thumb, crisp scanlines want 3+ output rows per downscale line.

## Headless verifier

```powershell
# Render an image through the full pipeline
.\target\release\ntscrt-smoke.exe input.png out.png --shader royale --downscale 320 --height 960

# Render with a bundled app preset
.\target\release\ntscrt-smoke.exe input.png out.png --preset "Medium VHS" --height 720

# Inventory
.\target\release\ntscrt-smoke.exe --list-shaders          # which shaders resolve here
.\target\release\ntscrt-smoke.exe --list-presets          # bundled presets and what they set
.\target\release\ntscrt-smoke.exe --list-params royale    # a shader's parameters, ranges and defaults
```

```powershell
# Video
.\target\release\ntscrt-smoke.exe clip.mp4 --video-info
.\target\release\ntscrt-smoke.exe clip.mp4 --playback 96      # throughput, drops, cache hits
.\target\release\ntscrt-smoke.exe clip.mp4 --export out.mp4 --format h264 --quality high
.\target\release\ntscrt-smoke.exe clip.mp4 --export out.gif --format gif --gif-width 480 --gif-fps 12
.\target\release\ntscrt-smoke.exe still.png --export out.mp4 --still-frames 48   # VHS motion
```

`--preset`, `--shader`, `--downscale <px|off>`, `--method`, `--height`,
`--snap`, `--no-ntsc`, `--ntsc-preset <file>`, `--frame <n>`, plus the video
flags above. Flags after `--preset` override it, so a preset works as a
starting point.

Useful for confirming a build renders correctly on a given adapter, for
byte-comparing output across revisions, and — via `--list-params` — for
checking that a shader's metadata reaches the UI.

## Asset locations

The app looks for the shader tree beside the executable (`shaders/`) and then
walks up to find `Vendor/slang-shaders`, so it works from both an install and
a source checkout. Override with `NTSCRT_SHADERS`; `NTSCRT_PRESETS` does the
same for the bundled `presets/` JSON.

## Differences from the macOS build

- **No keyframe timeline.** Video plays, scrubs and exports, but the
  keyframe animation the macOS build offers is not here. Presets that carry
  keyframes load and are preserved on save; they just don't animate.
- **No HEIC.** The `image` crate covers PNG/JPEG/BMP/TIFF/WebP; HEIC has no
  pure-Rust decoder. The macOS build gets it free from ImageIO.
- **Not frame-identical to the Mac build.** The macOS build pins librashader to
  76462c03 because later versions shifted crt-royale's output; that pin is
  Metal-specific. This uses librashader 0.12 on its wgpu runtime, so
  cross-backend bit-parity is not a goal Windows can meet. Output is still
  deterministic *here*: same settings and frame index give the same pixels.
- **No undo** — same as the original. Save presets before big experiments.

## Verified

On an RTX 3090 (Vulkan backend):

- All seven bundled CRT presets render.
- All six downscale kernels produce distinct, correct output.
- All 17 bundled app presets parse and render; "Clean CRT" and "Obliterated"
  produce the crisp and destroyed looks their names promise.
- Every rule in `param_gates.rs` names a parameter that exists in the real
  shaders (checked against `--list-params` for all seven).
- crt-royale, 320×240 → 1280×960, in 1.9s.
- Video playback: 24.2 fps against a 24 fps clip, zero drops, the frame cache
  serving every frame on the second loop. Same at 720p.
- All five export formats produce valid files (checked with `ffprobe`), GIF
  honouring its own width and rate. Loop 3 turns 48 frames into 144. A still
  exported as video genuinely animates.
- 88 tests pass (`cargo test --release`), no warnings.

The GUI launches and runs clean; its visual layout has not been checked against
the macOS app side by side.
