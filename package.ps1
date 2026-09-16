<#
.SYNOPSIS
Build a distributable VHS-Studio: a zip that runs from any folder.

.DESCRIPTION
Runs the tests, builds release binaries, then stages everything the app
looks for beside its executable (see vhs-studio-app/src/presets.rs):

    VHS-Studio-<version>-windows-x64/
        vhs-studio.exe          the app
        vhs-studio-smoke.exe    headless verifier
        ffmpeg.exe          video decode/encode, a pinned build (see below)
        ffprobe.exe
        av*.dll sw*.dll     ffmpeg's libraries
        presets/            the bundled look presets
        licenses/           FFmpeg's licence and where its source is
        README.md GUIDE.md  the front page and the full guide

and a VHS-Studio-<version>-windows-x64.zip.sha256 beside the zip.

The CRT shaders are not staged as files: build.rs packs the seven presets
and everything they #include or sample (about 70 files of the submodule's
5,000) into one compressed blob inside vhs-studio.exe, which unpacks it to
the user's cache directory on first run (see vhs-studio-app/src/shader_pack.rs).
The submodule is still needed here, to build. A shaders/ directory beside
the exe, if someone adds one, takes precedence over the pack.

ffmpeg is bundled so the zip runs on a machine with nothing installed and
so every install decodes and encodes identically (HEIC needs 7.1+; an old
PATH copy would silently lose it). The build is pinned in
ffmpeg-bundle.json — release tag, asset name and SHA-256 — and downloaded
from BtbN/FFmpeg-Builds on GitHub, verified, and its bin/ contents (minus
ffplay) staged beside vhs-studio.exe, which is the first place the app looks.
It is a GPL build (the app's H.264/HEVC exports need libx264/libx265), run
as a separate process; its licence ships in licenses/.

.PARAMETER Out
Where to put the staged folder and the zip. Default: dist/ in the repository.

.PARAMETER NoFFmpeg
Leave ffmpeg out of the zip. The app then needs ffmpeg on PATH for video
and HEIC, as before.

.PARAMETER SkipTests
Skip `cargo test --release`. For iterating on the packaging itself.

.PARAMETER SkipBuild
Reuse target/release as it stands.

.EXAMPLE
pwsh ./package.ps1
#>
[CmdletBinding()]
param(
    [string]$Out = (Join-Path $PSScriptRoot 'dist'),
    [switch]$SkipTests,
    [switch]$SkipBuild,
    [switch]$NoFFmpeg
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repo = $PSScriptRoot
$shadersSource = Join-Path $repo 'Vendor/slang-shaders'
$presetsSource = Join-Path $repo 'presets'

# build.rs embeds the shader pack from the submodule; without it the exe
# would build with an empty pack and find no shaders on a user's machine.
if (-not (Test-Path (Join-Path $shadersSource 'crt'))) {
    throw "Vendor/slang-shaders is empty. Run: git submodule update --init --depth 1 Vendor/ntsc-rs Vendor/slang-shaders"
}

Push-Location $repo
try {
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $version = ($meta.packages | Where-Object name -eq 'vhs-studio-app').version
    if (-not $version) { throw 'could not read the vhs-studio-app version from cargo metadata' }

    if (-not $SkipTests) {
        Write-Host '== cargo test --release ==' -ForegroundColor Cyan
        cargo test --release
        if ($LASTEXITCODE -ne 0) { throw "tests failed ($LASTEXITCODE)" }
    }
    if (-not $SkipBuild) {
        Write-Host '== cargo build --release ==' -ForegroundColor Cyan
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "build failed ($LASTEXITCODE)" }
    }

    # `$IsWindows` is PowerShell 6+; this also runs under Windows PowerShell 5.
    $exeSuffix = if ($env:OS -eq 'Windows_NT') { '.exe' } else { '' }
    $bin = Join-Path $repo 'target/release'
    foreach ($name in 'vhs-studio', 'vhs-studio-smoke') {
        if (-not (Test-Path (Join-Path $bin "$name$exeSuffix"))) {
            throw "target/release/$name$exeSuffix is missing; build first or drop -SkipBuild"
        }
    }

    $stageName = "VHS-Studio-$version-windows-x64"
    $stage = Join-Path $Out $stageName
    $zip = Join-Path $Out "$stageName.zip"

    Write-Host "== staging $stage ==" -ForegroundColor Cyan
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    if (Test-Path $zip) { Remove-Item -Force $zip }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    Copy-Item (Join-Path $bin "vhs-studio$exeSuffix") $stage
    Copy-Item (Join-Path $bin "vhs-studio-smoke$exeSuffix") $stage
    Copy-Item (Join-Path $repo 'README.md') $stage
    Copy-Item (Join-Path $repo 'docs/GUIDE.md') $stage

    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'presets') | Out-Null
    Copy-Item (Join-Path $presetsSource '*.json') (Join-Path $stage 'presets')

    if (-not $NoFFmpeg) {
        Write-Host '== bundling ffmpeg ==' -ForegroundColor Cyan
        $pin = Get-Content (Join-Path $repo 'ffmpeg-bundle.json') -Raw | ConvertFrom-Json
        $url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$($pin.release)/$($pin.asset)"
        # Cached under target/ (ignored by git) so repeated packaging doesn't
        # re-download 90 MB; the digest check below decides whether the cached
        # file is usable.
        $cacheDir = Join-Path $repo 'target/ffmpeg-bundle'
        New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
        $archive = Join-Path $cacheDir $pin.asset
        $fresh = $false
        if (Test-Path $archive) {
            $have = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLower()
            $fresh = ($have -eq $pin.sha256.ToLower())
            if (-not $fresh) { Write-Host "   cached archive has the wrong digest; re-downloading" }
        }
        if (-not $fresh) {
            Write-Host "   downloading $url"
            Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
        }
        $have = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLower()
        if ($have -ne $pin.sha256.ToLower()) {
            throw "ffmpeg archive digest mismatch: expected $($pin.sha256), got $have"
        }
        Write-Host "   sha256 ok: $have"

        $unpack = Join-Path $cacheDir 'unpacked'
        if (Test-Path $unpack) { Remove-Item -Recurse -Force $unpack }
        Expand-Archive -Path $archive -DestinationPath $unpack
        $ffRoot = Get-ChildItem -Path $unpack -Directory | Select-Object -First 1
        if (-not $ffRoot) { throw 'ffmpeg archive had no top-level folder' }
        $ffBin = Join-Path $ffRoot.FullName 'bin'
        foreach ($name in 'ffmpeg.exe', 'ffprobe.exe') {
            if (-not (Test-Path (Join-Path $ffBin $name))) { throw "ffmpeg archive is missing bin/$name" }
            Copy-Item (Join-Path $ffBin $name) $stage
        }
        # ffplay is not used and is the one thing in bin/ we'd otherwise
        # ship for nothing.
        Get-ChildItem -Path $ffBin -Filter '*.dll' | ForEach-Object { Copy-Item $_.FullName $stage }

        $licenses = Join-Path $stage 'licenses'
        New-Item -ItemType Directory -Force -Path $licenses | Out-Null
        Copy-Item (Join-Path $ffRoot.FullName 'LICENSE.txt') (Join-Path $licenses 'FFmpeg-LICENSE.txt')
        @(
            "FFmpeg $($pin.version) ($($pin.asset))"
            ''
            'ffmpeg.exe, ffprobe.exe and the av*/sw* DLLs beside vhs-studio.exe are FFmpeg,'
            "licensed under the $($pin.license) (see FFmpeg-LICENSE.txt). They are an"
            'unmodified build from https://github.com/BtbN/FFmpeg-Builds'
            "(release $($pin.release)); FFmpeg's source is at https://ffmpeg.org and"
            'the build scripts are in that repository. VHS-Studio runs ffmpeg as a separate'
            'program for video decoding/encoding and HEIC/AVIF stills; it does not link'
            'to it.'
            ''
            'To use a different ffmpeg, delete these files and put ffmpeg/ffprobe on PATH,'
            'or set VHS_STUDIO_FFMPEG and VHS_STUDIO_FFPROBE to the executables.'
        ) | Set-Content (Join-Path $licenses 'FFmpeg-NOTICE.txt')
        $shipped = (Get-ChildItem -Path $stage -File | Where-Object { $_.Name -match '\.(dll|exe)$' -and $_.Name -notmatch '^vhs-studio' } | Measure-Object -Property Length -Sum).Sum
        Write-Host ("   staged {0} MB of ffmpeg" -f [math]::Round($shipped / 1MB))
    }

    # Prove the staged executable carries its shaders: the pack must be the
    # source (not the Vendor/ tree it could find by walking up from dist/),
    # and all seven presets must resolve from it. A fresh cache directory so
    # the unpack itself runs here, not just the "already there" path.
    Write-Host '== checking the embedded shaders unpack and resolve ==' -ForegroundColor Cyan
    $env:VHS_STUDIO_SHADERS = $null
    $env:VHS_STUDIO_PRESETS = $null
    $env:VHS_STUDIO_CACHE = Join-Path $Out 'cache-check'
    if (Test-Path $env:VHS_STUDIO_CACHE) { Remove-Item -Recurse -Force $env:VHS_STUDIO_CACHE }
    $listing = & (Join-Path $stage "vhs-studio-smoke$exeSuffix") --list-shaders
    if ($LASTEXITCODE -ne 0) { throw "vhs-studio-smoke --list-shaders failed ($LASTEXITCODE)" }
    $listing | ForEach-Object { Write-Host "   $_" }
    if ($listing -match 'MISSING') { throw 'a bundled shader does not resolve from the staged executable' }
    if (-not ($listing[0] -match 'embedded pack')) { throw "the staged executable did not use its embedded shader pack: $($listing[0])" }
    Remove-Item -Recurse -Force $env:VHS_STUDIO_CACHE
    $env:VHS_STUDIO_CACHE = $null

    # And that the app finds the bundled ffmpeg first — the lookup is "beside
    # the executable, then PATH", and this is the one place it is exercised
    # with a real staged copy. Windows only: the .exe cannot run elsewhere.
    if (-not $NoFFmpeg -and $env:OS -eq 'Windows_NT') {
        Write-Host '== checking the bundled ffmpeg is the one found ==' -ForegroundColor Cyan
        $env:VHS_STUDIO_FFMPEG = $null
        $env:VHS_STUDIO_FFPROBE = $null
        $report = & (Join-Path $stage "vhs-studio-smoke$exeSuffix") --ffmpeg
        if ($LASTEXITCODE -ne 0) { throw "vhs-studio-smoke --ffmpeg failed ($LASTEXITCODE): $report" }
        $report | ForEach-Object { Write-Host "   $_" }
        if (($report | Where-Object { $_ -match 'bundled beside the app' } | Measure-Object).Count -ne 2) {
            throw 'the staged ffmpeg/ffprobe were not the ones resolved'
        }
    }

    # The zip must run on a clean Windows install, so the executables may not
    # import the Visual C++ Redistributable. .cargo/config.toml links it
    # statically; this catches the setting being lost. Import names sit in the
    # PE as plain ASCII, so a byte search is enough.
    if ($env:OS -eq 'Windows_NT') {
        Write-Host '== checking for Visual C++ Redistributable imports ==' -ForegroundColor Cyan
        foreach ($name in 'vhs-studio', 'vhs-studio-smoke') {
            $bytes = [System.IO.File]::ReadAllBytes((Join-Path $stage "$name.exe"))
            $text = [System.Text.Encoding]::ASCII.GetString($bytes)
            foreach ($dll in 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll', 'MSVCP140.dll') {
                if ($text.IndexOf($dll, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    throw "$name.exe imports $dll - the C++ runtime is not linked statically"
                }
            }
            Write-Host "   $name.exe: no VC++ Redistributable imports"
        }
    }

    Write-Host "== zipping $zip ==" -ForegroundColor Cyan
    Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
    $mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
    Write-Host "wrote $zip ($mb MB)" -ForegroundColor Green

    # A digest beside the zip, in sha256sum's format, so a download can be
    # checked against the Release page.
    $digest = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    "$digest  $stageName.zip`n" | Set-Content -NoNewline "$zip.sha256"
    Write-Host "sha256 $digest"
}
finally {
    Pop-Location
}
