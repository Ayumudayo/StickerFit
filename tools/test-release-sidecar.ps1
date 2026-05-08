param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"

$releaseDir = Join-Path $WorkspaceRoot "src-tauri\target\release"
$ffmpegPath = Join-Path $releaseDir "ffmpeg.exe"
$dllPath = Join-Path $releaseDir "libwinpthread-1.dll"

if (-not (Test-Path $ffmpegPath)) {
  throw "Release ffmpeg sidecar was not found at $ffmpegPath. Run npm run build:desktop first."
}

if (-not (Test-Path $dllPath)) {
  throw "Release ffmpeg runtime DLL was not found beside ffmpeg.exe at $dllPath."
}

$output = & $ffmpegPath -hide_banner -version 2>&1
if ($LASTEXITCODE -ne 0) {
  $message = ($output | Out-String).Trim()
  throw "Release ffmpeg sidecar failed to start with exit code $LASTEXITCODE. $message"
}

$firstLine = ($output | Select-Object -First 1)
Write-Host "Release sidecar OK: $firstLine"
