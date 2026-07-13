param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
  [string]$SampleDir = $env:STICKERFIT_SAMPLE_GIF_DIR,
  [string]$Preset = "auto",
  [string]$SearchDepth = "standard"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-CheckedNative {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [string[]]$ArgumentList = @(),
    [Parameter(Mandatory = $true)][string]$Description
  )

  & $FilePath @ArgumentList
  $exitCode = $LASTEXITCODE
  if ($exitCode -ne 0) {
    throw "$Description failed with exit code $exitCode."
  }
}

if ([string]::IsNullOrWhiteSpace($SampleDir)) {
  throw "Set STICKERFIT_SAMPLE_GIF_DIR or pass -SampleDir to a folder containing GIF samples."
}

$resolvedSampleDir = (Resolve-Path -LiteralPath $SampleDir -ErrorAction Stop).Path
if (-not (Test-Path -LiteralPath $resolvedSampleDir -PathType Container)) {
  throw "Sample GIF directory was not found: $resolvedSampleDir"
}

$resolvedWorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot -ErrorAction Stop).Path
$manifestPath = Join-Path $resolvedWorkspaceRoot "src-tauri\Cargo.toml"
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
  throw "Rust manifest was not found: $manifestPath"
}

$previousSampleDir = $env:STICKERFIT_SAMPLE_GIF_DIR
$previousPreset = $env:STICKERFIT_SAMPLE_GIF_PRESET
$previousSearchDepth = $env:STICKERFIT_SAMPLE_GIF_SEARCH_DEPTH

try {
  $env:STICKERFIT_SAMPLE_GIF_DIR = $resolvedSampleDir
  $env:STICKERFIT_SAMPLE_GIF_PRESET = $Preset
  $env:STICKERFIT_SAMPLE_GIF_SEARCH_DEPTH = $SearchDepth

  Push-Location $resolvedWorkspaceRoot
  try {
    $cargoCommand = Get-Command cargo.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1
    Invoke-CheckedNative -FilePath $cargoCommand.Source -ArgumentList @(
      "test",
      "--locked",
      "--manifest-path", $manifestPath,
      "sample_gif_folder_optimizes_to_discord_limit",
      "--",
      "--ignored",
      "--nocapture"
    ) -Description "optional sample GIF regression"
  }
  finally {
    Pop-Location
  }
}
finally {
  if ($null -eq $previousSampleDir) {
    Remove-Item Env:\STICKERFIT_SAMPLE_GIF_DIR -ErrorAction SilentlyContinue
  } else {
    $env:STICKERFIT_SAMPLE_GIF_DIR = $previousSampleDir
  }

  if ($null -eq $previousPreset) {
    Remove-Item Env:\STICKERFIT_SAMPLE_GIF_PRESET -ErrorAction SilentlyContinue
  } else {
    $env:STICKERFIT_SAMPLE_GIF_PRESET = $previousPreset
  }

  if ($null -eq $previousSearchDepth) {
    Remove-Item Env:\STICKERFIT_SAMPLE_GIF_SEARCH_DEPTH -ErrorAction SilentlyContinue
  } else {
    $env:STICKERFIT_SAMPLE_GIF_SEARCH_DEPTH = $previousSearchDepth
  }
}
