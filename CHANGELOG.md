# Changelog

Each `vX.Y.Z` tag publishes a GitHub Release whose notes are that version's
section below (the workflow extracts it), so a release starts by adding a
section here and bumping `version` in `Cargo.toml` and
`vhs-studio-core/Cargo.toml` to match.

Versions before 1.0.0 in this repository's tags are NTSCRT's, the macOS app
this grew out of; VHS-Studio's own numbering starts at 1.0.0.

## 1.0.0

First public release.

**Download** `VHS-Studio-1.0.0-windows-x64.zip`, unzip anywhere, run
`vhs-studio.exe`. Windows 10/11, 64-bit, a GPU with Direct3D 12 or Vulkan.
Nothing to install: the C runtime is linked in, FFmpeg is in the zip, the CRT
shaders are inside the executable. The `.sha256` beside the zip is its digest.

> **"Windows protected your PC"** — the download is not code-signed yet, so
> SmartScreen shows this the first time. Click *More info → Run anyway*.
> To avoid it, right-click the zip before extracting → Properties → tick
> *Unblock*.

**What it does.** Runs an image or a video through ntsc-rs's NTSC/VHS signal
emulation, an optional colour grade, and RetroArch's CRT shaders on the GPU,
and previews, animates and exports the result.

- **Sources:** PNG, JPEG, WebP, HEIC, AVIF and more; MP4, MOV, MKV and
  anything else FFmpeg reads. Drag and drop, or Ctrl+O. Files open on a
  background thread with a progress indicator.
- **Pipeline:** source → NTSC/VHS (ntsc-rs, sixty-odd settings) → downscale
  to a retro resolution (six kernels) → colour grade → CRT shader (seven
  RetroArch presets: Aperture, Easymode, Glow Gaussian/Lanczos, Hyllian,
  Royale, Sim, every runtime parameter exposed) → screen or file.
- **Colour grade:** hue shift, saturation, contrast, brightness, gamma,
  duotone tint, selective colour highlight, solarize, invert. Keyframable.
- **26 bundled presets** — looks from *Clean CRT* to *Obliterated*, eight
  animated ones, eight stackable colour looks (*Black & white*, *Solarized*,
  *Inverted*, *Blade Runner (screen)*, *Max Headroom*, *Neon*, *Red* and
  *Cyan highlight*) and *Blade Runner (film)*. Save your own as JSON; the
  format is NTSCRT's, so files move between the two apps.
- **Timeline:** master keyframes over the whole chain with per-key easing;
  on a video they pin to moments in the clip. Renders to video.
- **Export:** PNG for stills; H.264 / HEVC MP4, ProRes MOV (audio comes
  along) or GIF for video, sized by long edge, with progress and Cancel.
  Exports are deterministic.
- **Preview:** compare split against the untouched source, zoom, pan,
  integer scale, *Animate* to keep the tape noise moving at 30 fps.
- **About** (F1): version, commit, build date and library versions, with a
  Copy button for bug reports. `vhs-studio-smoke.exe` is a command-line
  verifier that renders the same pipeline headlessly.

**Origin.** VHS-Studio began as the Windows build of
[NTSCRT](https://github.com/finnmckenty/NTSCRT) by Finn McKenty, the macOS
app that first wired ntsc-rs and librashader together, and owes it the
pipeline, the house VHS look, the preset format and the keyframe model. It
was rebuilt on wgpu and egui, grew features the Mac app doesn't have, and
became its own program.

**Known limitations.** The NTSC stage runs on the CPU at the source's full
resolution, so 4K sources preview slowly with *Animate* on. No undo. The
download is unsigned. Output is not frame-identical to the macOS NTSCRT.
