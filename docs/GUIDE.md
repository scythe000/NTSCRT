# VHS-Studio

**Make any image or video look like it's playing on a 1980s TV — on
Windows.** [ntsc-rs](https://github.com/ntsc-rs/ntsc-rs) analog signal
emulation (composite artifacts, tape noise, head switching), a colour grade,
and RetroArch's CRT shaders through
[librashader](https://github.com/SnowflakePowered/librashader) (scanlines,
phosphor masks, glow), in one pipeline:

```
your image → NTSC/VHS signal degradation (full res) → downscale to retro resolution → colour grade → CRT shader → screen
```

VHS-Studio began as the Windows build of
[**NTSCRT**](https://github.com/finnmckenty/NTSCRT), Finn McKenty's macOS
app that first wired these two libraries together, and it still owes it the
pipeline, the house VHS look, the preset format, the keyframe model, the
bundled presets and most of its design. It has since become its own program
— a colour-grade stage, bundled ffmpeg and other things the Mac app doesn't
have — so it has its own name. The macOS NTSCRT app lives upstream at
[finnmckenty/NTSCRT](https://github.com/finnmckenty/NTSCRT); this repository
is VHS-Studio. As with NTSCRT: all the actual image magic belongs to ntsc-rs and the RetroArch shader community.

> **Picking this up?** [HANDOFF.md](../HANDOFF.md) has the current state, the
> decisions that aren't obvious from the code, and the API traps (egui 0.36,
> wgpu 30, librashader 0.12) that will otherwise cost you an hour each.

## Why this is a rewrite, not a port

The two projects doing the actual image work are already cross-platform Rust,
so they carry over directly. The app around them does not: the macOS NTSCRT build is
~9,000 lines of Swift against SwiftUI, AppKit, Metal, AVFoundation, CoreImage
and ImageIO, none of which exist on Windows. Swift has a Windows toolchain,
but that gets you the language, not the frameworks.

So the libraries are reused as-is and the shell is rebuilt on **wgpu** (D3D12
or Vulkan) and **egui**. In one respect this is simpler than the original: the
macOS build reaches both libraries through an Objective-C bridge over a C ABI
(its `Sources/CrtAppBridge` and `Vendor/ntscrs-capi`, upstream) because its
host language is Swift. Here the host is Rust, so both are ordinary crate dependencies — no
bridge, no dylib to ship, no header to keep in sync.

What was ported rather than rewritten: the scanline-grid math
(`ScanlineGrid.swift`), the downscale kernels (the MSL in `Downscaler.swift`,
transliterated to WGSL with the same weight functions and ratio-scaled
support), and the shader parameter gating rules (`ParamGates.swift`) — those
last are facts about the shaders, not about the platform.

## Requirements

- Windows 10 1809 or later (the D3D12 backend needs render passes), x64
- A GPU with D3D12 or Vulkan drivers

Nothing else: no runtime to install. The C++ runtime is linked into the
executable, and the zip ships its own ffmpeg (a pinned build, see
[Package](#package)), which handles video decoding, playback and export and
HEIC/AVIF stills. PNG/JPEG/BMP/TIFF/WebP stills need nothing at all. If you
build from source instead, put [ffmpeg](https://ffmpeg.org/download.html)
7.1 or newer on PATH for video and HEIC, or drop `ffmpeg.exe` and
`ffprobe.exe` beside `vhs-studio.exe` — that is where the app looks first.

Building additionally needs:

- [Rust](https://rustup.rs/) (stable, `x86_64-pc-windows-msvc`)
- Visual Studio Build Tools with the C++ workload — MSVC provides the linker

## Install

Download `VHS-Studio-<version>-windows-x64.zip` from the
[releases](https://github.com/scythe000/NTSCRT/releases), unzip it anywhere,
run `vhs-studio.exe`. The folder is self-contained: `presets/`,
`ffmpeg.exe`/`ffprobe.exe` and their DLLs sit beside the executable, and the
CRT shaders are inside `vhs-studio.exe` itself (unpacked to your user cache
the first time it runs; see [Asset locations](#asset-locations)).

The zip holds two programs. `vhs-studio.exe` is the app. `vhs-studio-smoke.exe` is
a command-line tool that runs the same pipeline without a window — it
renders a file with a shader or preset, lists the bundled shaders, presets
and parameters, and reports on a video — which is how the build checks
itself (the packaging step runs it) and how a look can be reproduced
exactly for a bug report. You never need it to use the app; see
[Headless verifier](#headless-verifier) if you want it.

## Build

```powershell
# From the repository root. Both submodules are required: ntsc-rs is the
# signal stage, slang-shaders holds the CRT presets themselves.
git submodule update --init --depth 1 Vendor/ntsc-rs Vendor/slang-shaders

cargo build --release
```

Two binaries land in `target/release/`:

- `vhs-studio.exe` — the app
- `vhs-studio-smoke.exe` — headless verifier, the counterpart of the macOS `crt-smoke`

Use `--release`. The NTSC stage is CPU-bound and unusably slow unoptimised;
dependencies are optimised even in debug builds for the same reason.

The build embeds the app icon (generated from `Assets/icon-source.png`, the
same file the macOS `.icns` comes from) and a per-monitor-v2 DPI manifest via
`build.rs`. Without the Windows SDK's `rc.exe` the binary still links; it just
gets the default icon.

### Package

```powershell
pwsh ./package.ps1        # tests, builds, stages and zips
```

Produces `dist/VHS-Studio-<version>-windows-x64.zip` with both binaries,
the presets, the README, this guide and ffmpeg, after checking that the
seven shaders unpack and resolve from the staged executable's embedded pack
and that the staged ffmpeg is the one the app finds. The ffmpeg is a pinned build — release tag, asset name and
SHA-256 in `ffmpeg-bundle.json` — downloaded from
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), verified, and
staged as `ffmpeg.exe`, `ffprobe.exe` and the `av*`/`sw*` DLLs (the shared
build; the static one is twice the size). It is a GPL build because the
H.264/HEVC exports use libx264/libx265; the app runs it as a separate
program, and its licence and a notice ship in `licenses/`. `-NoFFmpeg` leaves
it out. ffmpeg is most of the zip (about 96 MB in all, now that the shaders
are inside the exe rather than a 77 MB tree); in return the app
runs on a machine with nothing installed, and every install decodes and
encodes with the same ffmpeg (an old PATH copy would silently lose HEIC). To
move to a newer ffmpeg, change the three fields in `ffmpeg-bundle.json`. The same script runs in CI: the
[Windows workflow](../.github/workflows/windows.yml) builds, tests and
packages on every push and pull request, uploading the zip as an artifact,
and attaches it to a GitHub Release when a `v*` tag is pushed.

## Using the app

**Toolbar** — **Open** (Ctrl+O) an image or video, **Export** (Ctrl+E), the
**Preset** menu, the view controls, and **About** (F1) at the right end —
version, commit and build date (also in the window title), GPU and backend,
which ffmpeg is in use and where it came from, the library versions, and a
link to the NTSCRT app this is based on, with a **Copy** button for bug
reports. `vhs-studio-smoke --version` prints the
same text. **Animate** runs the preview
continuously so tape noise, jitter and interlacing actually move — leave it on
for the real experience. It runs at 30 fps (NTSC's own rate) regardless of
your monitor's refresh rate, as does the timeline preview at the timeline's
fps, so a 2-second loop takes 2 seconds. **Compare** splits the preview: full pipeline left of
the line, untouched source right; drag the line to move the split. **Integer
scale** locks the preview to a whole multiple of the downscale so every
scanline is the same height on screen, letterboxing the rest. **Zoom** (or
Alt+scroll over the preview, which zooms about the cursor) magnifies; drag
(Space-drag or middle-drag when Compare is on) pans, and a double-click on
the preview or a click on the percentage resets both. While anything
exports — a movie or a PNG — the Export button becomes a progress bar with a
**Cancel** beside it; while a file is being opened, a spinner with its name
sits beside **Open** and a card in the middle of the preview says what is
happening. Neither blocks the window: the previous picture stays up and
every control keeps working until the new one is ready.

**Presets** — save or load your entire configuration (downscale, NTSC, colour
grade, shader and every shader parameter) as JSON, with the 25 bundled presets
listed underneath in three sections. **Looks** (*Clean CRT*, *Mild VHS*,
*Obliterated*…) and **Animated** (the keyframed ones — *Glitch 1*, *Very wavy*…)
are whole looks, one at a time, with a radio dot against the one on screen.
**Colour** presets (*Black & white*, *Solarized*, *Inverted*, *Blade Runner*,
*Max Headroom*, *Neon*, *Red highlight*, *Cyan highlight*) are only the Colour
panel and *stack*: tick one and it lays over whatever look is loaded, switch
the look underneath and it stays, tick it again to take it off. Any two whole
looks can't stack — each is a complete snapshot, and the animated ones carry
every value in every keyframe — which is why the menu draws the line where it
does. **Save colour as…** writes your own stackable colour preset from the
Colour panel. Marks clear as soon as you change something the preset controls.
Loading a preset turns **Animate** on whatever the preset itself stores: these
looks are built out of tape noise and tracking error, and frozen on one frame
a preset shows you a still that happens to be noisy rather than the effect it
is for. The format is the macOS build's, so presets move between the two (a
colour preset carries the look it was built on, so older builds load it
whole). Loading a keyframed preset opens the timeline and starts it playing,
as on macOS, so the animation is what you see rather than one frame of it. A
whole look is a clean slate: every setting it covers goes to the preset's
value or, where the preset is silent, to the default — nothing from the
previous preset or from your own tweaks carries over except a ticked colour,
and on a video the switch shows immediately rather than after the cached
frames run out.

**Sidebar** — the creative pipeline, top to bottom in signal order
(source → NTSC → downscale → colour grade → CRT shader → export):

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
- **Colour** — a grade on the degraded, downscaled picture before the CRT
  draws it: saturation (0 is black and white), contrast, brightness, gamma
  and hue shift; a **Tint** that pulls shadows and highlights toward two
  colours of your choosing (teal and amber for the Blade Runner look);
  **Colour highlight**, which keeps one colour and greys everything else
  (pick the colour, set how wide a band around it counts); and **Solarize**
  and **Invert**. Every control keyframes like the rest, so a picture can
  drain to black and white or flip negative over a loop. The grade runs after
  everything the video frame cache stores, so dialling a colour on a playing
  clip does not restart it. This stage is this build's own — the macOS app
  does not have it; a preset saved with a grade loads there with the section
  ignored.
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
  the picture when **Loop** is more than 1; GIFs and stills are silent. Every
  export — movie or PNG — runs on its own thread with progress in the toolbar
  and the export panel, and can be cancelled; a cancelled export removes its
  partial file. Opening a file is threaded the same way, so a slow decode
  (a 4K clip, a big HEIC) never freezes the window.

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
.\target\release\vhs-studio-smoke.exe input.png out.png --shader royale --downscale 320 --height 960

# Render with a bundled app preset
.\target\release\vhs-studio-smoke.exe input.png out.png --preset "Medium VHS" --height 720

# Inventory
.\target\release\vhs-studio-smoke.exe --list-shaders          # which shaders resolve here, and from where
.\target\release\vhs-studio-smoke.exe --shader-files         # the embedded shader pack's manifest
.\target\release\vhs-studio-smoke.exe --list-presets          # bundled presets and what they set
.\target\release\vhs-studio-smoke.exe --list-params royale    # a shader's parameters, ranges and defaults
.\target\release\vhs-studio-smoke.exe --list-grade            # the colour-grade controls
.\target\release\vhs-studio-smoke.exe --ffmpeg                # which ffmpeg/ffprobe the app will run, and from where
.\target\release\vhs-studio-smoke.exe --version               # the About box as text, plus the ffmpeg version and source
```

```powershell
# Colour grade: any control, repeatable; turns the stage on
.\target\release\vhs-studio-smoke.exe input.png out.png --preset "Mild VHS" --grade saturation=0 --grade contrast=1.1
```

```powershell
# Video
.\target\release\vhs-studio-smoke.exe clip.mp4 --video-info
.\target\release\vhs-studio-smoke.exe clip.mp4 --playback 96      # throughput, drops, cache hits
.\target\release\vhs-studio-smoke.exe clip.mp4 --export out.mp4 --format h264 --quality high
.\target\release\vhs-studio-smoke.exe clip.mp4 --export out.gif --format gif --gif-width 480 --gif-fps 12
.\target\release\vhs-studio-smoke.exe still.png --export out.mp4 --still-frames 48   # VHS motion
```

`--preset`, `--shader`, `--downscale <px|off>`, `--method`, `--height`,
`--snap`, `--no-ntsc`, `--no-shader`, `--grade name=value`, `--ntsc-preset <file>`,
`--frame <n>`, plus the video
flags above. Flags after `--preset` override it, so a preset works as a
starting point.

Useful for confirming a build renders correctly on a given adapter, for
byte-comparing output across revisions, and — via `--list-params` — for
checking that a shader's metadata reaches the UI.

## Asset locations

**Shaders.** The seven CRT presets and every file they `#include` or
sample — about 70 files, 0.7 MB compressed — are built into `vhs-studio.exe`
as one pack (`build.rs` walks the slang-shaders submodule at build time;
`vhs-studio-smoke --shader-files` lists what went in). librashader reads
shaders from disk, so on first run the pack is unpacked to
`%LOCALAPPDATA%\VHS-Studio\shaders\<hash>` (a few milliseconds, once per
build; `~/.cache/vhs-studio/shaders/` on Linux, `VHS_STUDIO_CACHE` moves it)
and that directory is used from then on. Rendering is unaffected — the
shaders compile from the same files as before. To use a different shader
tree, put a `shaders/` directory beside the executable or set
`VHS_STUDIO_SHADERS`; both take precedence over the pack. A source build
without the submodule embeds an empty pack and walks up to find
`Vendor/slang-shaders` instead. `VHS_STUDIO_PRESETS` overrides the bundled
`presets/` JSON. ffmpeg and ffprobe are looked for
beside the executable first (the zip puts them there), then on PATH;
`VHS_STUDIO_FFMPEG` and `VHS_STUDIO_FFPROBE` point at a specific executable.

## Differences from the macOS NTSCRT app

- **A colour-grade stage.** Between the downscale and the CRT shader, with
  its own **Colour** panel and eight presets built on it (see above). Presets
  carry it in a `grade` section the macOS build ignores; presets without one
  load here with the stage off. Keyframes carry a `grade` map the same way.
- **HEIC goes through ffmpeg.** The `image` crate covers PNG/JPEG/BMP/TIFF/
  WebP and has no HEIC or AVIF decoder, so those stills are decoded as a
  one-frame clip by the same ffmpeg video needs. HEIC demuxing arrived in
  ffmpeg 7.1; the bundled build has it, and an older ffmpeg from PATH fails
  with a message saying so. The macOS build gets HEIC free from ImageIO.
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
- All 25 bundled app presets parse and render; "Clean CRT" and "Obliterated"
  produce the crisp and destroyed looks their names promise. The eight colour
  looks were rendered side by side on a night-city test frame: black and
  white is grey, inverted is a negative, the two highlights keep only their
  colour, Blade Runner is teal and amber.
- The grade's GPU pass matches its CPU reference (`grade_pixel`) within
  0.5/255 across invert, a mixed grade (hue shift, saturation, contrast,
  gamma, tint, solarize) and the colour highlight.
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
- 185 tests pass (`cargo test --release`), no warnings.

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
place (the settings they change live in collapsed groups), and 'Very wavy'
plays its loop in real time — the readout advances 0.5 s per 0.5 s of wall
clock, flat at its end keys and clearly wavier than 'Gentle waves loop'
through the middle. The Colour panel enables, drags saturation to 0 and the
preview goes black and white through the CRT shader; loading 'Blade Runner'
tints the preview and fills the panel with its values, and loading 'Clean
CRT' after it turns the grade off and returns every control to neutral.
The About box opens from the button and from F1 with no delay (it is all
compile-time text; the ffmpeg probe it once ran lives in
`vhs-studio-smoke --version`), and Copy puts the report on the clipboard. Opening a
6000×6000 PNG shows the spinner beside Open, the "Opening big6k.png…" card
over the old picture and the status line, with the window still painting,
then swaps the picture in; opening a 4K clip lands on frame 1/120 with the
transport bar; a PNG export shows the animated "Exporting…" bar and Cancel
in the toolbar and ends with "Exported … (1920x1920, 2.3 MB)".
It has not been run on a Windows desktop since these changes; the packaged
zip is what the workflow builds there.
