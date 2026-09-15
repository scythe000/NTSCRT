# VHS-Studio — handoff

Written for whoever picks this up next, human or model. It covers what the
port is, what state it's in, the decisions that aren't obvious from the code,
and the traps that will cost you an hour if nobody warns you.

**Branch:** `windows-port` on `github.com/scythe000/NTSCRT` (a fork of
`finnmckenty/NTSCRT`). 53 commits, ~13,000 lines of Rust/WGSL at the
repository root plus the release workflow in `.github/`. Everything below is
pushed.

**Name.** The app is **VHS-Studio**. It started as "NTSCRT for Windows" and
was renamed once it had grown features the macOS app doesn't have (the
colour grade, bundled ffmpeg, the About box) — the owner's call, on the
grounds that it is now its own program. Crates are `vhs-studio-core` and
`vhs-studio-app`, binaries `vhs-studio.exe` and `vhs-studio-smoke.exe`,
environment variables `VHS_STUDIO_*`, the zip `VHS-Studio-<version>-windows-x64`.
"NTSCRT" in this document and the code means the **macOS original** it came
from, which is credited in the README, the About box and
`vhs-studio-smoke --version`. Where you still see "the Mac" or "the macOS
app", that is NTSCRT. The icon is still NTSCRT's (`Assets/icon-source.png`)
— a VHS-Studio icon is on the to-do list. The preset file format is
unchanged and still moves between the two apps. The repository's **root
`README.md` is VHS-Studio's front page** (what GitHub renders for the
branch), with `docs/vhs-studio-header.webp` as its picture; NTSCRT's own
macOS app's own files are gone (see "The macOS sources are gone" below). The GitHub
"About" sidebar is the repository *description* in Settings, not a file —
it still reads as the macOS tool until the owner changes it.

The first 14 commits built the port headlessly. The next batch was a second
pass with a screen: the GUI was run on a Linux desktop, looked at, and fixed,
and the original "where to go next" list was done. The third pass closed the
feature gaps against the macOS app found by reading its source side by side
(§2, "Parity with the macOS app"). The latest pass is the first to go
*beyond* the Mac: a colour-grade stage with its own panel and presets, and
ffmpeg bundled in the zip. The owner has said cross-compatibility with the
macOS app is no longer a constraint — this is now its own program — but the
grade was still added in a way that keeps preset files loadable both ways.

---

## 1. What this is

VHS-Studio makes an image or video look like it's playing on a 1980s TV. It
runs media through [ntsc-rs](https://github.com/ntsc-rs/ntsc-rs) (analog
signal emulation), a colour grade, and then RetroArch's CRT shaders via
[librashader](https://github.com/SnowflakePowered/librashader). It is the
Windows descendant of [NTSCRT](https://github.com/finnmckenty/NTSCRT).

```
source → rotate → NTSC/VHS degradation (full res, CPU) → downscale → colour grade → CRT shader → output
```

The colour grade is this build's own stage; everything else is the Mac's
chain.

The original, NTSCRT, is a **macOS SwiftUI/Metal app**
([finnmckenty/NTSCRT](https://github.com/finnmckenty/NTSCRT)). This is a
**Windows rewrite**, not a port of the Swift.

**Why a rewrite:** the two libraries doing the image work are already
cross-platform Rust, so they carry over untouched. The ~9,000 lines of Swift
around them do not — SwiftUI, AppKit, Metal, AVFoundation, CoreImage and
ImageIO have no Windows equivalent. Swift has a Windows toolchain, but that
gets you the language, not the frameworks. So the libraries are reused and the
shell is rebuilt on **wgpu** + **egui**.

One thing is genuinely *simpler* than the original: macOS reaches both
libraries through an Objective-C bridge over a C ABI
(`Sources/CrtAppBridge`, `Vendor/ntscrs-capi`) because its host language is
Swift. Here the host is Rust, so both are ordinary crate dependencies. No
bridge, no dylib to ship, no header to keep in sync.

**The Swift is still the reference.** Every module here names its Swift
counterpart in its header comment (`Sources/CrtCore/Pipeline.swift` and so
on). Those paths are in the **upstream** repository now, not this one: the
macOS sources, tests, scripts, `Package.swift`, the Objective-C bridge and
`Vendor/ntscrs-capi` were removed once nothing here built against them
(last present at commit `105b74e`). When porting anything else, read the
upstream file first. Port *behaviour*, not frameworks.

### The macOS sources are gone

Removed, in one commit, as Mac-only: `Sources/`, `Tests/`, `Package.swift`,
`scripts/` (the Mac build/release/wrap scripts), `Vendor/ntscrs-capi` (the
C ABI the Swift called ntsc-rs through — Rust needs none), `Assets/AppIcon.icns`
(`icon-source.png` stays; `build.rs` makes the `.ico` from it), `DEVELOPMENT.md`
(Mac toolchain setup), NTSCRT's README and screenshots under `docs/`, a stray
`NTSCRT july 27.webp`, and a `CertificateSigningRequest.certSigningRequest`
that should never have been committed (a CSR carries no private key, but it
is still in history). `.gitignore` is now the Rust/Windows one. What the
Windows build depends on outside its own tree and stayed: `presets/`, the two
submodules, `Assets/icon-source.png`, `TestAssets/` (the smoke examples use
them), `docs/vhs-studio-header.webp`, `.github/`. Verified after the removal
by a clean clone → `cargo build --release` → `cargo test` → `package.ps1`
on the Windows runner.

**The `windows/` directory is gone too.** With the Mac app removed there was
nothing else at the root, so the Cargo workspace moved up: `Cargo.toml`,
`vhs-studio-core/`, `vhs-studio-app/`, `.cargo/`, `package.ps1`,
`ffmpeg-bundle.json` and this file live at the repository root, and the
full user guide (the old `windows/README.md`) is `docs/GUIDE.md`. `cargo`
and `package.ps1` run from the root; the workflow lost its
`working-directory: windows`. Relative paths that changed: the `ntsc-rs`
path dependency (`../Vendor/...`), `ICON_SOURCE` in `build.rs`, and the
artifact paths in the workflow (`dist/*.zip`).

---

## 2. Current state

| Area | State |
|---|---|
| Stills: load → NTSC → downscale → CRT → preview → PNG | ✅ verified |
| All 7 CRT shaders | ✅ verified rendering |
| All 6 downscale kernels | ✅ verified distinct + correct |
| Video decode / playback / frame cache | ✅ verified 24.2fps, 0 drops |
| MP4 / HEVC / ProRes 422 / ProRes 422 HQ / GIF export | ✅ verified with ffprobe |
| Rotation (90/180/270) | ✅ verified stills + video + export |
| App presets (load/save, 25 bundled) | ✅ all parse, format compatible |
| Keyframe timeline + interpolation | ✅ verified numerically |
| Timeline bar UI | ✅ looked at and driven on a Linux desktop (see below) |
| Keyframes animating during video playback / scrub | ✅ verified on screen |
| Export progress + cancel | ✅ in the toolbar, for movies *and* PNGs; cancel leaves no partial file |
| Threaded file opening with a busy indicator | ✅ on screen: toolbar spinner, preview card, status line; window stays live |
| Audio on video export | ✅ copied when the container allows, else AAC; looped, cut to length; ffprobe-checked |
| Preview zoom / pan / integer scale | ✅ pure `frame()` unit-tested + on screen |
| Icon, DPI manifest, zip, release workflow | ✅ workflow green on `windows-latest`: 130 tests, icon embedded without warnings, 65.8 MB zip artifact |
| CRT on/off, NTSC Reset, house defaults, per-shader saved params | ✅ on screen |
| Shader parameter presentation (headers, pickers, steppers, captions) | ✅ on screen against CRT Hyllian |
| Paste NTSC preset from the clipboard | ✅ on screen (copy → edit → paste restores) |
| Export sized by long edge | ✅ `export_output_size` unit-tested + panel shows it |
| HEIC / AVIF stills via ffmpeg | ✅ smoke-rendered with ffmpeg 7.0 (HEIC) and 6.1 (AVIF) |
| Edits while parked on a keyframe rewrite that key | ✅ on screen |
| Preview pacing (timeline at its fps, Animate at 30) | ✅ on screen: readout advances 0.5 s per 0.5 s |
| Preset load is a clean slate, incl. on a playing video | ✅ on screen: Glitch 1 → Clean VHS mid-playback |
| No VC++ Redistributable needed (crt-static) | ✅ CI check in `package.ps1` passes; `objdump -p` on the artifact shows no MSVCP140 / VCRUNTIME140 imports |
| Colour-grade stage (`vhs-studio-core/src/grade.rs`, `gpu/grade.{rs,wgsl}`, `ui/grade_panel.rs`) | ✅ GPU pass matches the CPU reference within 0.5/255; panel driven on screen; keyframes and presets carry it |
| Eight colour presets (B&W, Solarized, Inverted, Blade Runner, Max Headroom, Neon, Red/Cyan highlight) | ✅ smoke-rendered side by side and loaded in the GUI; now stackable colour layers (checkboxes) over any look |
| ffmpeg bundled beside the exe (pinned build, SHA-256 checked) | ✅ CI on `windows-latest`: digest ok, staged copy is the one resolved, ffmpeg 8.1.2 runs; 143 MB zip |
| About box (version, commit, date, libraries; Copy; F1) + version in the title + `vhs-studio-smoke --version` (adds ffmpeg) | ✅ on screen; report copied to the clipboard |
| Shaders embedded as one pack (70 files / 0.7 MB in the exe, replacing a 4,963-file `shaders/`) | ✅ all seven render byte-identically from the pack and from the full tree; unpack 5 ms once; `package.ps1` asserts the staged exe uses it; [CI run](https://github.com/scythe000/NTSCRT/actions/runs/35033655482) on `windows-latest` green from a clean clone, zip 143 → 96 MB |

**185 tests, zero warnings.** 62 in `vhs-studio-core`, 123 in `vhs-studio-app`.

### Parity with the macOS app

The Swift sources (`AppState.swift`, `ShaderPanel.swift`, `NtscPanel.swift`,
`ExportPopover.swift`, the menus) were read against the Rust and every
behavioural difference closed, except the ones listed under "Known gaps":

- **House defaults.** The Mac never showed ntsc-rs's raw defaults: it
  overlays `appNtscDefaults` (Finn's dialled-in VHS look) and
  `appShaderDefaults` (BOOST/GLOW_ROLLOFF/BLOOM_STRENGTH on the two Glow
  shaders). Windows was opening on the raw defaults, so the two apps looked
  different out of the box. Now `NtscStage::house()` /
  `HOUSE_DEFAULTS_JSON` in `vhs-studio-core/src/ntsc.rs` and
  `presets::HOUSE_SHADER_DEFAULTS` carry the same values; the app, the
  headless renderer and Reset all use them. The default shader is
  `glow_gauss`, as on the Mac (it was `royale`).
- **CRT on/off** (`shader_enabled`), round-tripped in preset JSON under
  `shader.enabled` — the field was already in the format, always `true`.
- **NTSC Reset** → house look. **CRT Reset** → house-or-declared defaults.
- **Per-shader saved parameters**: switching shaders stashes the outgoing
  values and restores them on return (`saved_shader_params`).
- **Auto-key while parked**: the Mac's `autoKeyIfParked`. Windows had the
  "click a key, change something, it edits in place" model described in
  `app_timeline.rs`'s header but never wired it — panel edits were discarded
  by the next scrub. `auto_key_if_parked` is now called by the NTSC and CRT
  panels after user edits (never by scrubbing or preset load).
- **Parameter presentation**: `presentation()` in `ui/shader_panel.rs` is a
  line-for-line port of `presentation(for:)`, with tests for each branch.
- **Declaration order**: librashader 0.12 hands parameters back in hash
  maps, so the panel was alphabetical and every header pseudo-param
  (`h_nonono`…) sorted to the bottom, leaving empty sections.
  `chain::declaration_order` walks the pass sources and their `#include`s
  for `#pragma parameter` lines to recover the author's order.
- **Paste preset** reads the clipboard with `arboard` (the Mac's
  `NSPasteboard`); it used to open a file dialog.
- **Export size** is the long edge, `export_output_size()` in `app.rs`.
- **HEIC/AVIF** decode through ffmpeg as a one-frame clip
  (`SourceImage::load_via_ffmpeg`); see the decision below.

### How the GUI was verified — and what that does and doesn't cover

The first sessions had no desktop, so the GUI was a guess. The second pass
built the app **on Linux** (an X11 desktop with Mesa's `lavapipe` software
Vulkan driver — see §4) and drove it with `xdotool`, reading back
screenshots. Every panel was looked at; the timeline bar in particular was
rewritten against `TimelineBar.swift` after that look. The fixes that came
out of it are the commits from `ab96348` on.

What that does **not** cover: nothing here has been run on a **Windows**
desktop since those changes. The D3D12 path, `rfd` file dialogs, DPI scaling
and the embedded icon were only ever exercised by the earlier, headless
Windows sessions (icon and manifest never — they're new). The packaged zip
from the workflow is the thing to run first.

### Known gaps

- **Not run on Windows since the GUI pass** — see above.
- **Zip only, unsigned.** No installer. SmartScreen blocks the downloaded
  exe ("Windows protected your PC") until the build has reputation, and a
  new build has none. The workflow has a **Sign** step ready: add the
  secrets `CODESIGN_PFX_BASE64` and `CODESIGN_PASSWORD` and it signs both
  exes with signtool (SHA-256, RFC 3161 timestamp) before packaging; with
  no secrets it does nothing. What is needed is the certificate: an OV
  certificate (≈$200–400/yr) still has to earn reputation; an EV one, or
  Azure Trusted Signing (≈$10/month, check current eligibility for
  individuals), is trusted at once. Until then the README tells users
  about *More info → Run anyway* and about unblocking the zip before
  extracting (Properties → Unblock), which removes the mark-of-the-web
  the extracted files would otherwise inherit.
- **The bundled ffmpeg has been run on a Windows *runner*, not a desktop.**
  [Run 34984713862](https://github.com/scythe000/NTSCRT/actions/runs/34984713862)'s
  Package step downloaded it, matched the digest, and `vhs-studio-smoke --ffmpeg`
  from the staged folder reported both tools "bundled beside the app" with
  `ffmpeg version n8.1.2-52-g5a03dfa0f6` — so the shared DLLs load. A real
  video decode/export with it on a desktop is still to do.
- **The grade runs after the signal stage only.** A "grade before NTSC"
  switch (so the tape records an already-tinted picture) was considered and
  left out: it would need a CPU implementation inside the playback
  producer, per-frame keyframe plumbing there, and cache invalidation on
  every colour edit — for a look that after-NTSC grading approximates
  closely. Add it only if someone asks for it.
- **HEIC needs ffmpeg ≥ 7.1.** The bundled 8.1.2 has it; a PATH copy older
  than that fails with a message naming the version.
- **`** CRT-HYLLIAN **`-style headers with no parameters under them** show
  as an empty collapsible section. The Mac does the same.
- **Not frame-identical to the Mac build,** and can't be. macOS pins
  librashader to `76462c03` because later versions shifted crt-royale's
  output; that pin is Metal-specific. This uses librashader 0.12 on its wgpu
  runtime. Output *is* deterministic here — same settings + same frame index =
  same pixels — just not bit-matched across backends. Don't chase this.
- **No undo.** Same as the original.

---

## 3. Layout

```
<repository root>
  Cargo.toml              workspace; pins wgpu 30 / eframe 0.36 / egui 0.36
  vhs-studio-core/            platform-neutral. No GPU, no windowing.
    ntsc.rs               ntsc-rs signal stage (replaces Vendor/ntscrs-capi)
    downscale.rs          method + spec model
    scanline.rs           supersample / snap math  (ScanlineGrid.swift)
    rotation.rs           quarter turns            (Windows addition)
    grade.rs              colour-grade model + CPU reference (Windows addition)
    timeline.rs           keyframes + interpolation (Timeline.swift)
    settings_ui.rs        re-exports ntsc-rs's settings schema
  vhs-studio-app/
    gpu/
      downscale.wgsl      the MSL kernels from Downscaler.swift, transliterated
      downscaler.rs       wgpu side of the downscale stage
      chain.rs            librashader filter chain (replaces LibrashaderBridge.m)
      grade.wgsl/.rs      colour-grade compute pass (Windows addition)
      pipeline.rs         frame orchestration       (Pipeline.swift)
    video/
      ffmpeg.rs           subprocess plumbing; tool lookup (beside exe → PATH)
      source.rs           decode / VideoSource      (VideoSource.swift)
      playback.rs         background producer       (PlaybackPipeline.swift)
      cache.rs            RAM preview               (ChainInputCache.swift)
      export.rs           MP4 / ProRes / GIF        (Mp4Exporter + GifExporter)
    ui/                   egui panels               (Views/*.swift); grade_panel.rs has no Swift twin
    app.rs                state + eframe shell      (AppState.swift)
    app_video.rs          video half of AppState
    app_load.rs           threaded file opening + the poll that applies it
    app_timeline.rs       keyframe half of AppState
    app_preset.rs         preset JSON load/save
    param_gates.rs        shader param gating       (ParamGates.swift)
    render.rs             HeadlessRenderer, FrameSequence — the export path
    bin/smoke.rs          vhs-studio-smoke, the headless verifier
    about.rs              build description (from build.rs env vars); ui/about_window.rs shows it
    presets.rs            shader catalog; shader root lookup incl. unpacking the embedded pack
    shader_pack.rs        preset file closure + pack format; compiled into build.rs too
    build.rs              shader pack, .ico from Assets/icon-source.png, DPI manifest, build info
  ffmpeg-bundle.json      the pinned ffmpeg build package.ps1 ships
  package.ps1             stage + zip, incl. the ffmpeg download and checks; also what CI runs
  .github/workflows/windows.yml   build, test, package; release on v* tags
```

Two binaries: `vhs-studio.exe` (the app) and `vhs-studio-smoke.exe` (the verifier).

In `ui/`: `mod.rs` holds the toolbar (including `export_control`, the
button-that-becomes-a-progress-bar, and `open_control`, the button that
gains a spinner while a file opens), status bar, export panel and the shared
`labelled_slider`; `preview_panel.rs` the preview with its pure `frame()`
geometry; `timeline_bar.rs` and `transport_bar.rs` the two bottom bars (they
share `paint_cached_runs` for the cached-frame strip).

---

## 4. Build and verify

```powershell
git submodule update --init --depth 1 Vendor/ntsc-rs Vendor/slang-shaders
cd windows
cargo build --release
cargo test --release
```

Requires Rust (MSVC toolchain), Visual Studio Build Tools with the C++
workload, and ffmpeg on PATH (this machine has it at `C:\ffmpeg\bin`).

**Always use `--release`.** The NTSC stage is CPU-bound and unusably slow
unoptimised; dependencies are optimised even in debug for the same reason.

```powershell
pwsh ./package.ps1                  # → dist/VHS-Studio-<version>-windows-x64.zip
```

The script runs the tests, builds, stages `vhs-studio.exe`, `vhs-studio-smoke.exe`,
`presets/`, the README and guide, and ffmpeg; runs `vhs-studio-smoke
--list-shaders` *from the staged folder* with a fresh cache directory to
prove the embedded shader pack unpacks and all seven presets resolve from
it (not from the `Vendor/` tree it could find by walking up from `dist/`);
and zips. The workflow does the same on
`windows-latest` and attaches the zip to a Release for a `v*` tag.

### Building on Linux, for looking at the GUI

Linux is a verification target only, but it is how the GUI got looked at.
On Ubuntu with an X11 desktop:

```bash
sudo apt-get install -y g++ libstdc++-13-dev mesa-vulkan-drivers ffmpeg xdotool
CXX=g++ CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc cargo build --release
./target/release/vhs-studio TestAssets/game-frame.png     # or VHS_STUDIO_SOURCE=…
```

- `Cargo.toml` enables eframe's `x11` feature for Linux only; without it
  winit has no backend and the build fails with "platform not supported".
- The `CXX`/linker overrides are for machines where `cc` is a symlink to
  clang that can't find `libstdc++` headers — librashader's `glslang-sys` and
  `spirv-cross-sys` are C++. Plain gcc toolchains don't need them.
- `mesa-vulkan-drivers` gives wgpu `lavapipe`, a software Vulkan device, so
  it runs with no GPU. Slowly, but correctly.
- `VHS_STUDIO_SOURCE=<file>` or a positional argument opens a file at launch;
  `VHS_STUDIO_EXPORT=<dest>` starts an export at launch so the progress UI can be
  seen without clicking through a file dialog.
- Drive it with `xdotool` (`mousemove x y click 1`, `keydown alt` + `click 4`
  for Alt+scroll, etc.) and capture with `ffmpeg -f x11grab`. Screenshot
  coordinates are in screen pixels; if you scale the shot down to read it,
  scale your clicks back up.

### vhs-studio-smoke is your most valuable tool

It drives the identical pipeline with no window, so you can verify work
without a screen. Learn it before changing anything.

```powershell
# Inventory
vhs-studio-smoke --list-shaders            # which shaders resolve here, and from where
vhs-studio-smoke --shader-files            # the embedded pack's manifest
vhs-studio-smoke --list-presets            # bundled presets and what they set
vhs-studio-smoke --list-params royale      # a shader's params, ranges, defaults
vhs-studio-smoke --timeline "Very wavy"    # evaluate a preset's animation
vhs-studio-smoke --timeline "Gltch 3" --watch head_switching_horizontal_shift

# Render
vhs-studio-smoke in.png out.png --shader royale --downscale 320 --height 960
vhs-studio-smoke in.png out.png --preset "Medium VHS" --rotate 90

# Video
vhs-studio-smoke clip.mp4 --video-info
vhs-studio-smoke clip.mp4 --playback 96           # throughput, drops, cache hits
vhs-studio-smoke clip.mp4 --export out.mp4 --format h264 --quality high
vhs-studio-smoke clip.mp4 --export out.gif --format gif --gif-width 480 --gif-fps 12
vhs-studio-smoke still.png --export out.mp4 --still-frames 48    # VHS motion
vhs-studio-smoke clip.mp4 --export out.mp4 --cancel-after 10     # cancel path
```

**A regression check that takes 30 seconds:**

```bash
for s in aperture easymode glow_gauss glow_lanczos hyllian royale sim; do
  vhs-studio-smoke TestAssets/game-frame.png /tmp/r-$s.png --shader $s --downscale 320 --height 720
done
vhs-studio-smoke /tmp/clip.mp4 --playback 72
```

---

## 5. Decisions that aren't obvious

These are the ones where the code looks arbitrary until you know why. Don't
"simplify" them without understanding the reason.

### Rotation happens *before* the NTSC stage

Not negotiable. NTSC is a **scanline** effect and a CRT draws horizontal
lines. Rotate afterwards and the scanlines swing round with the picture,
tilting the whole illusion. Rotating first means a portrait clip turned
landscape is degraded and scanned as if it had been shot that way — verified:
a 90° render comes out portrait with its scanlines still horizontal.

Consequence: `app.source` is **always pipeline-ready** (rotation already
applied), with the unrotated still kept in `app.original_source`. Video frames
are rotated in the producer *and* on the seek/first-frame paths — two places
that must stay in step. `RenderSettings.rotation` makes the headless renderer
do the turn, so don't also pass a pre-rotated source or you'll rotate twice.

### The timeline evaluator is built *after* the chain loads

It needs each shader parameter's declared `min`/`max`/`step` so interpolated
values land on legal steps. A parameter with `step: 1.0` over `0..1` is a
toggle; blending it to 0.4 is meaningless. Same reason booleans and enums
**hold** rather than lerp — a filter type of 1.5 doesn't exist.

### Keyframes are *master* keyframes

Each snapshots every animatable parameter at once. Parameters you don't touch
between two keys interpolate between equal values and hold still on their own.
That's what makes "dial in a look, press Keyframe, move, dial in another" work
without per-parameter tracks. Times are **proportional** (0..1), so changing
the duration stretches the animation rather than stranding keys past the end.

### Easing is per-key and shapes the segment *leaving* that key

CSS semantics. `Hold` returns 0 throughout, so the value snaps at the next key.

### `param_gates.rs` was verified empirically upstream

Every rule was checked with `crt-sweep`: the parameter shows ~zero pixel diff
across its range with the gate closed. These are facts about the shaders, not
the platform, so they port unchanged. All rule targets were re-validated
against the real shaders here — every name exists.

### Nothing slow runs on the UI thread — opening included

Opening a video was four subprocesses in a row (`ffmpeg -version`,
`ffprobe -version`, the probe, the first-frame decode) and opening a big
still a full decode, all inline in the click handler. On Windows, where a
process spawn is tens of milliseconds and antivirus can make it far more,
the window stopped painting for seconds and looked hung. `app_load.rs` now
does the decode on a thread and hands back plain data (`Loaded`) through a
channel that `ui()` polls; the UI-thread half (`apply_loaded_image`,
`apply_loaded_video`) is what the old `load_source`/`load_video` did after
the decode. The previous picture stays up until the swap. Opening a second
file while one is in flight replaces the task; the orphaned thread sends
into a dropped receiver and exits. PNG export got the same treatment — it
goes through `ExportTask` with `total = 1`, so the toolbar shows it as an
export and Cancel removes the file if it landed mid-render. What still runs
on the UI thread per frame is the *preview's* NTSC stage for a still (the
video path has its producer thread); at 4K that is a few hundred
milliseconds a frame with Animate on, which is the next thing to move if
anyone complains.

### Exports render every frame

Nothing is sampled from the playback cache, nothing is dropped under load. The
running frame index seeds the signal stage, so a **looped** export keeps
animating instead of repeating its noise.

### GIF gets its own width and rate

256 colours against full-frame analog noise makes files far larger than the
video codecs (~0.8 bytes/px/frame). Delays are stored in whole centiseconds,
so rates land on that grid — 12 fps plays at 12.5, 24 at 25, 30 at 33.3. 60 is
not offered because GIF can't reliably exceed 50. The palette is built across
the whole clip in one pass; a per-frame palette bands badly on noise.

### Presets always animate on load

Four of the bundled presets store `view.animate: false`. That's overridden —
these looks are built from tape noise and tracking error, and frozen on one
frame a preset shows a still that happens to be noisy rather than the effect
it's for. The flag still round-trips on save; it just doesn't decide what you
see.

### The preview's animations are paced by the clock, not the repaint

egui repaints as fast as the display allows. The timeline preview and
Animate used to advance one step per repaint, which tied their speed to the
monitor: a 2-second keyframe loop ran in 0.8 s at 60 Hz and 0.33 s at 144 Hz
(and crawled under lavapipe). That's what made 'Very wavy' look *less* wavy
than 'Gentle waves loop' — its wave is keyed from flat (both end keys are
the Clean-VHS values, `vhs_edge_wave` 0.5 at `bandwidth_scale` 0.3) up to a
peak mid-loop, and at that speed the peak was a flicker. `pacer.rs` now
schedules both the way the macOS app does: the timeline at its own fps
(`Task.sleep` to `timelineFPS` in `AppState.toggleTimelinePreview`), Animate
capped at 30 fps with NTSC on and 60 off (`preferredFramesPerSecond` in
`PreviewView`). Repaints are requested for when the next frame is due, so an
idle 24 fps loop no longer burns a CPU core repainting. When judging the
edge wave, remember ntsc-rs scales its shift by `bandwidth_scale`
(`intensity × 0.5 × horizontal_scale`), so compare the *product*: Gentle
waves is 6.3 × 0.39 ≈ 2.5 throughout, Very wavy peaks at 7.3 × 0.83 ≈ 6.1.

### Preset compatibility is a constraint, not a nicety

The `"version": 1` format is shared with the macOS build and files move both
ways. Rotation was added as a top-level `rotation` key in degrees — Swift's
decoder ignores keys it doesn't know, so that's safe. **Don't restructure the
format.** If you add a field, add it optional with a default.

The grade followed the same rule even though the owner has released the
Mac constraint: `grade` is an optional top-level section (`{enabled,
params}`), not written when it is the default, and keyframes carry an
optional `grade` map that is skipped when empty. A pre-grade preset loads
with the stage off; a graded preset loads on the Mac with the section
ignored.

### The grade is one flat parameter set, on the chain input

`vhs_studio_core::grade` defines the controls as a table (`GRADE_PARAMS`: name,
label, range, default, step) and the stage's state as `Grade { enabled,
values: BTreeMap<String, f32> }`. Flat named floats — the same shape as
shader parameters — so keyframes interpolate them, presets store them and
the panel renders them without any per-control code. Colours are three
floats apiece for the same reason; the panel turns them into colour
pickers. The kept hue for the colour highlight is stored as an *angle on
the NTSC I/Q plane*, which is what the shader compares hues on; the panel
shows a swatch at that angle and reads an angle back from whatever is
picked (`colour_for_hue` / `hue_for_colour`, radius chosen so no channel
clips, else the angle wouldn't round-trip).

The pass runs on the chain input — after NTSC and the downscale, before the
CRT — as one compute shader (`gpu/grade.wgsl`). That position was chosen
for three reasons: a tint reads as the *set* being lit that way; a
selective-colour highlight stays clean instead of being smeared by chroma
noise; and it is downstream of everything the video frame cache stores, so
a colour edit costs one chain-input-sized pass and invalidates nothing
(`set_grade_param` calls `mark_dirty`, not `mark_chain_input_edited`).
`Pipeline` records `last_chain_input` *before* the grade, so the cache
keeps ungraded frames.

`grade_pixel` in `vhs-studio-core` is a CPU reference of the shader, same math
in the same order. It is what the unit tests check presets against, and
the GPU was checked against it with a throwaway comparison (max 0.5/255).
**If you change one, change the other.** `RenderRequest::grade` is
`Option<[f32; 20]>`; callers pass `None` when `Grade::is_identity()`, so
every pre-grade preset skips the pass entirely.

### ffmpeg is bundled, pinned and verified

`video/ffmpeg.rs` resolves each tool as: `VHS_STUDIO_FFMPEG`/`VHS_STUDIO_FFPROBE`
override → a copy **beside our own executable** → the bare name on PATH.
The zip puts ffmpeg there. `package.ps1` reads `ffmpeg-bundle.json` (BtbN
release tag, asset name, SHA-256), downloads to `target/ffmpeg-bundle/`
(cached across runs), refuses a digest mismatch, and stages `bin/` minus
`ffplay.exe` plus `licenses/FFmpeg-LICENSE.txt` and a notice. It is the
*shared* GPL build: `ffmpeg.exe`/`ffprobe.exe` are small and the codecs are
in `av*.dll`, about half the size of the static build; GPL because the
H.264/HEVC exports use libx264/libx265, which the LGPL variant lacks. ffmpeg
is run as a separate process, so the app's own licence is unaffected. On
Windows the script then runs `vhs-studio-smoke --ffmpeg` from the staged folder
and requires both tools to report "bundled beside the app". `-NoFFmpeg`
skips all of it. To upgrade, change the three fields in the JSON — pick a
dated `autobuild-*` release, not the rolling `latest`, or the digest will
drift.

### On a video, the timeline *replaces* the transport bar

As on macOS. Both have a play button and a scrubber; showing both stacked
the controls twice. `transport_bar::show` returns early when
`timeline_open`, and the timeline bar carries the cached-frame strip
(`paint_cached_runs`, shared between the two).

### Keyframes on video: the producer gets a per-frame settings closure

Playback frames are made on a background thread, so the timeline can't be
applied on the UI thread as it is for stills. `playback::Config` carries an
optional `PerFrameJson` closure (`Arc<dyn Fn(usize) -> Option<String>>`)
built from the `TimelineEvaluator`, which is pure data and so `Send + Sync`.
The producer asks it for frame *n*'s NTSC JSON before degrading frame *n*.
Set it in **all three** places a config is pushed — `push_playback_config`,
`start_pipeline`, `tick_prerender` — or one path plays unanimated.

### Following the playhead must not invalidate the cache

`apply_timeline_at_playhead` marks the chain input edited (the user moved
something → re-render). `follow_video_frame` calls
`apply_timeline_state_at_playhead` instead, which applies the evaluated
settings **without** that mark. Playback and scrubbing go through the latter;
otherwise every consumed frame threw the RAM preview away.

### Preview framing is one pure function

`preview_panel::frame(panel, image, zoom, pan, integer) -> (Rect, pan)` does
aspect-fit, the integer-scale snap (rounding *down*, never below 1×, left as
a plain fit when even 1× won't fit), zoom about the centre, and clamps pan so
an image edge always reaches a panel edge. It returns the clamped pan so the
state can't drift while the image is not overflowing. Alt+scroll zooms about
the cursor by shifting pan by `(centre − cursor) · (new/old − 1)`. Unit-tested
like `PreviewGeometry.swift`; the interaction code just feeds it.

### Audio is copied when the container allows, cut with `-t`, never `-shortest`

`Mp4Exporter.swift` always re-encodes to AAC because AVFoundation's
passthrough is fragile. ffmpeg's isn't — `-c:a copy` is a remux — so the
track is copied untouched whenever the output container can hold its codec
(`ExportFormat::can_copy_audio`: the MPEG/Dolby family into MP4 and MOV, PCM
into MOV only) and re-encoded to the macOS settings (AAC 44.1 kHz stereo
128 kbps) otherwise — PCM or Vorbis into MP4, an unnamed codec. The status
line says which happened. The source file is ffmpeg's second input,
`-stream_loop (loops−1)` repeats it with the picture, and
**`-map 0:v:0 -map 1:a:0` is mandatory** — without it ffmpeg picks the
"best" video stream across inputs, which is the source, not the render.
`-t <picture length>` trims a long audio track; `-shortest` was rejected
because a track a few ms *shorter* than the picture would truncate video.
A copied track is cut at packet granularity, so it can run one audio frame
(~23 ms for AAC) past the last picture frame; a re-encoded one is exact.
On cancel ffmpeg is killed rather than allowed to finish the audio.

### A preset load is a clean slate

`apply_preset` ends with `mark_chain_input_edited`, not `mark_dirty`. The
difference only shows on a video: the playback producer holds a settings
snapshot and a per-frame keyframe closure, and the cache holds frames with
the NTSC stage baked in, so a plain dirty flag left the *previous* preset
playing back until the cache ran out or the next edit pushed a config. The
Mac's `loadLook` calls `noteChainInputEdited()` for the same reason. Shader
parameters the preset doesn't name go to their defaults first (the target
shader's stash is dropped; a same-shader load resets before applying), and
the bar follows the preset's `timeline.enabled`, forced open when keyed.
`load_preset` sets `active_preset` *after* applying, because the edit hooks
clear it.

### Colour presets are a layer; everything else is a snapshot

The owner asked for check marks so presets could be stacked. Two whole
presets can't be: each is a complete snapshot of downscale, NTSC, shader
and grade, and the animated ones carry all 78 values in every keyframe, so
"A then B" is simply B. What *can* stack is a grade over a look, because
the eight colour presets differ only in their `grade` section (they were
all built on Mild VHS). So `AppPreset` gained an optional `layer` field —
`"colour"` on those eight — and `bundled_classified()` files the bundled
presets as `Look`, `Animated` (has keyframes) or `Colour`. The menu shows
Looks and Animated as one radio group and Colour as checkboxes.

Applying a colour preset (`apply_colour_preset`) copies only its grade and
records it in `colour_layer: Option<(name, Grade)>`; the base tick stays.
`apply_preset` (a whole look) keeps the layer's grade in place when the
incoming preset carries no grade of its own — every bundled Look and
Animated preset is like that — and drops it when the preset does say what
its colour is. Editing a grade control by hand goes through `leave_grade`,
which clears the colour tick if there is one and the base tick otherwise;
edits elsewhere still clear only the base tick. Ticking the ticked colour
calls `remove_colour_layer`, grade back to off. Keyframes are untouched by
all this: the bundled animated presets' keys carry no grade values, so the
evaluator leaves the grade alone and the colour rides over the animation
(`grade_values` returns the empty map for keys without grade). The colour
files still contain the whole look they were built on, so the Mac and older
builds load them as before; `--preset` in the smoke tool is repeatable and
stacks a colour preset the same way. **Save colour as…** writes the current
state with `layer: colour`.

Neon was re-dialled at the same time: the first version was a purple duotone
at 0.45 that muddied everything and had the glow shader's strength at 0.1.
It is now saturation 2, contrast 1.35, blacks crushed, a faint blue-violet
in the shadows only, and (for whole-file loads) glow strength 0.7.

### The shaders are one pack inside the executable

Until the cleanup, `shaders/` beside the exe was the whole slang-shaders
tree — 4,963 files, 77 MB — because the `.slang` sources `#include` across
directories and nobody had computed which files the seven presets actually
reach. `shader_pack.rs` computes it: `closure()` walks each `.slangp`
(`#reference`, `shaderN =`, the `textures` list and each named texture's
path) and each source's `#include` lines recursively, normalising `..` so a
file reached two ways is one entry, and errors on a reference that doesn't
exist. `build.rs` includes that file with `#[path]` (it is std + flate2
only for that reason), runs the walk over `Vendor/slang-shaders`, and
writes the 70 files (1.2 MB; 0.7 MB compressed) as one deflate stream —
one stream rather than zip's per-file compression, because a hundred small
GLSL files share their vocabulary — to `OUT_DIR/shaders.pack`, which
`presets.rs` embeds with `include_bytes!`. Every file in the closure is a
`rerun-if-changed`, so editing a shader rebuilds the pack; a preset gaining
an include changes the `.slangp` or a `.slang` already in the list, so the
walk re-runs and picks it up — the failure mode the "ship everything"
approach was guarding against is now a build error, not a runtime one.

librashader resolves includes and textures from disk, so the pack has to
become files once: `presets::embedded_shaders` unpacks it into
`<cache>/shaders/<content-hash>/` (`%LOCALAPPDATA%\VHS-Studio` on Windows,
`~/.cache/vhs-studio` on Linux, `VHS_STUDIO_CACHE` to override), writing to
a `.tmp-<pid>` directory and renaming into place so a crash or two
instances racing never leaves a half tree that looks complete; a
`.complete` marker inside is what later launches check. Measured: 5 ms on
the first run, nothing after. The hash (`SHADER_PACK_ID`, FNV-1a of the
pack bytes, computed in `build.rs`) means a new build never reads a stale
extraction; old directories are not cleaned up (each is 1.2 MB). The hash
differs between a Linux and a Windows build of the same commit because the
Windows runner checks the submodule out with CRLF line endings; the
shaders compile the same either way, as they did when the tree shipped.

Lookup order in `shaders_root_and_source`: `VHS_STUDIO_SHADERS`, then
`shaders/` beside the exe, then the pack, then the walk up to
`Vendor/slang-shaders`. The pack comes *before* the source tree
deliberately: a dev build then exercises exactly what ships, and
`package.ps1`'s staged check asserts `--list-shaders` reports "embedded
pack" (the staged exe sits under `dist/`, inside the repo, so the walk-up
would otherwise succeed and prove nothing). A build without the submodule
gets a `cargo:warning` and an empty pack, and falls through to the tree.
`VHS_STUDIO_SHADER_PACK` at build time points `build.rs` at a different
checkout. Verified by rendering all seven shaders from the pack and from
`VHS_STUDIO_SHADERS=Vendor/slang-shaders` and byte-comparing the PNGs:
identical. `vhs-studio-smoke --shader-files` prints the manifest.

### House defaults live in `vhs-studio-core`, as data

`AppState.appNtscDefaults` was reproduced as `HOUSE_DEFAULTS_JSON`, a
partial ntsc-rs preset overlaid on the library defaults. ntsc-rs's own
`from_json` fills *missing* keys from its **legacy** values, not its
defaults, so the overlay is merged into a full default JSON first and that
is loaded. A test asserts every house key names a real setting in the
vendored ntsc-rs, because an unknown key is silently ignored and the look
would drift from the Mac's without anyone noticing.

### HEIC goes through ffmpeg, not a decoder crate

No pure-Rust HEIC decoder exists; `libheif` bindings would add a C library
to ship and to build in CI. ffmpeg is already a hard requirement for video
and demuxes HEIF since 7.1, so a HEIC (or AVIF) still is opened as a
one-frame `VideoSource` and `frame_at_index(0)` is the image. `image` is
tried first; ffmpeg is used only for extensions `image` doesn't handle or
when `image` reports `Unsupported`, so a broken PNG still reports `image`'s
error, not an ffmpeg one.

### Shader parameter order comes from the sources

See "librashader 0.12" in §6. `declaration_order` walks each pass's `.slang`
and its `#include`s in order, first occurrence wins, unreadable files are
skipped (it only decides ordering; the loader reports real problems). The
Hyllian test (`hyllian_headers_precede_their_sections`) runs against the real
checkout and is skipped without one.

### The C++ runtime is linked statically

`.cargo/config.toml` sets `crt-static` for `x86_64-pc-windows-msvc`. Without
it the exe imports `MSVCP140.dll` / `VCRUNTIME140.dll` / `VCRUNTIME140_1.dll`
(librashader's glslang and spirv-cross bindings are C++), which is the
Visual C++ Redistributable — installed on most PCs by something else, absent
on a clean one, and the app dies on launch with a "missing DLL" dialog.
`package.ps1` byte-searches both exes for those names and refuses to zip if
they appear. To check an artifact from Linux: `objdump -p vhs-studio.exe | grep
"DLL Name"` — the `api-ms-win-crt-*` entries are the UCRT and are fine (part
of Windows 10). This was found by inspecting the CI artifact; the setting
was not verified on a clean Windows install.

### The build describes itself through `build.rs`

`about.rs` reads `VHS_STUDIO_GIT_HASH`, `VHS_STUDIO_BUILD_DATE`, `VHS_STUDIO_TARGET`
and four `VHS_STUDIO_DEP_*` variables that `build.rs` emits with
`cargo:rustc-env`. The hash is `git rev-parse --short=9 HEAD` with
`-dirty` appended when `git status --porcelain --untracked-files=no` (the
whole checkout, submodules included) is non-empty — so a build from an edited tree says so, which is the
whole point when the question is "which exe is this". `build.rs` emits
`rerun-if-changed` for `.git/HEAD` and the branch ref it points to, so a
new commit rebuilds the crate; a dirty→clean transition without a commit
does not, which is acceptable. Library versions are scraped from
`Cargo.lock` by text (`name = "…"` then the next line's `version`), not
from `cargo metadata`, to keep the build script dependency-free. Without
git (a tarball) everything says "unknown" and the build still succeeds.

The About box shows only this compile-time data. It used to probe ffmpeg
(`ffmpeg -version`, a subprocess) when opened and show the GPU adapter;
on a Windows desktop the probe made the box take a visible moment to
appear, so both went — the owner's call: static information only. The
live details stay available from `vhs-studio-smoke --version` and
`--ffmpeg`, which is where a bug report should get them.

### The icon is generated, not committed

`build.rs` renders `Assets/icon-source.png` (the macOS icon's source) to a
six-size PNG-compressed `.ico` in `OUT_DIR` on every host, and embeds it with
`winresource` only when the target is Windows. Both failure modes are soft —
a missing asset or a missing `rc.exe` warns and the binary still links.

---

## 6. Traps

Things that actually cost time in the sessions that built this.

### egui 0.36 is not egui 0.31

The API changed substantially and most tutorials/training data are stale:

- `SidePanel` and `TopBottomPanel` are **gone** — unified into `Panel`
  (`Panel::left(id)`, `Panel::top(id)`, `Panel::bottom(id)`).
- Panels take **`&mut Ui`**, not `&Context`: `Panel::top("x").show(ui, |ui| …)`.
- `eframe::App::update(&mut self, ctx, frame)` is now
  **`ui(&mut self, ui: &mut Ui, frame: &mut Frame)`**. Get the context with
  `ui.ctx().clone()`.
- `CentralPanel::show` also takes `&mut Ui`.
- `InputState::raw_scroll_delta` → `smooth_scroll_delta`.
- `DroppedFile` is a trait now: `f.path()` (method), not `f.path` (field).
- Panel builders: `.default_size()` / `.size_range()`, not `.default_width()` /
  `.width_range()`.
- **`key_pressed` coalesces.** It reports whether the key went down at all
  this frame, once, with the modifiers as they are *at the end* of the frame.
  A fast Shift+Left arrived as one unshifted step. Iterate `i.events` and
  match `Event::Key { pressed: true, modifiers, .. }` — each event carries its
  own modifiers and repeats are counted. Both bottom bars do this.
- **`DragValue::max_decimals(4)` prints integers as `0.0`.** It overrides the
  integer formatting. `labelled_slider` only applies it for non-integral
  types.
- **Reserve space for things that may appear.** The timeline bar grew when
  the first keyframe added an easing chip, shifting every control above it —
  which made the "Keyframe" button move under a scripted click. The chip
  row's height is now always reserved.

### Drag identity across a re-sort

`move_keyframe` re-sorts by time, so a dragged key's index changes the
moment it crosses another. Holding the index across frames broke the drag.
The bar stores the dragged index in `egui` temp memory and re-points it to
the index `move_keyframe` returns after every move.

### The desktop can eat your input

On XFCE, Alt+scroll is the **window manager's** desktop magnifier
(`xfwm4 zoom_desktop`); the app never saw it and the whole screen zoomed
instead. `xfconf-query -c xfwm4 -p /general/zoom_desktop -s false`, then
`xfwm4 --replace` to clear an active zoom. If a shortcut "does nothing",
check whether the desktop took it before debugging the app.

### wgpu 30 differences

- `Instance::new(desc)` takes the descriptor **by value**.
- `InstanceDescriptor` has no `Default` — use
  `InstanceDescriptor::new_without_display_handle()`.
- `RequestAdapterOptions` needs `apply_limit_buckets`.
- `DeviceDescriptor` needs `experimental_features`.
- `PipelineLayoutDescriptor` has `immediate_size: u32`, **not**
  `push_constant_ranges`, and `bind_group_layouts: &[Option<&BindGroupLayout>]`.
- `PollType::Wait` is a struct variant — use `PollType::wait_indefinitely()`.
- `BufferSlice::get_mapped_range()` returns a `Result`.
- `PipelineCompilationOptions.constants` is `&[(&str, f64)]`.

### librashader 0.12

- `ShaderFeatures` is **not** re-exported through the facade. Depend on
  `librashader-common` directly and use
  `librashader_common::shader_features::ShaderFeatures`.
- `enable_cache: true` requires `wgpu::Features::PIPELINE_CACHE` on the device.
  Without it wgpu treats it as a **fatal validation error**, not a soft
  fallback. Check `adapter.features()` and pass the flag through.
- To get parameter metadata you must go **preset → pack → chain**, not
  `load_from_path`. The `#pragma parameter` data only exists on the pack.
- Parameter order is **lost**: `ShaderSource.parameters` and
  `get_parameter_meta` are hash maps. If you need the author's order (the
  panel's section headers do) read the sources — `chain::declaration_order`.

### Editing these files with Python heredocs

Most edits here were made with `python - <<'PYEOF'` scripts. Two real hazards:

1. **Rust `\u{2026}` escapes break Python string literals.** Use raw strings
   (`r'''…'''`) for any block containing them, or you'll get
   `SyntaxError: truncated \uXXXX escape`.
2. **Windows path backslashes get mangled.** Writing `.\\target\\release\\` in
   a normal Python string produced literal tab and carriage-return characters
   in the README once. Build paths with `chr(92)` or use forward slashes.

Also: a `sub()` helper that asserts before writing means **one failed match
aborts the whole script and writes nothing** — check the output actually says
it succeeded before assuming the edit landed.

### Label rows must lay the controls out first

`ui::label_row` exists because a truncating `Label` measures against the
*whole* row if it is added before the right-aligned controls, and the
controls then draw over its tail ("Internal Resolution Scale (downsa− li"
was the symptom). Right-to-left: controls, then the label in the space left.
`labelled_slider` and the CRT stepper both use it.

### Verify tests actually ran

A `cargo test | grep` that swallows output once hid a **non-compiling test
suite** through a commit. `0f61fa6` is that fix. Check the test-result line is
really there, not just that the command exited 0.

### Don't trust PSNR on these renders

Comparing two presets with PSNR is misleading — the output is dominated by
random noise, so obviously-different presets can score *more* similar than
near-identical ones. Compare settings numerically, or look at the images.

---

## 7. Where to go next

The original seven-item list (look at the GUI, cached-frame strip, timeline
polish, toolbar export progress, zoom/pan/integer scale, audio, packaging)
is done. What's left, roughly in order of value:

1. **Run the zip on a Windows desktop.** Take the artifact from the
   [first green run](https://github.com/scythe000/NTSCRT/actions/runs/34926158311)
   (or any later one) and check what Linux couldn't: the D3D12 backend, the file
   dialogs, DPI scaling on a HiDPI monitor, the embedded icon in Explorer and
   the taskbar, and the `.mov`/`.mp4` audio in a Windows player. Then tag
   `v0.1.0` and the workflow publishes the Release.
2. **Side-by-side with the macOS app.** The GUI was checked against the
   Swift *source*, not a running Mac. Someone with both should compare the
   timeline bar, the preview framing and the panel layout.
3. **Code signing**, so SmartScreen stops warning. The workflow's Sign
   step is in place; it needs the certificate secrets (see Known gaps).
4. **An installer** (MSIX or Inno Setup) with a Start-menu entry and file
   associations. The zip is deliberately the first cut — it needs nothing.
5. **Undo.** Neither build has it. Presets are the current workaround.
6. **Chain cache.** The Mac keeps compiled chains per shader
   (`chainCache`) so switching back is instant; Windows recompiles
   (crt-royale takes a couple of seconds). Parameter values already survive
   the switch, so this is purely time.
7. **An icon of its own.** `build.rs` still renders NTSCRT's
   `Assets/icon-source.png`; point `ICON_SOURCE` at a VHS-Studio one.
8. **More grade controls, if wanted.** Candidates: posterize, vignette,
   a "grade before the signal stage" switch (see Known gaps for its cost),
   and a proper HSV hue for the colour highlight instead of the I/Q angle.

---

### The grade has a CPU twin — keep them in step

`gpu/grade.wgsl` and `vhs_studio_core::grade::grade_pixel` are the same
function twice. The tests only see the CPU one. A change to the shader that
isn't mirrored passes every test and silently renders differently from what
the presets were tuned against. Check with the smoke tool: render with
`--no-ntsc --no-shader --downscale off` (pixels map 1:1) with and without a
`--grade`, and compare against `grade_pixel` on the ungraded PNG.

### The I/Q hue is not an HSV hue

The colour highlight's `keep_hue` is an angle on the NTSC I/Q chroma plane.
Orange sits on the +I axis (~0°), red at ~20°, so a red band 30° wide
catches orange too. That is by design (it's how the TV sees colour) but it
surprises anyone expecting HSV degrees. The panel hides the number behind a
swatch for this reason.

## 8. Working agreements that produced this code

Worth keeping, because the codebase is consistent about them:

- **Comments explain *why*, not *what*.** Density matches the surrounding
  code, which is fairly high — this project has a lot of non-obvious
  reasoning in it.
- **Every ported module names its Swift counterpart** in the header.
- **Tests cover pure logic thoroughly** — easing, interpolation, scanline
  math, rotation, preset round-trips, GIF timing. GPU paths are verified with
  `vhs-studio-smoke` instead.
- **Never claim something was verified if it wasn't.** §2 says exactly what
  the GUI pass covered (Linux, software Vulkan, `xdotool`) and what it
  didn't (a Windows desktop). Keep following it — a handoff that overstates
  what works is worse than one that admits gaps.
- **Look before you fix.** Every GUI change in the second pass started from
  a screenshot of the actual bug and ended with a screenshot of the fix.
  Cheap on Linux with `ffmpeg -f x11grab`; there is no reason to guess.
- `cargo build --release` and `cargo test --release` stay **clean, warnings
  included.**
