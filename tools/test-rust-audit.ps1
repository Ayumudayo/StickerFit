param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
  [string]$CargoAuditPath = $env:STICKERFIT_CARGO_AUDIT_PATH
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-CheckedNative {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [string[]]$ArgumentList = @(),
    [Parameter(Mandatory = $true)][string]$Description,
    [switch]$CaptureOutput
  )

  if ($CaptureOutput) {
    $output = & $FilePath @ArgumentList 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
      throw "$Description failed with exit code $exitCode."
    }
    return (($output | Out-String).Trim())
  }

  & $FilePath @ArgumentList
  $exitCode = $LASTEXITCODE
  if ($exitCode -ne 0) {
    throw "$Description failed with exit code $exitCode."
  }
}

$resolvedWorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot -ErrorAction Stop).Path
$lockPath = Join-Path $resolvedWorkspaceRoot "src-tauri\Cargo.lock"
if (-not (Test-Path -LiteralPath $lockPath -PathType Leaf)) {
  throw "Cargo.lock was not found: $lockPath"
}

if ([string]::IsNullOrWhiteSpace($CargoAuditPath)) {
  $cargoAuditCommand = Get-Command cargo-audit.exe -CommandType Application -ErrorAction Stop |
    Select-Object -First 1
  $CargoAuditPath = $cargoAuditCommand.Source
}
$resolvedCargoAuditPath = (Resolve-Path -LiteralPath $CargoAuditPath -ErrorAction Stop).Path
$cargoAuditItem = Get-Item -LiteralPath $resolvedCargoAuditPath -Force
if ($cargoAuditItem.PSIsContainer -or
    ($cargoAuditItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
    -not [string]::Equals($cargoAuditItem.Name, "cargo-audit.exe", [System.StringComparison]::Ordinal)) {
  throw "cargo-audit must be a regular executable named exactly cargo-audit.exe."
}

$version = Invoke-CheckedNative `
  -FilePath $resolvedCargoAuditPath `
  -ArgumentList @("--version") `
  -Description "cargo-audit --version" `
  -CaptureOutput
if ($version -ne "cargo-audit 0.22.2") {
  throw "cargo-audit must be exactly 0.22.2; got: $version"
}

# --file pins the audit input to the checked-in lockfile. The direct executable
# invocation avoids repository-local Cargo configuration and does not resolve or
# update dependency versions in this mode.
Invoke-CheckedNative -FilePath $resolvedCargoAuditPath -ArgumentList @(
  "audit", "--file", $lockPath
) -Description "cargo-audit"

Write-Host "Rust advisory audit passed with cargo-audit 0.22.2."
