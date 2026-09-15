# NTSCRT for Windows

A native Windows build of [NTSCRT](../README.md): [ntsc-rs](https://github.com/ntsc-rs/ntsc-rs)
analog signal emulation followed by RetroArch's CRT shaders through
[librashader](https://github.com/SnowflakePowered/librashader), same pipeline
as the macOS app —

```
your image → NTSC/VHS signal degradation (full res) → downscale to retro resolution → CRT shader → screen
```

> **Picking this up?** [HANDOFF.md](HANDOFF.md) has the current state, the
> decisions that aren't obvious from the code, and the API traps (egui 0.36,
> wgpu 30, librashader 0.12) that will otherwise cost you an hour each.

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
- [ffmpeg](https://ffmpeg.org/download.html) on PATH for video — decoding,
  playback and export all go through it. PNG/JPEG/BMP/TIFF/WebP stills need
  nothing extra; HEIC and AVIF stills are decoded through ffmpeg too (HEIC
  needs 7.1 or newer).

Building additionally needs:

- [Rust](https://rustup.rs/) (stable, `x86_64-pc-windows-msvc`)
- Visual Studio Build Tools with the C++ workload — MSVC provides the linker

## Install

Download `NTSCRT-<version>-windows-x64.zip` from the
[releases](https://github.com/scythe000/NTSCRT/releases), unzip it anywhere,
run `ntscrt.exe`. The folder is self-contained — `shaders/` and `presets/`
sit beside the executable — apart from ffmpeg, which you install yourself.

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

The build embeds the app icon (generated from `Assets/icon-source.png`, the
same file the macOS `.icns` comes from) and a per-monitor-v2 DPI manifest via
`build.rs`. Without the Windows SDK's `rc.exe` the binary still links; it just
gets the default icon.

### Package

```powershell
pwsh windows/package.ps1        # tests, builds, stages and zips
```

Produces `windows/dist/NTSCRT-<version>-windows-x64.zip` with both binaries,
the shader tree, the presets and this README, after checking that the seven
shaders resolve from the staged folder. The same script runs in CI: the
[Windows workflow](../.github/workflows/windows.yml) builds, tests and
packages on every push and pull request, uploading the zip as an artifact,
and attaches it to a GitHub Release when a `v*` tag is pushed.

## Using the app

**Toolbar** — **Open** (Ctrl+O) an image or video, **Export** (Ctrl+E), the
**Preset** menu, and the view controls. **Animate** runs the preview
continuously so tape noise, jitter and interlacing actually move — leave it on
for the real experience. **Compare** splits the preview: full pipeline left of
the line, untouched source right; drag the line to move the split. **Integer
scale** locks the preview to a whole multiple of the downscale so every
scanline is the same height on screen, letterboxing the rest. **Zoom** (or
Alt+scroll over the preview, which zooms about the cursor) magnifies; drag
(Space-drag or middle-drag when Compare is on) pans, and a double-click on
the preview or a click on the percentage resets both. While a video exports,
the Export button becomes a progress bar with a **Cancel** beside it.

**Presets** — save or load your entire configuration (downscale, NTSC, shader
and every shader parameter) as JSON, with the 17 bundled presets listed
underneath, with a tick against the loaded one that clears as soon as you
change anything it controls. Eight of them are keyframe-animated and play
their animation. Loading a preset turns **Animate** on whatever
the preset itself stores: these looks are built out of tape noise and
tracking error, and frozen on one frame a preset shows you a still that
happens to be noisy rather than the effect it is for. The format is the
macOS build's, so presets move between the two. Loading a keyframed preset
opens the timeline and starts it playing, as on macOS, so the animation is
what you see rather than one frame of it.

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
  library. The app opens on the same house VHS look as the macOS build (not
  ntsc-rs's raw defaults) and **Reset** returns to it. **Copy preset** and
  **Paste preset** move ntsc-rs preset JSON through the clipboard, so a look
  from the [ntsc-rs desktop app](https://github.com/ntsc-rs/ntsc-rs/releases)
  pastes straight in. Many settings sit inside collapsed groups (*VHS
  emulation › Edge wave*, *Scale*, …) — a preset that only changes those
  looks different in the preview without anything at the top of the panel
  moving.
- **CRT** — the seven bundled RetroArch presets with their runtime parameters,
  in the order the shader author declared them, under the author's own section
  headers. Each control is chosen from the parameter's declaration, as on the
  Mac: named choices ("SPHERE / CYLINDER") are pickers, integer ranges are
  steppers, unnamed 0/1s are checkboxes, the rest sliders, with any legend
  ("1-6 APERT, 7-10 DOT…") as a caption. **Apply CRT shader** bypasses the
  shader to show the signal stage on its own; **Reset** returns every
  parameter to the shader's defaults (including the app's house tweaks to
  the two Glow shaders). Switching shaders remembers each one's values.
  Grayed-out controls tell you which switch activates them; many CRT
  parameters only apply when their feature (curvature, mask, geometry mode)
  is on.
- **Export** — the output's long edge (the other side follows the source's
  aspect, as on the Mac), with a scanline-grid warning when the result would
  band. A loaded video exports as H.264, HEVC, ProRes 422 / 422 HQ or
  GIF; a still exports a PNG, or video if you tick **Export as video (VHS
  motion)** — the signal stage animates on its own, so a still can make a
  clip without a timeline. GIF gets its own width and rate and estimates the
  file size before writing it. A clip's audio comes along untouched — the
  track is copied when the output container can hold it (AAC, MP3, AC-3 and
  the like into MP4 or MOV; PCM into MOV), and re-encoded to AAC 44.1 kHz
  stereo only when it can't (PCM or Vorbis into MP4) — and is looped with
  the picture when **Loop** is more than 1; GIFs and stills are silent. Movie
  exports run on their own thread with progress in the toolbar and the
  export panel, and can be cancelled — a cancelled export removes its
  partial file.

**Animate (timeline)** — toggle **Timeline** in the toolbar to keyframe the
whole effect chain: scrub the playhead, dial in a look, press **Keyframe**,
move, dial in another. Everything keys together as one master keyframe, so
parameters you don't change between keys hold still on their own. Click a
diamond to jump to it — and while the playhead sits on a key, any parameter
you change rewrites that key in place, as in After Effects — drag it to retime (it snaps to frames), right-click
it for easing or delete, or nudge the selected one with Left/Right (Shift for
ten frames) and remove it with Delete. Each key's easing (linear, ease in,
ease out, ease in-out, hold) is also in the chip underneath it. Keyframe
times are proportional, so changing the duration stretches the whole
animation. Exports render the animation frame by frame. On a video the
timeline takes the transport bar's place, with the same play button,
scrubber and cached-frame strip under the ruler, and keyframed settings
animate during playback and scrubbing.

**Video** — open a clip and a transport bar docks under the preview:
play/pause (Space), a frame-accurate scrubber (Left/Right step a frame, Shift
for ten), and a render bar showing how far the RAM preview has got. Playback
decodes and runs the signal stage on a background thread and plays in real
time, dropping a frame when a spike hits rather than slowing down. Finished
frames are cached, so a covered loop replays with no per-frame CPU work and
scrubbing inside it is instant. Any change to the NTSC, downscale or shader
settings starts it over.

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

- **HEIC goes through ffmpeg.** The `image` crate covers PNG/JPEG/BMP/TIFF/
  WebP and has no HEIC or AVIF decoder, so those stills are decoded as a
  one-frame clip by the same ffmpeg video needs. HEIC demuxing arrived in
  ffmpeg 7.1; an older ffmpeg fails with a message saying so. The macOS build
  gets HEIC free from ImageIO.
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
- Audio: an AAC track is copied into MP4 (2 s clip → 2 s AAC; looped ×3 →
  6 s), PCM is copied into MOV and re-encoded to AAC for MP4, Vorbis is
  re-encoded for MP4; a silent clip, a still and a GIF export with no audio
  stream.
- Keyframes: 'Very wavy' sweeps `vhs_edge_wave` 0.5 -> 7.29 -> 0.5 across its
  48 frames (it was frozen at 1.64), and all 8 animated presets evaluate over
  real ranges. The animation bakes into exports frame by frame.
- A HEIC still renders through ffmpeg 7.0 (320×240 in, 1280×960 out), an
  AVIF through ffmpeg 6.1, and ffmpeg 6.1 refuses the HEIC with a message
  naming the version it needs.
- 155 tests pass (`cargo test --release`), no warnings.

On a Linux desktop (X11, Mesa's software Vulkan driver — a verification
target, not a shipping one), driving the window with `xdotool` and reading
screenshots: the sidebar, toolbar, transport bar, export progress and the
timeline bar were looked at and fixed where they were wrong. Keyframes add,
drag with snap, retime past each other, nudge, delete and take easing from
the chip and the context menu; the export button turns into a progress bar
and back; zoom anchors on the cursor, pan clamps to the image and Compare's
split still drags. The CRT panel's headers, pickers, steppers and captions
were checked against CRT Hyllian; the shader toggle shows the signal stage
as hard blocks; NTSC Reset and clipboard Paste both put a dragged slider
back; a parameter edited while parked on a keyframe survives jumping away
and back. 'Wavy loop' and 'Very wavy' load with their edge-wave values in
place (the settings they change live in collapsed groups). It has not been
run on a Windows desktop since these changes; the packaged zip is what the
workflow builds there.
