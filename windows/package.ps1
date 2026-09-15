<#
.SYNOPSIS
Build a distributable NTSCRT for Windows: a zip that runs from any folder.

.DESCRIPTION
The Windows counterpart of scripts/make-release.sh. Runs the tests, builds
release binaries, then stages everything the app looks for beside its
executable (see ntscrt-app/src/presets.rs):

    NTSCRT-<version>-windows-x64/
        ntscrt.exe          the app
        ntscrt-smoke.exe    headless verifier
        shaders/            the slang-shaders tree (crt/, include/, blurs/, ...)
        presets/            the bundled look presets
        README.md

The whole slang-shaders tree ships rather than the seven presets' files
because .slang sources #include across directories (include/, misc/,
crt-effects/); a pruned copy breaks the moment a shader gains an include.
ffmpeg is not bundled — the README tells the user to install it — so the
zip carries no LGPL/GPL binaries of its own.

.PARAMETER Out
Where to put the staged folder and the zip. Default: dist/ under windows/.

.PARAMETER SkipTests
Skip `cargo test --release`. For iterating on the packaging itself.

.PARAMETER SkipBuild
Reuse target/release as it stands.

.EXAMPLE
pwsh windows/package.ps1
#>
[CmdletBinding()]
param(
    [string]$Out = (Join-Path $PSScriptRoot 'dist'),
    [switch]$SkipTests,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$windows = $PSScriptRoot
$repo = Resolve-Path (Join-Path $windows '..')
$shadersSource = Join-Path $repo 'Vendor/slang-shaders'
$presetsSource = Join-Path $repo 'presets'

if (-not (Test-Path (Join-Path $shadersSource 'crt'))) {
    throw "Vendor/slang-shaders is empty. Run: git submodule update --init --depth 1 Vendor/ntsc-rs Vendor/slang-shaders"
}

Push-Location $windows
try {
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $version = ($meta.packages | Where-Object name -eq 'ntscrt-app').version
    if (-not $version) { throw 'could not read the ntscrt-app version from cargo metadata' }

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
    $bin = Join-Path $windows 'target/release'
    foreach ($name in 'ntscrt', 'ntscrt-smoke') {
        if (-not (Test-Path (Join-Path $bin "$name$exeSuffix"))) {
            throw "target/release/$name$exeSuffix is missing; build first or drop -SkipBuild"
        }
    }

    $stageName = "NTSCRT-$version-windows-x64"
    $stage = Join-Path $Out $stageName
    $zip = Join-Path $Out "$stageName.zip"

    Write-Host "== staging $stage ==" -ForegroundColor Cyan
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    if (Test-Path $zip) { Remove-Item -Force $zip }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    Copy-Item (Join-Path $bin "ntscrt$exeSuffix") $stage
    Copy-Item (Join-Path $bin "ntscrt-smoke$exeSuffix") $stage
    Copy-Item (Join-Path $windows 'README.md') $stage

    # The shader tree, minus its .git. Copy-Item has no exclude-directory, so
    # walk the files and rebuild the relative paths.
    $shadersDest = Join-Path $stage 'shaders'
    $sourceRoot = (Resolve-Path $shadersSource).Path
    Get-ChildItem -Path $shadersSource -Recurse -File -Force |
        Where-Object { $_.FullName.Substring($sourceRoot.Length) -notmatch '[\\/]\.git([\\/]|$)' } |
        ForEach-Object {
            $rel = $_.FullName.Substring($sourceRoot.Length).TrimStart('\', '/')
            $dest = Join-Path $shadersDest $rel
            $dir = Split-Path $dest -Parent
            if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
            Copy-Item $_.FullName $dest
        }

    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'presets') | Out-Null
    Copy-Item (Join-Path $presetsSource '*.json') (Join-Path $stage 'presets')

    # Prove the staged layout resolves its own assets before zipping: this is
    # the "beside the executable" lookup the install relies on, and the one
    # thing a source-tree build never exercises.
    Write-Host '== checking the staged shaders resolve ==' -ForegroundColor Cyan
    $env:NTSCRT_SHADERS = $null
    $env:NTSCRT_PRESETS = $null
    $listing = & (Join-Path $stage "ntscrt-smoke$exeSuffix") --list-shaders
    if ($LASTEXITCODE -ne 0) { throw "ntscrt-smoke --list-shaders failed ($LASTEXITCODE)" }
    $listing | ForEach-Object { Write-Host "   $_" }
    if ($listing -match 'MISSING') { throw 'a bundled shader does not resolve from the staged folder' }

    Write-Host "== zipping $zip ==" -ForegroundColor Cyan
    Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
    $mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
    Write-Host "wrote $zip ($mb MB)" -ForegroundColor Green
}
finally {
    Pop-Location
}
