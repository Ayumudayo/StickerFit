param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
  [string]$JsonOutput,
  [switch]$ExcludeWebDist
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-PathSizeBytes {
  param([Parameter(Mandatory = $true)][string]$Path)

  $item = Get-Item -LiteralPath $Path -ErrorAction Stop
  if (-not $item.PSIsContainer) {
    return [long]$item.Length
  }

  $measurement = Get-ChildItem -LiteralPath $item.FullName -Recurse -File -ErrorAction Stop |
    Measure-Object -Property Length -Sum
  if ($null -eq $measurement.Sum) {
    return [long]0
  }
  return [long]$measurement.Sum
}

function New-ArtifactRecord {
  param(
    [Parameter(Mandatory = $true)][string]$Label,
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Kind
  )

  $item = Get-Item -LiteralPath $Path -ErrorAction Stop
  $sizeBytes = Get-PathSizeBytes -Path $item.FullName
  $sha256 = if ($item.PSIsContainer) {
    $null
  } else {
    (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  }

  return [pscustomobject][ordered]@{
    label = $Label
    kind = $Kind
    path = $item.FullName
    bytes = $sizeBytes
    sizeMiB = [math]::Round(($sizeBytes / 1MB), 2)
    sha256 = $sha256
  }
}

function Get-CheckedGitCommit {
  param([Parameter(Mandatory = $true)][string]$ResolvedWorkspaceRoot)

  $gitCommand = Get-Command git.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1
  $output = & $gitCommand.Source -C $ResolvedWorkspaceRoot rev-parse --verify HEAD 2>&1
  $exitCode = $LASTEXITCODE
  if ($exitCode -ne 0) {
    throw "git rev-parse failed with exit code $exitCode."
  }

  $commit = (($output | Out-String).Trim()).ToLowerInvariant()
  if ($commit -notmatch '^[0-9a-f]{40}$') {
    throw "git returned an invalid commit SHA: $commit"
  }
  return $commit
}

function Get-ConfiguredResourceDestinations {
  param([Parameter(Mandatory = $true)]$Resources)

  if ($null -eq $Resources) {
    return @()
  }

  if ($Resources -is [string]) {
    return @([string]$Resources)
  }

  if ($Resources -is [System.Collections.IEnumerable] -and -not ($Resources -is [pscustomobject])) {
    return @($Resources | ForEach-Object { [string]$_ })
  }

  return @(
    $Resources.PSObject.Properties |
      ForEach-Object { [string]$_.Value }
  )
}

$resolvedWorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot -ErrorAction Stop).Path
$tauriConfigPath = Join-Path $resolvedWorkspaceRoot "src-tauri\tauri.conf.json"
if (-not (Test-Path -LiteralPath $tauriConfigPath -PathType Leaf)) {
  throw "Tauri configuration was not found: $tauriConfigPath"
}

$tauriConfig = Get-Content -Raw -LiteralPath $tauriConfigPath | ConvertFrom-Json
$releaseRoot = Join-Path $resolvedWorkspaceRoot "src-tauri\target\release"
if ([string]::IsNullOrWhiteSpace($JsonOutput)) {
  $JsonOutput = Join-Path $releaseRoot "size-report.json"
}
$desktopPath = Join-Path $releaseRoot "desktop.exe"
$sidecarPath = Join-Path $releaseRoot "ffmpeg.exe"
$nsisRoot = Join-Path $releaseRoot "bundle\nsis"

# Resolve the complete required set before hashing, printing, or touching the
# requested JSON path. Missing or ambiguous release inputs are fail-closed.
$requiredPaths = New-Object System.Collections.Generic.List[object]
$requiredPaths.Add([pscustomobject]@{ Label = "desktop.exe"; Kind = "desktop"; Path = $desktopPath; PathType = "Leaf" })
$requiredPaths.Add([pscustomobject]@{ Label = "packaged ffmpeg sidecar"; Kind = "sidecar"; Path = $sidecarPath; PathType = "Leaf" })

$resourceDestinations = Get-ConfiguredResourceDestinations -Resources $tauriConfig.bundle.resources
foreach ($destination in $resourceDestinations) {
  if ([string]::IsNullOrWhiteSpace($destination)) {
    throw "Tauri bundle.resources contains an empty destination."
  }
  if ([IO.Path]::IsPathRooted($destination) -or
      $destination -match '^[A-Za-z]:' -or
      $destination -match '(^|[\\/])\.\.([\\/]|$)') {
    throw "Tauri bundle resource destination is not a safe relative path: $destination"
  }

  $normalizedDestination = $destination.TrimStart('\', '/').Replace('/', '\')
  $packagedResourcePath = [IO.Path]::GetFullPath((Join-Path $releaseRoot $normalizedDestination))
  $releasePrefix = [IO.Path]::GetFullPath($releaseRoot).TrimEnd('\') + '\'
  if (-not $packagedResourcePath.StartsWith($releasePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Tauri bundle resource destination escapes the release directory: $destination"
  }
  $requiredPaths.Add([pscustomobject]@{
      Label = "packaged resource: $destination"
      Kind = "resource"
      Path = $packagedResourcePath
      PathType = "Any"
    })
}

foreach ($required in $requiredPaths) {
  $requiredExists = if ($required.PathType -eq "Any") {
    Test-Path -LiteralPath $required.Path
  } else {
    Test-Path -LiteralPath $required.Path -PathType $required.PathType
  }
  if (-not $requiredExists) {
    throw "Required $($required.Kind) input is missing: $($required.Path). JSON was not written."
  }
}

if (-not (Test-Path -LiteralPath $nsisRoot -PathType Container)) {
  throw "Required NSIS output directory is missing: $nsisRoot. JSON was not written."
}
$nsisBundles = @(
  Get-ChildItem -LiteralPath $nsisRoot -Filter "StickerFit_*_x64-setup.exe" -File -ErrorAction Stop
)
if ($nsisBundles.Count -ne 1) {
  throw "Expected exactly one NSIS installer under $nsisRoot, found $($nsisBundles.Count). Remove stale installers before reporting. JSON was not written."
}
$requiredPaths.Add([pscustomobject]@{
    Label = "NSIS bundle"
    Kind = "installer"
    Path = $nsisBundles[0].FullName
    PathType = "Leaf"
  })

$seenPaths = @{}
$artifacts = New-Object System.Collections.Generic.List[object]
foreach ($required in $requiredPaths) {
  $fullPath = [IO.Path]::GetFullPath([string]$required.Path)
  if ($seenPaths.ContainsKey($fullPath)) {
    continue
  }
  $seenPaths[$fullPath] = $true
  $artifacts.Add((New-ArtifactRecord -Label $required.Label -Path $fullPath -Kind $required.Kind))
}

$jsonFullPath = [IO.Path]::GetFullPath($JsonOutput)
if ([IO.Path]::GetExtension($jsonFullPath) -ine ".json") {
  throw "JSON output must use a .json file path: $jsonFullPath"
}
foreach ($artifact in $artifacts) {
  $artifactFullPath = [IO.Path]::GetFullPath([string]$artifact.path)
  if ([string]::Equals($jsonFullPath, $artifactFullPath, [StringComparison]::OrdinalIgnoreCase)) {
    throw "JSON output must not overwrite a release artifact: $artifactFullPath"
  }

  $artifactItem = Get-Item -LiteralPath $artifactFullPath -ErrorAction Stop
  if ($artifactItem.PSIsContainer) {
    $artifactPrefix = $artifactFullPath.TrimEnd('\') + '\'
    if ($jsonFullPath.StartsWith($artifactPrefix, [StringComparison]::OrdinalIgnoreCase)) {
      throw "JSON output must not be written inside a packaged resource directory: $artifactFullPath"
    }
  }
}

$runtimeArtifacts = @($artifacts | Where-Object { $_.kind -ne "installer" })
$installedFootprintBytes = [long](($runtimeArtifacts | Measure-Object -Property bytes -Sum).Sum)
$distPath = Join-Path $resolvedWorkspaceRoot "dist"
$distRecord = if (-not $ExcludeWebDist -and (Test-Path -LiteralPath $distPath -PathType Container)) {
  New-ArtifactRecord -Label "web dist" -Path $distPath -Kind "web-dist"
} else {
  $null
}

$report = [pscustomobject][ordered]@{
  schemaVersion = 1
  generatedAtUtc = [DateTime]::UtcNow.ToString("o")
  commitSha = Get-CheckedGitCommit -ResolvedWorkspaceRoot $resolvedWorkspaceRoot
  installedFootprintBytes = $installedFootprintBytes
  installedFootprintMiB = [math]::Round(($installedFootprintBytes / 1MB), 2)
  artifacts = @($artifacts)
  webDist = $distRecord
}

$tableEntries = @(
  [pscustomobject]@{
    Label = "Estimated installed footprint"
    SizeMiB = $report.installedFootprintMiB
    Bytes = $report.installedFootprintBytes
    Path = $releaseRoot
  }
) + @(
  $artifacts | ForEach-Object {
    [pscustomobject]@{
      Label = $_.label
      SizeMiB = $_.sizeMiB
      Bytes = $_.bytes
      Path = $_.path
    }
  }
)
if ($null -ne $distRecord) {
  $tableEntries += [pscustomobject]@{
    Label = $distRecord.label
    SizeMiB = $distRecord.sizeMiB
    Bytes = $distRecord.bytes
    Path = $distRecord.path
  }
}

$tableEntries | Format-Table -AutoSize

$reportJson = $report | ConvertTo-Json -Depth 8
$jsonDirectory = Split-Path -Parent $jsonFullPath
if (-not (Test-Path -LiteralPath $jsonDirectory -PathType Container)) {
  $null = New-Item -ItemType Directory -Path $jsonDirectory -Force
}

$temporaryJsonPath = Join-Path $jsonDirectory (([IO.Path]::GetRandomFileName()) + ".tmp")
try {
  Set-Content -LiteralPath $temporaryJsonPath -Value $reportJson -Encoding UTF8
  Move-Item -LiteralPath $temporaryJsonPath -Destination $jsonFullPath -Force
}
finally {
  Remove-Item -LiteralPath $temporaryJsonPath -Force -ErrorAction SilentlyContinue
}
Write-Host "JSON report: $jsonFullPath"

if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_STEP_SUMMARY)) {
  $summaryLines = @(
    "### StickerFit release size report",
    "",
    "- Commit: ``$($report.commitSha)``",
    "- Installed footprint: $($report.installedFootprintBytes) bytes ($($report.installedFootprintMiB) MiB)",
    "- NSIS: $($nsisBundles[0].FullName) ($($nsisBundles[0].Length) bytes)"
  )
  Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $summaryLines -Encoding UTF8
}
