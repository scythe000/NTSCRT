# NTSCRT for Windows — handoff

Written for whoever picks this up next, human or model. It covers what the
port is, what state it's in, the decisions that aren't obvious from the code,
and the traps that will cost you an hour if nobody warns you.

**Branch:** `windows-port` on `github.com/scythe000/NTSCRT` (a fork of
`finnmckenty/NTSCRT`). 14 commits, ~10,000 lines of Rust/WGSL under `windows/`.
Everything below is pushed.

---

## 1. What this is

NTSCRT makes an image or video look like it's playing on a 1980s TV. It runs
media through [ntsc-rs](https://github.com/ntsc-rs/ntsc-rs) (analog signal
emulation) and then RetroArch's CRT shaders via
[librashader](https://github.com/SnowflakePowered/librashader).

```
source → rotate → NTSC/VHS degradation (full res, CPU) → downscale → CRT shader → output
```

The original is a **macOS SwiftUI/Metal app** in `Sources/`. This is a
**Windows rewrite** in `windows/`, not a port of the Swift.

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

**The Swift is still the reference.** When porting anything else, read the
corresponding file in `Sources/` first. Every module here names its Swift
counterpart in its header comment. Port *behaviour*, not frameworks.

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
| App presets (load/save, 17 bundled) | ✅ all parse, format compatible |
| Keyframe timeline + interpolation | ✅ verified numerically |
| Timeline bar UI | ⚠️ **never looked at** |
| Export progress + cancel | ✅ verified cancel leaves no partial file |

**118 tests, zero warnings.** 52 in `ntscrt-core`, 66 in `ntscrt-app`.

### ⚠️ The big caveat

**Almost none of the GUI has been visually verified.** The sessions that built
this had no interactive desktop — screen capture failed with "handle is
invalid". Everything above was verified *headlessly*, through the identical
code path, via `ntscrt-smoke`. The app launches clean and renders correct
pixels, but nobody has looked at the sidebar, the transport bar, the export
panel, or the timeline bar.

**If you can see a screen, that is the single highest-value thing you can do.**
Run it, look at it, fix what's ugly or broken.

### Known gaps

- **Timeline bar is unverified visually** — diamonds, ruler spacing, easing
  chips, drag-to-retime. Most likely place for real bugs.
- **No cached-frame strip under the timeline ruler.** macOS has one; the
  transport bar already shows cache coverage, so it was skipped.
- **No HEIC.** The `image` crate covers PNG/JPEG/BMP/TIFF/WebP; HEIC has no
  pure-Rust decoder. macOS gets it free from ImageIO.
- **Not frame-identical to the Mac build,** and can't be. macOS pins
  librashader to `76462c03` because later versions shifted crt-royale's
  output; that pin is Metal-specific. This uses librashader 0.12 on its wgpu
  runtime. Output *is* deterministic here — same settings + same frame index =
  same pixels — just not bit-matched across backends. Don't chase this.
- **No undo.** Same as the original.

---

## 3. Layout

```
windows/
  Cargo.toml              workspace; pins wgpu 30 / eframe 0.36 / egui 0.36
  ntscrt-core/            platform-neutral. No GPU, no windowing.
    ntsc.rs               ntsc-rs signal stage (replaces Vendor/ntscrs-capi)
    downscale.rs          method + spec model
    scanline.rs           supersample / snap math  (ScanlineGrid.swift)
    rotation.rs           quarter turns            (Windows addition)
    timeline.rs           keyframes + interpolation (Timeline.swift)
    settings_ui.rs        re-exports ntsc-rs's settings schema
  ntscrt-app/
    gpu/
      downscale.wgsl      the MSL kernels from Downscaler.swift, transliterated
      downscaler.rs       wgpu side of the downscale stage
      chain.rs            librashader filter chain (replaces LibrashaderBridge.m)
      pipeline.rs         frame orchestration       (Pipeline.swift)
    video/
      ffmpeg.rs           subprocess plumbing
      source.rs           decode / VideoSource      (VideoSource.swift)
      playback.rs         background producer       (PlaybackPipeline.swift)
      cache.rs            RAM preview               (ChainInputCache.swift)
      export.rs           MP4 / ProRes / GIF        (Mp4Exporter + GifExporter)
    ui/                   egui panels               (Views/*.swift)
    app.rs                state + eframe shell      (AppState.swift)
    app_video.rs          video half of AppState
    app_timeline.rs       keyframe half of AppState
    app_preset.rs         preset JSON load/save
    param_gates.rs        shader param gating       (ParamGates.swift)
    render.rs             HeadlessRenderer, FrameSequence — the export path
    bin/smoke.rs          ntscrt-smoke, the headless verifier
```

Two binaries: `ntscrt.exe` (the app) and `ntscrt-smoke.exe` (the verifier).

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

### ntscrt-smoke is your most valuable tool

It drives the identical pipeline with no window, so you can verify work
without a screen. Learn it before changing anything.

```powershell
# Inventory
ntscrt-smoke --list-shaders            # which shaders resolve here
ntscrt-smoke --list-presets            # bundled presets and what they set
ntscrt-smoke --list-params royale      # a shader's params, ranges, defaults
ntscrt-smoke --timeline "Very wavy"    # evaluate a preset's animation
ntscrt-smoke --timeline "Gltch 3" --watch head_switching_horizontal_shift

# Render
ntscrt-smoke in.png out.png --shader royale --downscale 320 --height 960
ntscrt-smoke in.png out.png --preset "Medium VHS" --rotate 90

# Video
ntscrt-smoke clip.mp4 --video-info
ntscrt-smoke clip.mp4 --playback 96           # throughput, drops, cache hits
ntscrt-smoke clip.mp4 --export out.mp4 --format h264 --quality high
ntscrt-smoke clip.mp4 --export out.gif --format gif --gif-width 480 --gif-fps 12
ntscrt-smoke still.png --export out.mp4 --still-frames 48    # VHS motion
ntscrt-smoke clip.mp4 --export out.mp4 --cancel-after 10     # cancel path
```

**A regression check that takes 30 seconds:**

```bash
for s in aperture easymode glow_gauss glow_lanczos hyllian royale sim; do
  ntscrt-smoke TestAssets/game-frame.png /tmp/r-$s.png --shader $s --downscale 320 --height 720
done
ntscrt-smoke /tmp/clip.mp4 --playback 72
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

### Preset compatibility is a constraint, not a nicety

The `"version": 1` format is shared with the macOS build and files move both
ways. Rotation was added as a top-level `rotation` key in degrees — Swift's
decoder ignores keys it doesn't know, so that's safe. **Don't restructure the
format.** If you add a field, add it optional with a default.

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

Roughly in order of value:

1. **Look at the GUI.** Especially the timeline bar. This is the biggest
   unknown in the whole project.
2. **Cached-frame strip under the timeline ruler** (macOS has one).
3. **Timeline polish** — right-click a diamond to delete, snap-to-frame while
   dragging, keyboard nudge.
4. **Export progress in the toolbar**, as macOS does, so it's visible with
   every panel closed.
5. **Integer scale / zoom / pan in the preview.** `PreviewScaling.swift` and
   `PreviewGeometry.swift` were never ported; the preview currently fits to
   the window with Alt+scroll zoom only.
6. **Audio on video export.** Exports are silent: the only thing piped to
   ffmpeg is rawvideo frames, so there is no audio stream to carry. The macOS
   ProRes path muxes the source's audio through. Adding it means giving ffmpeg
   the original file as a second input and mapping its audio track.
7. **Package it.** No installer, no icon embedding (`winresource` is in
   `Cargo.toml` as a build-dep but there's no `build.rs` using it), no release
   workflow.

---

## 8. Working agreements that produced this code

Worth keeping, because the codebase is consistent about them:

- **Comments explain *why*, not *what*.** Density matches the surrounding
  code, which is fairly high — this project has a lot of non-obvious
  reasoning in it.
- **Every ported module names its Swift counterpart** in the header.
- **Tests cover pure logic thoroughly** — easing, interpolation, scanline
  math, rotation, preset round-trips, GIF timing. GPU paths are verified with
  `ntscrt-smoke` instead.
- **Never claim something was verified if it wasn't.** The GUI caveat in §2
  exists because that rule was followed. Keep following it — a handoff that
  overstates what works is worse than one that admits gaps.
- `cargo build --release` and `cargo test --release` stay **clean, warnings
  included.**
