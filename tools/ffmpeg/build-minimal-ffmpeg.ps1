param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\\..")).Path,
  [string]$FfmpegSourceRoot,
  [string]$FfmpegVersion = "7.1.3",
  [string]$DownloadCacheRoot = (Join-Path $env:TEMP "stickerfit-ffmpeg-cache"),
  [string]$BuildRoot = (Join-Path $env:TEMP "stickerfit-ffmpeg-build"),
  [string]$TargetTriple = "x86_64-pc-windows-msvc",
  [string]$BashPath = "C:\\msys64\\usr\\bin\\bash.exe",
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"

$requiredDemuxers = @(
  "matroska",
  "mov"
)

$requiredDecoders = @(
  "h264",
  "mjpeg",
  "mpeg4",
  "vp8",
  "vp9"
)

$requiredMuxers = @(
  "null",
  "rawvideo"
)

$requiredEncoders = @(
  "rawvideo"
)

$requiredParsers = @(
  "h264",
  "mjpeg",
  "mpeg4",
  "vp8",
  "vp9"
)

$requiredProtocols = @(
  "file",
  "pipe"
)

$requiredFilters = @(
  "crop",
  "scale",
  "pad",
  "fps",
  "format",
  "setpts",
  "setsar",
  "select"
)

function Join-EnabledArgs {
  param(
    [string]$Prefix,
    [string[]]$Values
  )

  return ($Values | ForEach-Object { "$Prefix=$_" })
}

function Resolve-FfmpegSource {
  param(
    [string]$ExplicitSourceRoot,
    [string]$Version,
    [string]$CacheRoot
  )

  if ($ExplicitSourceRoot) {
    if (-not (Test-Path $ExplicitSourceRoot)) {
      throw "FFmpeg source root not found: $ExplicitSourceRoot"
    }

    return (Resolve-Path $ExplicitSourceRoot).Path
  }

  $archiveName = "ffmpeg-$Version.tar.xz"
  $archiveUrl = "https://ffmpeg.org/releases/$archiveName"
  $archivePath = Join-Path $CacheRoot $archiveName
  $extractRoot = Join-Path $CacheRoot "src"
  $sourceRoot = Join-Path $extractRoot "ffmpeg-$Version"
  $windowsTarPath = Join-Path $env:SystemRoot "System32\tar.exe"

  New-Item -ItemType Directory -Force -Path $CacheRoot | Out-Null
  New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null

  if (-not (Test-Path $archivePath)) {
    Write-Host "Downloading FFmpeg source archive:"
    Write-Host "  $archiveUrl"
    Invoke-WebRequest -Uri $archiveUrl -OutFile $archivePath
  }

  if (-not (Test-Path $sourceRoot)) {
    if (-not (Test-Path $windowsTarPath)) {
      throw "The tar command is required to extract FFmpeg source archives."
    }

    $extractAttempts = 0
    do {
      $extractAttempts += 1
      Write-Host "Extracting FFmpeg source archive to:"
      Write-Host "  $extractRoot"
      & $windowsTarPath -xf $archivePath -C $extractRoot
      if ($LASTEXITCODE -eq 0 -and (Test-Path $sourceRoot)) {
        break
      }

      if ($extractAttempts -ge 2) {
        throw "Failed to extract FFmpeg source archive."
      }

      Write-Host "The cached FFmpeg archive appears invalid. Re-downloading..."
      if (Test-Path $sourceRoot) {
        Remove-Item -Path $sourceRoot -Recurse -Force
      }

      Remove-Item -Path $archivePath -Force -ErrorAction SilentlyContinue
      Invoke-WebRequest -Uri $archiveUrl -OutFile $archivePath
    }
    while ($extractAttempts -lt 2)
  }

  return (Resolve-Path $sourceRoot).Path
}

$configureArgs = @(
  "--disable-autodetect",
  "--disable-debug",
  "--disable-doc",
  "--disable-everything",
  "--disable-network",
  "--disable-programs",
  "--disable-sdl2",
  "--disable-shared",
  "--enable-ffmpeg",
  "--enable-small",
  "--arch=x86_64",
  "--target-os=mingw32"
) + `
  (Join-EnabledArgs "--enable-demuxer" $requiredDemuxers) + `
  (Join-EnabledArgs "--enable-decoder" $requiredDecoders) + `
  (Join-EnabledArgs "--enable-muxer" $requiredMuxers) + `
  (Join-EnabledArgs "--enable-encoder" $requiredEncoders) + `
  (Join-EnabledArgs "--enable-parser" $requiredParsers) + `
  (Join-EnabledArgs "--enable-protocol" $requiredProtocols) + `
  (Join-EnabledArgs "--enable-filter" $requiredFilters)

$binaryName = "ffmpeg-$TargetTriple.exe"
$targetBinaryPath = Join-Path $WorkspaceRoot "src-tauri\\binaries\\$binaryName"
$runtimeDllPath = Join-Path $WorkspaceRoot "src-tauri\\binaries\\libwinpthread-1.dll"
$resolvedSourceRoot = Resolve-FfmpegSource -ExplicitSourceRoot $FfmpegSourceRoot -Version $FfmpegVersion -CacheRoot $DownloadCacheRoot

Write-Host "StickerFit minimal ffmpeg build"
Write-Host "Workspace root : $WorkspaceRoot"
Write-Host "FFmpeg source  : $resolvedSourceRoot"
Write-Host "Build root     : $BuildRoot"
Write-Host "Target binary  : $targetBinaryPath"
Write-Host "Runtime DLL    : $runtimeDllPath"
Write-Host ""
Write-Host "Retained demuxers : $($requiredDemuxers -join ', ')"
Write-Host "Retained decoders : $($requiredDecoders -join ', ')"
Write-Host "Retained muxers   : $($requiredMuxers -join ', ')"
Write-Host "Retained encoders : $($requiredEncoders -join ', ')"
Write-Host "Retained filters  : $($requiredFilters -join ', ')"
Write-Host ""

$bash = Get-Command $BashPath -ErrorAction SilentlyContinue
if (-not $bash) {
  throw "MSYS2 bash was not found at $BashPath."
}

New-Item -ItemType Directory -Force -Path $BuildRoot | Out-Null
$buildScriptPath = Join-Path $BuildRoot "build-minimal-ffmpeg.sh"
$configureLine = $configureArgs -join " "

$buildScriptTemplate = @'
#!/usr/bin/env bash
set -euo pipefail

export MSYSTEM=MINGW64
export PATH=/mingw64/bin:/usr/bin:$PATH

for required in gcc make nasm pkgconf; do
  command -v "$required" >/dev/null 2>&1 || {
    echo "Required MSYS2 tool is missing: $required" >&2
    exit 1
  }
done

cd "$(cygpath -u '__SOURCE_ROOT__')"
make distclean >/dev/null 2>&1 || true
./configure __CONFIGURE_LINE__
make -j$(nproc)
cp ffmpeg.exe "$(cygpath -u '__TARGET_BINARY__')"
cp /mingw64/bin/libwinpthread-1.dll "$(cygpath -u '__RUNTIME_DLL__')"
'@

$buildScript = $buildScriptTemplate.
  Replace("__SOURCE_ROOT__", $resolvedSourceRoot).
  Replace("__CONFIGURE_LINE__", $configureLine).
  Replace("__TARGET_BINARY__", $targetBinaryPath).
  Replace("__RUNTIME_DLL__", $runtimeDllPath)

[System.IO.File]::WriteAllText($buildScriptPath, $buildScript)

if ($DryRun) {
  Write-Host "Dry run requested. Generated script:"
  Write-Host $buildScript
  exit 0
}

& $bash.Source $buildScriptPath
if ($LASTEXITCODE -ne 0) {
  throw "Minimal ffmpeg build failed with exit code $LASTEXITCODE."
}

Write-Host ""
Write-Host "Copied minimal ffmpeg binary to:"
Write-Host $targetBinaryPath
