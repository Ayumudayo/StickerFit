[CmdletBinding()]
param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path,
  [string]$ManifestPath = "tools/ffmpeg/ffmpeg-version.json",
  [string]$DownloadCacheRoot = (Join-Path $env:TEMP "stickerfit-ffmpeg-cache"),
  [switch]$VerifySourceOnly,
  [switch]$VerifyVendorArtifacts,
  [switch]$UpdateVendorArtifacts,
  [string]$TargetVersion,
  [string]$ExpectedSourceSha256,
  [long]$SourceDateEpoch = 0,
  [Alias('OutputDirectory')]
  [string]$StagingRoot,
  [string]$FfmpegSourceRoot,
  [string]$BashPath,
  [string]$GpgPath,
  [string]$GpgvPath,
  [string]$TarPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$modulePath = Join-Path $PSScriptRoot "FfmpegSourceVerification.psm1"
Import-Module -Name $modulePath -Force

function Assert-ProtectedVendorUpdateContext {
  if (-not [string]::Equals(
      $env:GITHUB_ACTIONS,
      'true',
      [System.StringComparison]::Ordinal
    )) {
    throw "FFmpeg vendor updates run only in GitHub Actions."
  }

  if (-not [string]::Equals(
      $env:GITHUB_REF,
      'refs/heads/main',
      [System.StringComparison]::Ordinal
    )) {
    throw "FFmpeg vendor updates require the protected main branch."
  }

  if (-not [string]::Equals(
      $env:GITHUB_WORKFLOW_REF,
      'Ayumudayo/StickerFit/.github/workflows/update-ffmpeg-vendor.yml@refs/heads/main',
      [System.StringComparison]::Ordinal
    )) {
    throw "FFmpeg vendor updates require the protected update-ffmpeg-vendor.yml workflow on main."
  }

  if (-not [string]::Equals(
      $env:STICKERFIT_PROTECTED_FFMPEG_UPDATE,
      '1',
      [System.StringComparison]::Ordinal
    )) {
    throw "The protected FFmpeg vendor environment marker is missing."
  }

  if ($env:GITHUB_RUN_ID -notmatch '^\d+$' -or $env:GITHUB_SHA -cnotmatch '^[0-9a-f]{40}$') {
    throw "The protected workflow run ID or source commit is missing or malformed."
  }

  if (-not $env:RUNNER_TEMP -or -not (Test-Path -LiteralPath $env:RUNNER_TEMP -PathType Container)) {
    throw "RUNNER_TEMP is required for staging-only FFmpeg updates."
  }

  if ($env:SOURCE_DATE_EPOCH -notmatch '^\d+$' -or [long]$env:SOURCE_DATE_EPOCH -le 0) {
    throw "Protected workflow must export a positive SOURCE_DATE_EPOCH."
  }

  $expectedAuthorities = [ordered]@{
    STICKERFIT_VENDOR_RUN_ID = $env:GITHUB_RUN_ID
    STICKERFIT_VENDOR_REPOSITORY = $env:GITHUB_REPOSITORY
    STICKERFIT_VENDOR_WORKFLOW_PATH = '.github/workflows/update-ffmpeg-vendor.yml'
    STICKERFIT_VENDOR_EVENT = 'workflow_dispatch'
    STICKERFIT_VENDOR_HEAD_BRANCH = 'main'
    STICKERFIT_VENDOR_HEAD_SHA = $env:GITHUB_SHA
  }
  foreach ($entry in $expectedAuthorities.GetEnumerator()) {
    $actual = [Environment]::GetEnvironmentVariable($entry.Key)
    if (-not $actual -or -not [string]::Equals(
        $actual,
        "$($entry.Value)",
        [System.StringComparison]::Ordinal
      )) {
      throw "Protected workflow authority mismatch: $($entry.Key)"
    }
  }

  if (-not [string]::Equals(
      $env:GITHUB_EVENT_NAME,
      'workflow_dispatch',
      [System.StringComparison]::Ordinal
    ) -or
      -not [string]::Equals(
        $env:GITHUB_REPOSITORY,
        'Ayumudayo/StickerFit',
        [System.StringComparison]::Ordinal
      )) {
    throw "Protected workflow event or repository authority is invalid."
  }

  foreach ($pathEnvironmentName in @(
      'STICKERFIT_BASH_PATH',
      'STICKERFIT_GPG_PATH',
      'STICKERFIT_GPGV_PATH',
      'STICKERFIT_VSWHERE_PATH'
    )) {
    $pathValue = [Environment]::GetEnvironmentVariable($pathEnvironmentName)
    if (-not $pathValue -or -not (Test-Path -LiteralPath $pathValue -PathType Leaf)) {
      throw "Protected workflow did not export a valid tool path: $pathEnvironmentName"
    }
  }

  if (-not $env:STICKERFIT_MSYS2_LOCATION -or
      -not (Test-Path -LiteralPath $env:STICKERFIT_MSYS2_LOCATION -PathType Container)) {
    throw "Protected workflow did not export a valid STICKERFIT_MSYS2_LOCATION."
  }

  return [pscustomobject]@{
    runId = [long]$env:STICKERFIT_VENDOR_RUN_ID
    repository = $env:STICKERFIT_VENDOR_REPOSITORY
    workflowPath = $env:STICKERFIT_VENDOR_WORKFLOW_PATH
    event = $env:STICKERFIT_VENDOR_EVENT
    headBranch = $env:STICKERFIT_VENDOR_HEAD_BRANCH
    headSha = $env:STICKERFIT_VENDOR_HEAD_SHA
  }
}

function Assert-NoReparsePointInProtectedPath {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [string]$Boundary
  )

  $cursor = [System.IO.Path]::GetFullPath($Path).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $resolvedBoundary = [System.IO.Path]::GetFullPath($Boundary).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $boundaryPrefix = $resolvedBoundary + [System.IO.Path]::DirectorySeparatorChar
  if ($cursor -ne $resolvedBoundary -and
      -not $cursor.StartsWith($boundaryPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Protected path is outside its checked boundary: $Path"
  }

  while ($true) {
    if (Test-Path -LiteralPath $cursor) {
      $item = Get-Item -LiteralPath $cursor -Force
      if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Protected recursive path contains a junction, symlink, or other reparse point: $cursor"
      }
    }

    if ([string]::Equals(
        $cursor,
        $resolvedBoundary,
        [System.StringComparison]::OrdinalIgnoreCase
      )) {
      break
    }

    $parent = [System.IO.Directory]::GetParent($cursor)
    if ($null -eq $parent) {
      throw "Protected path traversal reached no checked boundary: $Path"
    }

    $cursor = $parent.FullName.TrimEnd(
      [System.IO.Path]::DirectorySeparatorChar,
      [System.IO.Path]::AltDirectorySeparatorChar
    )
  }
}

function Assert-NoReparsePointInProtectedTree {
  param(
    [Parameter(Mandatory = $true)]
    [string]$RootPath
  )

  $pendingDirectories = New-Object 'System.Collections.Generic.Queue[string]'
  $pendingDirectories.Enqueue([System.IO.Path]::GetFullPath($RootPath))
  while ($pendingDirectories.Count -gt 0) {
    $currentDirectory = $pendingDirectories.Dequeue()
    $currentItem = Get-Item -LiteralPath $currentDirectory -Force
    if (($currentItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Protected recursive tree contains a reparse-point directory: $currentDirectory"
    }

    foreach ($child in @(Get-ChildItem -LiteralPath $currentDirectory -Force)) {
      if (($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Protected recursive tree contains a reparse-point descendant: $($child.FullName)"
      }

      if (($child.Attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
        $pendingDirectories.Enqueue($child.FullName)
      }
    }
  }
}

function Reset-ProtectedStagingRoot {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [string]$Workspace,

    [Parameter(Mandatory = $true)]
    [string]$AllowedRoot,

    [Parameter(Mandatory = $true)]
    [string]$RunId
  )

  $resolvedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $resolvedWorkspace = [System.IO.Path]::GetFullPath($Workspace).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $resolvedAllowedRoot = [System.IO.Path]::GetFullPath($AllowedRoot).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )

  $allowedPrefix = $resolvedAllowedRoot + [System.IO.Path]::DirectorySeparatorChar
  $workspacePrefix = $resolvedWorkspace + [System.IO.Path]::DirectorySeparatorChar
  if (-not $resolvedPath.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      $resolvedPath.StartsWith($workspacePrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      $resolvedPath -eq $resolvedWorkspace) {
    throw "Staging root must be a descendant of RUNNER_TEMP and outside the repository: $resolvedPath"
  }

  Assert-NoReparsePointInProtectedPath `
    -Path $resolvedPath `
    -Boundary $resolvedAllowedRoot

  $markerValue = "stickerfit-ffmpeg-vendor-staging-v1:$RunId"
  $markerPath = Join-Path $resolvedPath '.stickerfit-ffmpeg-staging.marker'
  if (Test-Path -LiteralPath $resolvedPath) {
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
      throw "Refusing to clear an unmarked or differently owned staging root: $resolvedPath"
    }

    $markerItem = Get-Item -LiteralPath $markerPath -Force
    if (($markerItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Refusing a reparse-point staging marker: $markerPath"
    }

    if ((Get-Content -Raw -LiteralPath $markerPath) -ne $markerValue) {
      throw "Refusing to clear a differently owned staging root: $resolvedPath"
    }

    Assert-NoReparsePointInProtectedTree -RootPath $resolvedPath

    Remove-Item -LiteralPath $resolvedPath -Recurse -Force
  }

  New-Item -ItemType Directory -Path $resolvedPath | Out-Null
  [System.IO.File]::WriteAllText(
    $markerPath,
    $markerValue,
    [System.Text.UTF8Encoding]::new($false)
  )
  return $resolvedPath
}

function Remove-ProtectedStagingRoot {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [string]$Workspace,

    [Parameter(Mandatory = $true)]
    [string]$AllowedRoot,

    [Parameter(Mandatory = $true)]
    [string]$RunId
  )

  $resolvedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $resolvedWorkspace = [System.IO.Path]::GetFullPath($Workspace).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $resolvedAllowedRoot = [System.IO.Path]::GetFullPath($AllowedRoot).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $allowedPrefix = $resolvedAllowedRoot + [System.IO.Path]::DirectorySeparatorChar
  $workspacePrefix = $resolvedWorkspace + [System.IO.Path]::DirectorySeparatorChar
  $markerPath = Join-Path $resolvedPath '.stickerfit-ffmpeg-staging.marker'
  $expectedMarker = "stickerfit-ffmpeg-vendor-staging-v1:$RunId"
  if (-not $resolvedPath.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      $resolvedPath.StartsWith($workspacePrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      $resolvedPath -eq $resolvedWorkspace -or
      -not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
    throw "Refusing to remove an unmarked or unsafe protected staging root: $resolvedPath"
  }

  Assert-NoReparsePointInProtectedPath `
    -Path $resolvedPath `
    -Boundary $resolvedAllowedRoot
  $markerItem = Get-Item -LiteralPath $markerPath -Force
  if (($markerItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Refusing a reparse-point staging marker: $markerPath"
  }

  if ((Get-Content -Raw -LiteralPath $markerPath) -ne $expectedMarker) {
    throw "Refusing to remove a differently owned protected staging root: $resolvedPath"
  }

  Assert-NoReparsePointInProtectedTree -RootPath $resolvedPath

  Remove-Item -LiteralPath $resolvedPath -Recurse -Force
}

function Assert-ExactStringMap {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Actual,

    [Parameter(Mandatory = $true)]
    [System.Collections.IDictionary]$Expected,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($null -eq $Actual -or
      $Actual -is [string] -or
      $Actual -is [System.Array]) {
    throw "$Label must be a JSON object with the exact approved string map."
  }

  $actualProperties = @($Actual.PSObject.Properties | Where-Object {
      $_.MemberType -eq [System.Management.Automation.PSMemberTypes]::NoteProperty
    })
  if ($actualProperties.Count -ne $Expected.Count) {
    throw "$Label does not contain the exact approved entries."
  }

  foreach ($entry in $Expected.GetEnumerator()) {
    $matchingProperties = @($actualProperties | Where-Object {
        [string]::Equals(
          [string]$_.Name,
          [string]$entry.Key,
          [System.StringComparison]::Ordinal
        )
      })
    if ($matchingProperties.Count -ne 1 -or
        $matchingProperties[0].Value -isnot [string] -or
        -not [string]::Equals(
          [string]$matchingProperties[0].Value,
          [string]$entry.Value,
          [System.StringComparison]::Ordinal
        )) {
      throw "$Label has an unexpected or missing entry: $($entry.Key)"
    }
  }
}

function Get-ExactJsonProperty {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Object,

    [Parameter(Mandatory = $true)]
    [string]$Name,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($null -eq $Object -or $Object -is [string] -or $Object -is [System.Array]) {
    throw "$Label must be a JSON object containing the exact '$Name' property."
  }
  $matchingProperties = @($Object.PSObject.Properties | Where-Object {
      $_.MemberType -eq [System.Management.Automation.PSMemberTypes]::NoteProperty -and
      [string]::Equals([string]$_.Name, $Name, [System.StringComparison]::Ordinal)
    })
  if ($matchingProperties.Count -ne 1) {
    throw "$Label must contain exactly one case-sensitive '$Name' property."
  }
  return $matchingProperties[0]
}

function ConvertTo-BashSingleQuotedString {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Value
  )

  if ($Value.Contains("'")) {
    throw "Single quotes are not allowed in protected FFmpeg build paths or arguments."
  }

  return "'$Value'"
}

function Get-FirstNativeVersionLine {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,

    [string[]]$Arguments = @(),

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  $result = Invoke-StrictNativeCommand `
    -Executable $Executable `
    -Arguments $Arguments `
    -FailureMessage "Could not query $Label version."
  $line = @($result.Output | ForEach-Object { $_.Trim() } | Where-Object { $_ } | Select-Object -First 1)
  if ($line.Count -ne 1) {
    throw "$Label did not report one nonblank version line."
  }

  return $line[0]
}

function Assert-ExportedToolVersion {
  param(
    [Parameter(Mandatory = $true)]
    [string]$EnvironmentName,

    [Parameter(Mandatory = $true)]
    [string]$ActualVersion
  )

  $expectedVersion = [Environment]::GetEnvironmentVariable($EnvironmentName)
  if (-not $expectedVersion -or -not [string]::Equals(
      $expectedVersion,
      $ActualVersion,
      [System.StringComparison]::Ordinal
    )) {
    throw "Resolved tool version does not match protected workflow export: $EnvironmentName"
  }
}

function Resolve-Dumpbin {
  param(
    [string]$VswherePath
  )

  $programFilesX86 = [Environment]::GetFolderPath(
    [Environment+SpecialFolder]::ProgramFilesX86
  )
  $vswhereCandidate = Join-Path $programFilesX86 "Microsoft Visual Studio\Installer\vswhere.exe"
  if (-not $VswherePath) {
    $VswherePath = Resolve-FfmpegTool -Name 'vswhere.exe' -PreferredPaths @($vswhereCandidate)
  }

  $installation = Invoke-StrictNativeCommand `
    -Executable $VswherePath `
    -Arguments @(
      '-latest',
      '-products',
      '*',
      '-requires',
      'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
      '-property',
      'installationPath'
    ) `
    -FailureMessage "Could not locate Visual Studio C++ tools."
  $installationPath = @($installation.Output | Where-Object { $_.Trim() } | Select-Object -Last 1)
  if ($installationPath.Count -ne 1 -or -not (Test-Path -LiteralPath $installationPath[0] -PathType Container)) {
    throw "vswhere did not return one valid Visual Studio installation."
  }

  $msvcRoot = Join-Path $installationPath[0] "VC\Tools\MSVC"
  $dumpbinCandidates = @(Get-ChildItem `
      -Path (Join-Path $msvcRoot "*\bin\Hostx64\x64\dumpbin.exe") `
      -File `
      -ErrorAction SilentlyContinue |
      Sort-Object -Property FullName -Descending)
  if ($dumpbinCandidates.Count -eq 0) {
    throw "dumpbin.exe was not found below the Visual Studio installation."
  }

  return [pscustomobject]@{
    VswherePath = $VswherePath
    DumpbinPath = $dumpbinCandidates[0].FullName
  }
}

function Get-DumpbinDependencies {
  param(
    [Parameter(Mandatory = $true)]
    [string]$DumpbinPath,

    [Parameter(Mandatory = $true)]
    [string]$ExecutablePath
  )

  $result = Invoke-StrictNativeCommand `
    -Executable $DumpbinPath `
    -Arguments @('/nologo', '/dependents', $ExecutablePath) `
    -FailureMessage "dumpbin could not inspect the staged FFmpeg executable."
  $dependencies = @($result.Output | ForEach-Object {
      if ($_ -match '^\s+([A-Za-z0-9._-]+\.dll)\s*$') {
        $Matches[1].ToLowerInvariant()
      }
    } | Where-Object { $_ } | Sort-Object -Unique)

  if ($dependencies.Count -eq 0) {
    throw "dumpbin did not report any Windows dependencies for the staged FFmpeg executable."
  }

  $forbiddenDependencies = @($dependencies | Where-Object {
      $_ -match '^(?:libwinpthread|libgcc|libstdc\+\+|msys-|mingw|cygwin)'
    })
  if ($forbiddenDependencies.Count -ne 0) {
    throw "Staged FFmpeg has non-system or forbidden dynamic dependencies: $($forbiddenDependencies -join ', ')"
  }

  return $dependencies
}

$selectedModes = @(@(
    [bool]$VerifySourceOnly,
    [bool]$VerifyVendorArtifacts,
    [bool]$UpdateVendorArtifacts
  ) | Where-Object { $_ })
if ($selectedModes.Count -ne 1) {
  throw "Select exactly one mode: -VerifySourceOnly, -VerifyVendorArtifacts, or -UpdateVendorArtifacts."
}

if ($FfmpegSourceRoot) {
  throw "Unpacked local FFmpeg source roots are not approved. Use the signed archive verification path."
}

if ($VerifySourceOnly) {
  $verifiedSource = Invoke-FfmpegSourceVerification `
    -WorkspaceRoot $WorkspaceRoot `
    -ManifestPath $ManifestPath `
    -DownloadCacheRoot $DownloadCacheRoot `
    -GpgPath $GpgPath `
    -GpgvPath $GpgvPath `
    -TarPath $TarPath
  Write-Host "Verified FFmpeg $($verifiedSource.Version) source archive, signature, hash, and entry policy."
  exit 0
}

if ($VerifyVendorArtifacts) {
  $verifiedVendor = Test-FfmpegVendorArtifacts `
    -WorkspaceRoot $WorkspaceRoot `
    -ManifestPath $ManifestPath `
    -AllowLegacyBootstrap
  $state = if ($verifiedVendor.LegacyBootstrap) { 'legacy bootstrap' } else { 'approved workflow' }
  Write-Host "Verified tracked FFmpeg $($verifiedVendor.Version) vendor payload ($state)."
  exit 0
}

$vendorAuthority = Assert-ProtectedVendorUpdateContext
$protectedPathBindings = @(
  [pscustomobject]@{ Name = 'STICKERFIT_BASH_PATH'; Value = $BashPath },
  [pscustomobject]@{ Name = 'STICKERFIT_GPG_PATH'; Value = $GpgPath },
  [pscustomobject]@{ Name = 'STICKERFIT_GPGV_PATH'; Value = $GpgvPath }
)
foreach ($binding in $protectedPathBindings) {
  $exportedPath = [System.IO.Path]::GetFullPath(
    [Environment]::GetEnvironmentVariable($binding.Name)
  )
  if ($binding.Value -and -not [string]::Equals(
      [System.IO.Path]::GetFullPath($binding.Value),
      $exportedPath,
      [System.StringComparison]::OrdinalIgnoreCase
    )) {
    throw "Explicit tool path does not match protected workflow export: $($binding.Name)"
  }

  switch ($binding.Name) {
    'STICKERFIT_BASH_PATH' { $BashPath = $exportedPath }
    'STICKERFIT_GPG_PATH' { $GpgPath = $exportedPath }
    'STICKERFIT_GPGV_PATH' { $GpgvPath = $exportedPath }
  }
}

$manifest = Get-FfmpegManifest -WorkspaceRoot $WorkspaceRoot -ManifestPath $ManifestPath
$tauriConfigPath = Join-Path $WorkspaceRoot 'src-tauri\tauri.conf.json'
if (-not (Test-Path -LiteralPath $tauriConfigPath -PathType Leaf)) {
  throw "The current Tauri configuration was not found: $tauriConfigPath"
}

try {
  $tauriConfig = Get-Content -Raw -LiteralPath $tauriConfigPath | ConvertFrom-Json
}
catch {
  throw "The current Tauri configuration is invalid JSON: $($_.Exception.Message)"
}

$bundleProperty = Get-ExactJsonProperty -Object $tauriConfig -Name 'bundle' -Label 'The current Tauri configuration'
if ($null -eq $bundleProperty.Value) {
  throw "The current Tauri configuration is missing bundle settings."
}

$resourcesProperty = Get-ExactJsonProperty -Object $bundleProperty.Value -Name 'resources' -Label 'The current Tauri bundle'
$externalBinProperty = Get-ExactJsonProperty -Object $bundleProperty.Value -Name 'externalBin' -Label 'The current Tauri bundle'

$legacyResourceMap = [ordered]@{
  'binaries/libwinpthread-1.dll' = 'libwinpthread-1.dll'
  'binaries/LICENSE-ffmpeg.txt' = 'LICENSE-ffmpeg.txt'
  'binaries/LICENSE-libwinpthread.txt' = 'LICENSE-libwinpthread.txt'
  'binaries/ffmpeg-provenance.json' = 'ffmpeg-provenance.json'
}
$nonLegacyResourceMap = [ordered]@{
  'binaries/LICENSE-ffmpeg.txt' = 'LICENSE-ffmpeg.txt'
  'binaries/ffmpeg-provenance.json' = 'ffmpeg-provenance.json'
}
$currentExpectedResourceMap = $nonLegacyResourceMap
if ([bool]$manifest.vendor.legacyBootstrapAllowedForRoutineVerification) {
  $currentExpectedResourceMap = $legacyResourceMap
}

Assert-ExactStringMap `
  -Actual $resourcesProperty.Value `
  -Expected $currentExpectedResourceMap `
  -Label 'The current Tauri bundle.resources map'

if ($externalBinProperty.Value -isnot [System.Array]) {
  throw "The current Tauri bundle.externalBin setting must be an exact one-entry JSON array."
}

$externalBins = @($externalBinProperty.Value)
if ($externalBins.Count -ne 1 -or
    $externalBins[0] -isnot [string] -or
    -not [string]::Equals(
      $externalBins[0],
      'binaries/ffmpeg',
      [System.StringComparison]::Ordinal
    )) {
  throw "The current Tauri bundle.externalBin setting must contain only binaries/ffmpeg."
}

if (-not $TargetVersion -or $TargetVersion -notmatch '^\d+\.\d+\.\d+$') {
  throw "-TargetVersion must be an exact three-part FFmpeg release in update mode."
}

if ($TargetVersion -eq "$($manifest.version)") {
  throw "Update mode requires a version different from the committed manifest."
}

if ($ExpectedSourceSha256 -and $ExpectedSourceSha256 -notmatch '^[0-9a-f]{64}$') {
  throw "-ExpectedSourceSha256 must be a lowercase SHA-256 when provided."
}

if ($ExpectedSourceSha256 -and $ExpectedSourceSha256 -eq "$($manifest.sourceArchiveSha256)") {
  throw "A new FFmpeg version cannot reuse the committed previous version's source SHA-256."
}

if ($SourceDateEpoch -gt 0 -and
    $SourceDateEpoch -ne [long]$env:SOURCE_DATE_EPOCH) {
  throw "-SourceDateEpoch must exactly match the protected workflow SOURCE_DATE_EPOCH."
}

$SourceDateEpoch = [long]$env:SOURCE_DATE_EPOCH

if ($SourceDateEpoch -le 0) {
  throw "-SourceDateEpoch must be a positive pinned Unix timestamp in update mode."
}

if ($TargetVersion -eq '8.1.2' -and $SourceDateEpoch -ne 1781654400) {
  throw "FFmpeg 8.1.2 must use the pinned SOURCE_DATE_EPOCH 1781654400."
}

$canonicalDownloadCacheRoot = [System.IO.Path]::GetFullPath(
  (Join-Path $env:RUNNER_TEMP "stickerfit-ffmpeg-download-cache-$($env:GITHUB_RUN_ID)")
).TrimEnd(
  [System.IO.Path]::DirectorySeparatorChar,
  [System.IO.Path]::AltDirectorySeparatorChar
)
if ($PSBoundParameters.ContainsKey('DownloadCacheRoot')) {
  if ([string]::IsNullOrWhiteSpace($DownloadCacheRoot)) {
    throw "Update mode does not accept an empty explicit download cache root."
  }

  $requestedDownloadCacheRoot = [System.IO.Path]::GetFullPath($DownloadCacheRoot).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  if (-not [string]::Equals(
      $requestedDownloadCacheRoot,
      $canonicalDownloadCacheRoot,
      [System.StringComparison]::OrdinalIgnoreCase
    )) {
    throw "Update mode only permits its canonical RUNNER_TEMP download cache root."
  }
}

$DownloadCacheRoot = $canonicalDownloadCacheRoot

if (-not $StagingRoot) {
  $StagingRoot = Join-Path $env:RUNNER_TEMP "stickerfit-ffmpeg-vendor-$($env:GITHUB_RUN_ID)"
}

$requestedStagingRoot = [System.IO.Path]::GetFullPath($StagingRoot).TrimEnd(
  [System.IO.Path]::DirectorySeparatorChar,
  [System.IO.Path]::AltDirectorySeparatorChar
)
$downloadCachePrefix = $canonicalDownloadCacheRoot + [System.IO.Path]::DirectorySeparatorChar
$stagingPrefix = $requestedStagingRoot + [System.IO.Path]::DirectorySeparatorChar
if ([string]::Equals(
    $requestedStagingRoot,
    $canonicalDownloadCacheRoot,
    [System.StringComparison]::OrdinalIgnoreCase
  ) -or
    $requestedStagingRoot.StartsWith(
      $downloadCachePrefix,
      [System.StringComparison]::OrdinalIgnoreCase
    ) -or
    $canonicalDownloadCacheRoot.StartsWith(
      $stagingPrefix,
      [System.StringComparison]::OrdinalIgnoreCase
    )) {
  throw "The protected output staging root and download cache must be disjoint."
}

$resolvedStagingRoot = Reset-ProtectedStagingRoot `
  -Path $StagingRoot `
  -Workspace $WorkspaceRoot `
  -AllowedRoot $env:RUNNER_TEMP `
  -RunId $env:GITHUB_RUN_ID
$protectedDownloadCacheRoot = $null
$workRoot = $null
try {
  $protectedDownloadCacheRoot = Reset-ProtectedStagingRoot `
    -Path $canonicalDownloadCacheRoot `
    -Workspace $WorkspaceRoot `
    -AllowedRoot $env:RUNNER_TEMP `
    -RunId $env:GITHUB_RUN_ID
  $DownloadCacheRoot = $protectedDownloadCacheRoot

  $workRoot = Reset-ProtectedStagingRoot `
    -Path (Join-Path $env:RUNNER_TEMP "stickerfit-ffmpeg-build-work-$($env:GITHUB_RUN_ID)") `
    -Workspace $WorkspaceRoot `
    -AllowedRoot $env:RUNNER_TEMP `
    -RunId $env:GITHUB_RUN_ID
  $sourceContainer = Join-Path $workRoot 'source'
  $payloadRoot = $resolvedStagingRoot
  New-Item -ItemType Directory -Path $sourceContainer | Out-Null

$verifiedSource = Invoke-FfmpegSourceVerification `
  -WorkspaceRoot $WorkspaceRoot `
  -ManifestPath $ManifestPath `
  -DownloadCacheRoot $DownloadCacheRoot `
  -VersionOverride $TargetVersion `
  -ExpectedSourceSha256 $ExpectedSourceSha256 `
  -GpgPath $GpgPath `
  -GpgvPath $GpgvPath `
  -TarPath $TarPath
$resolvedSourceSha256 = "$($verifiedSource.ArchiveSha256)"
if ($resolvedSourceSha256 -eq "$($manifest.sourceArchiveSha256)") {
  throw "Verified replacement source hash unexpectedly equals the committed previous version's source hash."
}

if (-not $TarPath) {
  $systemTar = Join-Path $env:SystemRoot "System32\tar.exe"
  $TarPath = Resolve-FfmpegTool -Name 'tar.exe' -PreferredPaths @($systemTar)
}

Invoke-StrictNativeCommand `
  -Executable $TarPath `
  -Arguments @('-xf', $verifiedSource.ArchivePath, '-C', $sourceContainer) `
  -FailureMessage "Could not extract the freshly verified FFmpeg source archive." | Out-Null
$sourceRoot = Join-Path $sourceContainer $verifiedSource.ExpectedTopLevelDirectory
if (-not (Test-Path -LiteralPath $sourceRoot -PathType Container)) {
  throw "Fresh FFmpeg extraction did not create the expected source directory."
}

$visualStudioTools = Resolve-Dumpbin -VswherePath $env:STICKERFIT_VSWHERE_PATH
$exportedPacmanPath = if ($env:STICKERFIT_MSYS2_LOCATION) {
  Join-Path $env:STICKERFIT_MSYS2_LOCATION 'usr\bin\pacman.exe'
}
else {
  $null
}
$pacmanPath = Resolve-FfmpegTool `
  -Name 'pacman.exe' `
  -PreferredPaths @($exportedPacmanPath, 'C:\msys64\usr\bin\pacman.exe')
$msys2ToolPaths = [ordered]@{
  gcc = Join-Path $env:STICKERFIT_MSYS2_LOCATION 'mingw64\bin\gcc.exe'
  binutils = Join-Path $env:STICKERFIT_MSYS2_LOCATION 'mingw64\bin\ld.exe'
  make = Join-Path $env:STICKERFIT_MSYS2_LOCATION 'usr\bin\make.exe'
  nasm = Join-Path $env:STICKERFIT_MSYS2_LOCATION 'usr\bin\nasm.exe'
  pkgconf = Join-Path $env:STICKERFIT_MSYS2_LOCATION 'mingw64\bin\pkgconf.exe'
  strip = Join-Path $env:STICKERFIT_MSYS2_LOCATION 'mingw64\bin\strip.exe'
}
foreach ($msys2ToolPath in $msys2ToolPaths.GetEnumerator()) {
  if (-not (Test-Path -LiteralPath $msys2ToolPath.Value -PathType Leaf)) {
    throw "Protected MSYS2 tool path is missing: $($msys2ToolPath.Key)"
  }
}

$protectedVersions = [ordered]@{
  pacman = Get-FirstNativeVersionLine -Executable $pacmanPath -Arguments @('-Q', 'pacman') -Label 'pacman package'
  bash = Get-FirstNativeVersionLine -Executable $BashPath -Arguments @('--version') -Label 'bash'
  gpg = Get-FirstNativeVersionLine -Executable $GpgPath -Arguments @('--version') -Label 'gpg'
  gpgv = Get-FirstNativeVersionLine -Executable $GpgvPath -Arguments @('--version') -Label 'gpgv'
  gcc = Get-FirstNativeVersionLine -Executable $msys2ToolPaths.gcc -Arguments @('--version') -Label 'GCC'
  binutils = Get-FirstNativeVersionLine -Executable $msys2ToolPaths.binutils -Arguments @('--version') -Label 'binutils ld'
  make = Get-FirstNativeVersionLine -Executable $msys2ToolPaths.make -Arguments @('--version') -Label 'make'
  nasm = Get-FirstNativeVersionLine -Executable $msys2ToolPaths.nasm -Arguments @('-v') -Label 'nasm'
  pkgconf = Get-FirstNativeVersionLine -Executable $msys2ToolPaths.pkgconf -Arguments @('--version') -Label 'pkgconf'
  strip = Get-FirstNativeVersionLine -Executable $msys2ToolPaths.strip -Arguments @('--version') -Label 'strip'
  tar = Get-FirstNativeVersionLine -Executable $TarPath -Arguments @('--version') -Label 'tar'
}
$directVersionContracts = [ordered]@{
  pacman = 'STICKERFIT_VENDOR_PACMAN_VERSION'
  bash = 'STICKERFIT_VENDOR_BASH_VERSION'
  gpg = 'STICKERFIT_VENDOR_GPG_VERSION'
  gpgv = 'STICKERFIT_VENDOR_GPGV_VERSION'
  gcc = 'STICKERFIT_VENDOR_GCC_VERSION'
  binutils = 'STICKERFIT_VENDOR_BINUTILS_VERSION'
  make = 'STICKERFIT_VENDOR_MAKE_VERSION'
  nasm = 'STICKERFIT_VENDOR_NASM_VERSION'
  pkgconf = 'STICKERFIT_VENDOR_PKGCONF_VERSION'
}
foreach ($versionContract in $directVersionContracts.GetEnumerator()) {
  Assert-ExportedToolVersion `
    -EnvironmentName $versionContract.Value `
    -ActualVersion $protectedVersions[$versionContract.Key]
}

$configureArguments = @(Get-FfmpegConfigureArguments -Manifest $manifest)
$configureLine = @($configureArguments | ForEach-Object {
    ConvertTo-BashSingleQuotedString -Value $_
  }) -join ' '

$targetFileName = "ffmpeg-$($manifest.targetTriple).exe"
$stagedExecutable = Join-Path $payloadRoot $targetFileName
$stagedLicense = Join-Path $payloadRoot 'LICENSE-ffmpeg.txt'
$buildScriptPath = Join-Path $workRoot 'build-verified-ffmpeg.sh'
$sourceRootLiteral = ConvertTo-BashSingleQuotedString -Value $sourceRoot
$payloadRootLiteral = ConvertTo-BashSingleQuotedString -Value $payloadRoot
$targetFileNameLiteral = ConvertTo-BashSingleQuotedString -Value $targetFileName
$parallelJobs = [Math]::Max(1, [Environment]::ProcessorCount)

$buildScript = @"
#!/usr/bin/env bash
set -euo pipefail

export MSYSTEM=MINGW64
export PATH=/mingw64/bin:/usr/bin:`$PATH
export SOURCE_DATE_EPOCH=$SourceDateEpoch
export TZ=UTC
export LC_ALL=C
export LANG=C
export ZERO_AR_DATE=1

SOURCE_ROOT_WIN=$sourceRootLiteral
PAYLOAD_ROOT_WIN=$payloadRootLiteral
TARGET_FILE_NAME=$targetFileNameLiteral
SOURCE_ROOT=`"`$(cygpath -u "`$SOURCE_ROOT_WIN")`"
PAYLOAD_ROOT=`"`$(cygpath -u "`$PAYLOAD_ROOT_WIN")`"

for required in gcc ld make nasm pkgconf strip cygpath; do
  command -v "`$required" >/dev/null 2>&1 || {
    echo "Required MSYS2 tool is missing: `$required" >&2
    exit 1
  }
done

cd "`$SOURCE_ROOT"
export CFLAGS="-O2 -ffile-prefix-map=`$SOURCE_ROOT=/usr/src/ffmpeg-$TargetVersion -fdebug-prefix-map=`$SOURCE_ROOT=/usr/src/ffmpeg-$TargetVersion"
export LDFLAGS="-static -static-libgcc -Wl,--no-insert-timestamp"

./configure $configureLine
make "-j$parallelJobs"
strip --strip-all ffmpeg.exe
install -m 0755 ffmpeg.exe "`$PAYLOAD_ROOT/`$TARGET_FILE_NAME"
install -m 0644 COPYING.LGPLv2.1 "`$PAYLOAD_ROOT/LICENSE-ffmpeg.txt"

printf 'STICKERFIT_TOOLCHAIN_BASH=%s\n' "`$(bash --version | sed -n '1p')"
printf 'STICKERFIT_TOOLCHAIN_GCC=%s\n' "`$(gcc --version | sed -n '1p')"
printf 'STICKERFIT_TOOLCHAIN_BINUTILS=%s\n' "`$(ld --version | sed -n '1p')"
printf 'STICKERFIT_TOOLCHAIN_MAKE=%s\n' "`$(make --version | sed -n '1p')"
printf 'STICKERFIT_TOOLCHAIN_NASM=%s\n' "`$(nasm -v)"
printf 'STICKERFIT_TOOLCHAIN_PKGCONF=%s\n' "`$(pkgconf --version)"
printf 'STICKERFIT_TOOLCHAIN_STRIP=%s\n' "`$(strip --version | sed -n '1p')"
"@
[System.IO.File]::WriteAllText(
  $buildScriptPath,
  $buildScript,
  [System.Text.UTF8Encoding]::new($false)
)

$buildResult = Invoke-StrictNativeCommand `
  -Executable $BashPath `
  -Arguments @('--noprofile', '--norc', $buildScriptPath) `
  -FailureMessage "Verified minimal FFmpeg build failed."

if (-not (Test-Path -LiteralPath $stagedExecutable -PathType Leaf) -or
    -not (Test-Path -LiteralPath $stagedLicense -PathType Leaf)) {
  throw "The protected build did not produce the staged executable and license."
}

$toolchain = [ordered]@{}
foreach ($line in @($buildResult.Output)) {
  if ($line -match '^STICKERFIT_TOOLCHAIN_([A-Z]+)=(.+)$') {
    $toolchain[$Matches[1].ToLowerInvariant()] = $Matches[2]
  }
}

foreach ($requiredToolchainKey in @('bash', 'gcc', 'binutils', 'make', 'nasm', 'pkgconf', 'strip')) {
  if (-not $toolchain.Contains($requiredToolchainKey)) {
    throw "Protected build did not report exact toolchain data for: $requiredToolchainKey"
  }
}

$buildVersionContracts = [ordered]@{
  bash = 'STICKERFIT_VENDOR_BASH_VERSION'
  gcc = 'STICKERFIT_VENDOR_GCC_VERSION'
  binutils = 'STICKERFIT_VENDOR_BINUTILS_VERSION'
  make = 'STICKERFIT_VENDOR_MAKE_VERSION'
  nasm = 'STICKERFIT_VENDOR_NASM_VERSION'
  pkgconf = 'STICKERFIT_VENDOR_PKGCONF_VERSION'
}
foreach ($versionContract in $buildVersionContracts.GetEnumerator()) {
  Assert-ExportedToolVersion `
    -EnvironmentName $versionContract.Value `
    -ActualVersion $toolchain[$versionContract.Key]
}

foreach ($directVersion in $protectedVersions.GetEnumerator()) {
  $toolchain[$directVersion.Key] = $directVersion.Value
}

$toolchain['msys2Runtime'] = Get-FirstNativeVersionLine `
  -Executable $pacmanPath `
  -Arguments @('-Q', 'msys2-runtime') `
  -Label 'MSYS2 runtime package'

$toolchain['vswhere'] = [System.Diagnostics.FileVersionInfo]::GetVersionInfo(
  $visualStudioTools.VswherePath
).FileVersion
$toolchain['dumpbin'] = [System.Diagnostics.FileVersionInfo]::GetVersionInfo(
  $visualStudioTools.DumpbinPath
).FileVersion
if (-not $toolchain.vswhere -or -not $toolchain.dumpbin) {
  throw "Visual Studio verification tools did not expose exact file versions."
}

$toolPaths = [ordered]@{
  bash = $BashPath
  gpg = $GpgPath
  gpgv = $GpgvPath
  gcc = $msys2ToolPaths.gcc
  binutils = $msys2ToolPaths.binutils
  make = $msys2ToolPaths.make
  nasm = $msys2ToolPaths.nasm
  pkgconf = $msys2ToolPaths.pkgconf
  strip = $msys2ToolPaths.strip
  tar = $TarPath
  pacman = $pacmanPath
  vswhere = $visualStudioTools.VswherePath
  dumpbin = $visualStudioTools.DumpbinPath
}

$systemDependencies = @(Get-DumpbinDependencies `
    -DumpbinPath $visualStudioTools.DumpbinPath `
    -ExecutablePath $stagedExecutable)
if (Test-Path -LiteralPath (Join-Path $payloadRoot 'libwinpthread-1.dll')) {
  throw "Protected FFmpeg payload must not contain the legacy libwinpthread runtime DLL."
}

$embeddedText = [System.Text.Encoding]::ASCII.GetString(
  [System.IO.File]::ReadAllBytes($stagedExecutable)
)
if (-not $embeddedText.Contains(($configureArguments -join ' '))) {
  throw "Staged FFmpeg executable does not contain the exact protected configure argument string."
}

$artifactSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $stagedExecutable).Hash.ToLowerInvariant()
$artifactBytes = (Get-Item -LiteralPath $stagedExecutable).Length
$licenseSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $stagedLicense).Hash.ToLowerInvariant()
$licenseBytes = (Get-Item -LiteralPath $stagedLicense).Length
$archiveBytes = (Get-Item -LiteralPath $verifiedSource.ArchivePath).Length

$updatedManifest = $manifest | ConvertTo-Json -Depth 20 | ConvertFrom-Json
$updatedManifest.version = $TargetVersion
$updatedManifest.releaseKeyFingerprint = "$($manifest.releaseKeyFingerprint)"
$updatedManifest.sourceArchiveSha256 = $resolvedSourceSha256
$updatedManifest.expectedExeSha256 = $artifactSha256
$updatedManifest.vendorUpdateRunId = [long]$env:GITHUB_RUN_ID
$updatedManifest.release.archiveName = "ffmpeg-$TargetVersion.tar.xz"
$updatedManifest.release.archiveUrl = "https://ffmpeg.org/releases/ffmpeg-$TargetVersion.tar.xz"
$updatedManifest.release.signatureUrl = "https://ffmpeg.org/releases/ffmpeg-$TargetVersion.tar.xz.asc"
$updatedManifest.release.archiveSha256 = $resolvedSourceSha256
$updatedManifest.release.archiveBytes = $archiveBytes
$updatedManifest.release.expectedTopLevelDirectory = "ffmpeg-$TargetVersion"
$updatedManifest.vendor.legacyBootstrapAllowedForRoutineVerification = $false
$updatedManifest.vendor.expectedRuntimeDependencies = @()
$updatedManifest.vendor.trackedCompanionFiles = @('LICENSE-ffmpeg.txt')

$deterministicFlags = @(
  "SOURCE_DATE_EPOCH=$SourceDateEpoch",
  'TZ=UTC',
  'LC_ALL=C',
  'LANG=C',
  'ZERO_AR_DATE=1',
  "CFLAGS=-O2 -ffile-prefix-map=<source>=/usr/src/ffmpeg-$TargetVersion -fdebug-prefix-map=<source>=/usr/src/ffmpeg-$TargetVersion",
  'LDFLAGS=-static -static-libgcc -Wl,--no-insert-timestamp'
)
$provenance = [ordered]@{
  schemaVersion = 1
  legacyBootstrap = $false
  sourceUrl = $updatedManifest.release.archiveUrl
  version = $TargetVersion
  sourceArchiveSha256 = $resolvedSourceSha256
  signingPrimaryFingerprint = "$($manifest.releaseKeyFingerprint)"
  configureArgs = $configureArguments
  toolchainVersions = $toolchain
  sourceDateEpoch = $SourceDateEpoch
  vendorUpdateRunId = [long]$env:GITHUB_RUN_ID
  exeSha256 = $artifactSha256
  targetTriple = "$($manifest.targetTriple)"
  artifact = [ordered]@{
    path = "src-tauri/binaries/$targetFileName"
    sha256 = $artifactSha256
    bytes = $artifactBytes
    origin = 'protected-workflow-artifact'
  }
  source = [ordered]@{
    archiveUrl = $updatedManifest.release.archiveUrl
    signatureUrl = $updatedManifest.release.signatureUrl
    archiveSha256 = $resolvedSourceSha256
    signingPrimaryFingerprint = "$($manifest.release.primaryFingerprint)"
    archiveSignatureVerifiedSeparately = $true
    artifactBuiltFromVerifiedSource = $true
  }
  build = [ordered]@{
    approvedWorkflow = $true
    runId = [long]$env:GITHUB_RUN_ID
    sourceCommit = $env:GITHUB_SHA
    authority = $vendorAuthority
    sourceDateEpoch = $SourceDateEpoch
    toolchain = $toolchain
    toolPaths = $toolPaths
    deterministicFlags = $deterministicFlags
    configurationObservation = 'protected build invocation and embedded ASCII configure string'
    configureArguments = $configureArguments
  }
  runtimeDependencies = @()
  systemDependencies = $systemDependencies
  companionFiles = @(
    [ordered]@{
      path = 'src-tauri/binaries/LICENSE-ffmpeg.txt'
      sha256 = $licenseSha256
      bytes = $licenseBytes
    }
  )
}

$stagedProvenancePath = Join-Path $payloadRoot 'ffmpeg-provenance.json'
$stagedManifestPath = Join-Path $payloadRoot 'ffmpeg-version.json'
$stagedTauriConfigPath = Join-Path $payloadRoot 'tauri.conf.json'
$updatedTauriConfig = $tauriConfig | ConvertTo-Json -Depth 20 | ConvertFrom-Json
$updatedTauriConfig.bundle.resources = [pscustomobject]$nonLegacyResourceMap
Assert-ExactStringMap `
  -Actual $updatedTauriConfig.bundle.resources `
  -Expected $nonLegacyResourceMap `
  -Label 'The staged Tauri bundle.resources map'

Write-FfmpegJson -Path $stagedProvenancePath -Value $provenance
Write-FfmpegJson -Path $stagedManifestPath -Value $updatedManifest
Write-FfmpegJson -Path $stagedTauriConfigPath -Value $updatedTauriConfig
$stagedProvenanceSha256 = (
  Get-FileHash -Algorithm SHA256 -LiteralPath $stagedProvenancePath
).Hash.ToLowerInvariant()
$stagedProvenanceBytes = (Get-Item -LiteralPath $stagedProvenancePath).Length
$stagedManifestSha256 = (
  Get-FileHash -Algorithm SHA256 -LiteralPath $stagedManifestPath
).Hash.ToLowerInvariant()
$stagedManifestBytes = (Get-Item -LiteralPath $stagedManifestPath).Length
$stagedTauriConfigSha256 = (
  Get-FileHash -Algorithm SHA256 -LiteralPath $stagedTauriConfigPath
).Hash.ToLowerInvariant()
$stagedTauriConfigBytes = (Get-Item -LiteralPath $stagedTauriConfigPath).Length

$updateFragment = [ordered]@{
  schemaVersion = 1
  applyOnlyAfterProtectedReview = $true
  sourceWorkflow = [ordered]@{
    workflowRef = $env:GITHUB_WORKFLOW_REF
    runId = [long]$env:GITHUB_RUN_ID
    sourceCommit = $env:GITHUB_SHA
  }
  preconditions = [ordered]@{
    currentVersion = "$($manifest.version)"
    currentSourceSha256 = "$($manifest.sourceArchiveSha256)"
    replacementVersion = $TargetVersion
    replacementSourceSha256 = $resolvedSourceSha256
  }
  replacements = @(
    [ordered]@{
      source = $targetFileName
      destination = "src-tauri/binaries/$targetFileName"
      sha256 = $artifactSha256
      bytes = $artifactBytes
    },
    [ordered]@{
      source = 'LICENSE-ffmpeg.txt'
      destination = 'src-tauri/binaries/LICENSE-ffmpeg.txt'
      sha256 = $licenseSha256
      bytes = $licenseBytes
    },
    [ordered]@{
      source = 'ffmpeg-provenance.json'
      destination = 'src-tauri/binaries/ffmpeg-provenance.json'
      sha256 = $stagedProvenanceSha256
      bytes = $stagedProvenanceBytes
    },
    [ordered]@{
      source = 'ffmpeg-version.json'
      destination = 'tools/ffmpeg/ffmpeg-version.json'
      sha256 = $stagedManifestSha256
      bytes = $stagedManifestBytes
    },
    [ordered]@{
      source = 'tauri.conf.json'
      destination = 'src-tauri/tauri.conf.json'
      sha256 = $stagedTauriConfigSha256
      bytes = $stagedTauriConfigBytes
    }
  )
  deletions = @(
    'src-tauri/binaries/libwinpthread-1.dll',
    'src-tauri/binaries/LICENSE-ffmpeg-BtbN.txt',
    'src-tauri/binaries/LICENSE-libwinpthread.txt'
  )
}
$updateFragmentPath = Join-Path $resolvedStagingRoot 'update-fragment.json'
Write-FfmpegJson -Path $updateFragmentPath -Value $updateFragment

$hashLines = @(
  "$artifactSha256  $targetFileName",
  "$licenseSha256  LICENSE-ffmpeg.txt",
  "$stagedProvenanceSha256  ffmpeg-provenance.json",
  "$stagedManifestSha256  ffmpeg-version.json",
  "$stagedTauriConfigSha256  tauri.conf.json",
  "$((Get-FileHash -Algorithm SHA256 -LiteralPath $updateFragmentPath).Hash.ToLowerInvariant())  update-fragment.json"
)
[System.IO.File]::WriteAllLines(
  (Join-Path $resolvedStagingRoot 'SHA256SUMS.txt'),
  $hashLines,
  [System.Text.UTF8Encoding]::new($false)
)

$outputMarkerPath = Join-Path $resolvedStagingRoot '.stickerfit-ffmpeg-staging.marker'
$expectedOutputMarker = "stickerfit-ffmpeg-vendor-staging-v1:$($env:GITHUB_RUN_ID)"
if (-not (Test-Path -LiteralPath $outputMarkerPath -PathType Leaf) -or
    (Get-Content -Raw -LiteralPath $outputMarkerPath) -ne $expectedOutputMarker) {
  throw "Protected output staging marker changed before finalization."
}

Remove-Item -LiteralPath $outputMarkerPath -Force

  Write-Host "Protected FFmpeg $TargetVersion vendor payload staged at:"
  Write-Host "  $resolvedStagingRoot"
  Write-Host "No repository files were replaced and no Git operation was performed."
}
finally {
  try {
    if ($workRoot -and (Test-Path -LiteralPath $workRoot -PathType Container)) {
      Remove-ProtectedStagingRoot `
        -Path $workRoot `
        -Workspace $WorkspaceRoot `
        -AllowedRoot $env:RUNNER_TEMP `
        -RunId $env:GITHUB_RUN_ID
    }
  }
  finally {
    if ($protectedDownloadCacheRoot -and
        (Test-Path -LiteralPath $protectedDownloadCacheRoot)) {
      Remove-ProtectedStagingRoot `
        -Path $protectedDownloadCacheRoot `
        -Workspace $WorkspaceRoot `
        -AllowedRoot $env:RUNNER_TEMP `
        -RunId $env:GITHUB_RUN_ID
    }
  }
}
