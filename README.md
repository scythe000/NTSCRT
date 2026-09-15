# VHS-Studio

![VHS-Studio — the full VHS + CRT pipeline on the left of the compare split, untouched source on the right](docs/vhs-studio-header.webp)

**Make any image or video look like it's playing off a worn tape on a 1980s TV.** VHS-Studio runs your media through a real analog signal emulation ([ntsc-rs](https://github.com/ntsc-rs/ntsc-rs) — composite artifacts, tape noise, head switching, tracking errors), an optional colour grade, and then through RetroArch's CRT shaders (via [librashader](https://github.com/SnowflakePowered/librashader) — scanlines, phosphor masks, glow), on the GPU, for Windows.

```
your image/video → NTSC/VHS signal degradation (full res) → downscale to retro resolution → colour grade → CRT shader → screen / file
```

Full disclosure: **this is two much better projects hacked together, plus a lot of glue.** All of the actual image magic belongs to ntsc-rs and the RetroArch shader community. VHS-Studio is the desktop app that connects them into one pipeline, with video playback, keyframe animation, exports and presets around it.

## Download

Grab `VHS-Studio-<version>-windows-x64.zip` from [**Releases**](../../releases/latest), unzip it anywhere, and run `vhs-studio.exe`. Nothing to install: no runtime, no ffmpeg to find — a pinned FFmpeg is in the zip.

**Requirements:** Windows 10 or 11, 64-bit, with a GPU that supports Direct3D 12 or Vulkan (anything from the last decade).

> **"Windows protected your PC".** The download isn't code-signed yet, so SmartScreen shows this the first time you run it. Click **More info → Run anyway**; it asks once per build. To avoid it altogether, right-click the zip before extracting → **Properties** → tick **Unblock** → OK: that removes the "downloaded from the internet" mark that SmartScreen keys on, and the extracted files inherit the cleared state. (Extracting with 7-Zip has the same effect.) A signed build is on the list — see the note in [`HANDOFF.md`](HANDOFF.md).

The **[full guide](docs/GUIDE.md)** covers every panel and control. In short:

- **Open** (Ctrl+O) an image (PNG, JPEG, WebP, HEIC, AVIF…) or a video (MP4, MOV, MKV…), or drop one on the window.
- **Presets** — 25 bundled looks from *Clean CRT* to *Obliterated*, including eight colour looks (*Black & white*, *Solarized*, *Inverted*, *Blade Runner*, *Max Headroom*, *Neon*, two colour highlights) and eight that animate. Save your own as JSON.
- **Sidebar** — the pipeline in signal order: Source, Downscale, NTSC (TV) with ntsc-rs's sixty-odd settings, Colour, and CRT with seven RetroArch presets and every runtime parameter.
- **Timeline** — keyframe the entire chain (After Effects-style master keyframes, easing per key) and render the animation; on a video the keyframes pin to moments in the clip.
- **Export** (Ctrl+E) — PNG for stills; H.264/HEVC MP4, ProRes MOV (audio comes along) or GIF for video, at your size and quality, with progress and Cancel. Exports are deterministic.
- **Compare** splits the preview against the untouched source; **Animate** keeps the tape noise moving in the preview at NTSC's 30 fps.

## Where it comes from

VHS-Studio began as the Windows build of [**NTSCRT**](https://github.com/finnmckenty/NTSCRT), Finn McKenty's macOS app that first wired ntsc-rs and librashader together. It owes NTSCRT the pipeline, the house VHS look, the preset format, the keyframe model, the bundled presets and most of its design — and the two still share preset files. SwiftUI and Metal don't exist on Windows, so the app was rebuilt on [wgpu](https://wgpu.rs) and [egui](https://github.com/emilk/egui) in Rust; it then grew things the Mac app doesn't have (the colour-grade stage, bundled ffmpeg, an About box, and so on) and became its own program with its own name.

The macOS app itself lives upstream at [finnmckenty/NTSCRT](https://github.com/finnmckenty/NTSCRT); its Swift sources were removed from this repository once VHS-Studio no longer built against them (they are in this repository's history up to commit `105b74e` if ever needed).

## Repository layout

| Path | What |
|---|---|
| `vhs-studio-core/`, `vhs-studio-app/` | **VHS-Studio** — the Rust workspace: the platform-neutral pipeline maths, and the wgpu/egui app with its headless verifier |
| `package.ps1`, `ffmpeg-bundle.json` | the packaging script CI runs, and the pinned FFmpeg build it bundles |
| [`HANDOFF.md`](HANDOFF.md) | the whole story of the code, for whoever works on it next |
| `presets/` | the 25 bundled presets (the format is NTSCRT's, so files move between the two apps) |
| `Vendor/ntsc-rs`, `Vendor/slang-shaders` | git submodules: the signal-emulation crate the app builds against, and the RetroArch shader tree it ships |
| `Assets/` | `icon-source.png`, from which `build.rs` generates the Windows icon |
| `TestAssets/` | two small test frames for `vhs-studio-smoke` |
| `docs/` | the [full guide](docs/GUIDE.md) and the README's header image |

## Building from source

```powershell
git clone --recurse-submodules https://github.com/scythe000/NTSCRT
cd NTSCRT
cargo build --release
.\target\release\vhs-studio.exe
```

Needs a Rust toolchain (`rustup`, stable) and the Visual Studio Build Tools' C++ workload; the app links the C runtime statically, so the result runs on any Windows machine. `.\package.ps1` stages a release folder with ffmpeg and zips it — the same script CI runs. `vhs-studio-smoke.exe` is a headless verifier that renders frames and reports what it finds, handy for bug reports. See [the guide](docs/GUIDE.md#build) for details and [HANDOFF.md](HANDOFF.md) for the whole story of the code.

## Limitations

- The NTSC stage runs on the CPU at your source's full resolution. With **Animate** on, 4K sources drop the preview frame rate; exports render every frame regardless.
- No undo — save a preset before big experiments.
- The download is unsigned, so SmartScreen warns once.
- Not frame-identical to the macOS NTSCRT (different shader runtime backend); deterministic on its own.

## Credits

- [NTSCRT](https://github.com/finnmckenty/NTSCRT) by Finn McKenty — the original app this grew out of
- [ntsc-rs](https://github.com/ntsc-rs/ntsc-rs) — the NTSC/VHS signal emulation (MIT/ISC/Apache-2.0)
- [librashader](https://github.com/SnowflakePowered/librashader) by SnowflakePowered — the RetroArch-compatible shader runtime (MPL-2.0)
- [libretro/slang-shaders](https://github.com/libretro/slang-shaders) and the RetroArch community — the CRT shaders themselves: crt-royale by TroggleMonkey, crt-easymode and crt-aperture by EasyMode, crt-hyllian by Hyllian, crtsim, crtglow (various licenses, largely GPL)
- [FFmpeg](https://ffmpeg.org) — decoding and encoding; the bundled build is from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (GPL)
- [wgpu](https://wgpu.rs) and [egui](https://github.com/emilk/egui) — the GPU and UI layers
