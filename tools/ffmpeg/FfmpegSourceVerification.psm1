Set-StrictMode -Version Latest

function Get-FfmpegFileSha256 {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  $stream = [System.IO.File]::OpenRead($Path)
  try {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
      return ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    }
    finally {
      $sha256.Dispose()
    }
  }
  finally {
    $stream.Dispose()
  }
}

function Resolve-RepositoryPath {
  param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceRoot,

    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  $root = [System.IO.Path]::GetFullPath($WorkspaceRoot).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )

  if ([System.IO.Path]::IsPathRooted($Path)) {
    $candidate = [System.IO.Path]::GetFullPath($Path)
  }
  else {
    $candidate = [System.IO.Path]::GetFullPath((Join-Path $root $Path))
  }

  $rootPrefix = $root + [System.IO.Path]::DirectorySeparatorChar
  if (-not $candidate.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Path must resolve below the workspace root: $Path"
  }

  return $candidate
}

function Resolve-FfmpegTool {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name,

    [string[]]$PreferredPaths = @()
  )

  foreach ($preferredPath in @($PreferredPaths)) {
    if ($preferredPath -and (Test-Path -LiteralPath $preferredPath -PathType Leaf)) {
      return (Resolve-Path -LiteralPath $preferredPath).Path
    }
  }

  $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if ($command) {
    return $command.Source
  }

  throw "Required native tool was not found: $Name"
}

function Invoke-StrictNativeCommand {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,

    [string[]]$Arguments = @(),

    [string]$WorkingDirectory,

    [string]$FailureMessage = "Native command failed."
  )

  if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "Native executable does not exist: $Executable"
  }

  $previousLocation = $null
  $previousErrorActionPreference = $ErrorActionPreference
  $output = @()
  $exitCode = $null
  $invocationSucceeded = $false
  try {
    if ($WorkingDirectory) {
      $previousLocation = Get-Location
      Set-Location -LiteralPath $WorkingDirectory
    }

    # Windows PowerShell 5.1 promotes ordinary native stderr to a
    # NativeCommandError when the caller uses Stop. Capture it and decide
    # success exclusively from the native exit code instead.
    $ErrorActionPreference = "Continue"
    $global:LASTEXITCODE = $null
    try {
      $output = @(& $Executable @Arguments 2>&1 | ForEach-Object { "$_" })
      $invocationSucceeded = $?
    }
    catch {
      $output = @("$($_.Exception.Message)")
      $invocationSucceeded = $false
    }
    $exitCode = $global:LASTEXITCODE
  }
  finally {
    $ErrorActionPreference = $previousErrorActionPreference
    if ($null -ne $previousLocation) {
      Set-Location -LiteralPath $previousLocation.Path
    }
  }

  if (-not $invocationSucceeded -or $null -eq $exitCode -or $exitCode -ne 0) {
    $detail = ($output -join [Environment]::NewLine).Trim()
    $status = if ($null -eq $exitCode) { "unavailable" } else { "$exitCode" }
    if ($detail) {
      throw "$FailureMessage Exit code: $status. $detail"
    }

    throw "$FailureMessage Exit code: $status."
  }

  return [pscustomobject]@{
    ExitCode = $exitCode
    Output = @($output)
  }
}

function Read-FfmpegJson {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "Required JSON file was not found: $Path"
  }

  try {
    return (Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json)
  }
  catch {
    throw "Invalid JSON file '$Path': $($_.Exception.Message)"
  }
}

function Write-FfmpegJson {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [object]$Value
  )

  $parent = Split-Path -Parent $Path
  if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
  }

  $temporaryPath = "$Path.$([Guid]::NewGuid().ToString('N')).tmp"
  $json = ($Value | ConvertTo-Json -Depth 20) -replace "`r`n?", "`n"
  [System.IO.File]::WriteAllText(
    $temporaryPath,
    $json + "`n",
    [System.Text.UTF8Encoding]::new($false)
  )
  Move-Item -LiteralPath $temporaryPath -Destination $Path -Force
}

function Assert-ExactSequence {
  param(
    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [object[]]$Actual,

    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [object[]]$Expected,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  $actualValues = @($Actual | ForEach-Object { "$_" })
  $expectedValues = @($Expected | ForEach-Object { "$_" })
  if ($actualValues.Count -ne $expectedValues.Count) {
    throw "$Label count mismatch. Expected $($expectedValues.Count), found $($actualValues.Count)."
  }

  for ($index = 0; $index -lt $expectedValues.Count; $index += 1) {
    if (-not [string]::Equals(
        $actualValues[$index],
        $expectedValues[$index],
        [System.StringComparison]::Ordinal
      )) {
      throw "$Label mismatch at index $index. Expected '$($expectedValues[$index])', found '$($actualValues[$index])'."
    }
  }
}

function Assert-FfmpegManifest {
  param(
    [Parameter(Mandatory = $true)]
    [object]$Manifest
  )

  if ([int]$Manifest.schemaVersion -ne 1) {
    throw "Unsupported FFmpeg manifest schemaVersion: $($Manifest.schemaVersion)"
  }

  $version = "$($Manifest.version)"
  if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "FFmpeg manifest version must be an exact three-part release: $version"
  }

  if ("$($Manifest.targetTriple)" -ne "x86_64-pc-windows-msvc") {
    throw "Unsupported FFmpeg target triple: $($Manifest.targetTriple)"
  }

  $archiveName = "ffmpeg-$version.tar.xz"
  $archiveUrl = "https://ffmpeg.org/releases/$archiveName"
  if ("$($Manifest.release.archiveName)" -ne $archiveName -or
      "$($Manifest.release.archiveUrl)" -ne $archiveUrl -or
      "$($Manifest.release.signatureUrl)" -ne "$archiveUrl.asc" -or
      "$($Manifest.release.expectedTopLevelDirectory)" -ne "ffmpeg-$version") {
    throw "FFmpeg release metadata must use the canonical ffmpeg.org URLs and archive names."
  }

  if ("$($Manifest.release.archiveSha256)" -notmatch '^[0-9a-f]{64}$') {
    throw "FFmpeg archive SHA-256 must be 64 lowercase hexadecimal characters."
  }

  if ("$($Manifest.sourceArchiveSha256)" -ne "$($Manifest.release.archiveSha256)") {
    throw "Top-level FFmpeg sourceArchiveSha256 must match release.archiveSha256."
  }

  if ("$($Manifest.release.signingKeySha256)" -notmatch '^[0-9a-f]{64}$') {
    throw "FFmpeg signing-key SHA-256 must be 64 lowercase hexadecimal characters."
  }

  if ("$($Manifest.release.primaryFingerprint)" -notmatch '^[0-9A-F]{40}$') {
    throw "FFmpeg release primary fingerprint must be 40 uppercase hexadecimal characters."
  }

  if ("$($Manifest.releaseKeyFingerprint)" -ne "$($Manifest.release.primaryFingerprint)") {
    throw "Top-level FFmpeg releaseKeyFingerprint must match release.primaryFingerprint."
  }

  if ("$($Manifest.expectedExeSha256)" -notmatch '^[0-9a-f]{64}$') {
    throw "Top-level FFmpeg expectedExeSha256 must be 64 lowercase hexadecimal characters."
  }

  if ($Manifest.vendor.legacyBootstrapAllowedForRoutineVerification -isnot [bool]) {
    throw "FFmpeg manifest legacy bootstrap policy must be a JSON boolean."
  }

  if ([bool]$Manifest.vendor.legacyBootstrapAllowedForRoutineVerification) {
    if ($null -ne $Manifest.vendorUpdateRunId) {
      throw "Legacy FFmpeg manifest vendorUpdateRunId must be explicit null."
    }
  }
  elseif ("$($Manifest.vendorUpdateRunId)" -notmatch '^\d+$') {
    throw "Protected FFmpeg manifest vendorUpdateRunId must be numeric."
  }

  if ("$($Manifest.release.signingKeyPath)" -ne "tools/ffmpeg/ffmpeg-release-signing-key.asc") {
    throw "FFmpeg signing key path is not the protected repository path."
  }

  $parsers = @($Manifest.configure.capabilities.parsers | ForEach-Object { "$_" })
  if ($parsers -notcontains "mpeg4video" -or $parsers -contains "mpeg4") {
    throw "New FFmpeg builds must configure the MPEG-4 parser as 'mpeg4video'."
  }

  foreach ($property in @(
      'demuxers',
      'decoders',
      'muxers',
      'encoders',
      'parsers',
      'protocols',
      'filters'
    )) {
    $values = @($Manifest.configure.capabilities.$property | ForEach-Object { "$_" })
    if ($values.Count -eq 0) {
      throw "FFmpeg capability list cannot be empty: $property"
    }

    if (@($values | Sort-Object -Unique).Count -ne $values.Count) {
      throw "FFmpeg capability list contains duplicates: $property"
    }
  }
}

function Get-FfmpegManifest {
  param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceRoot,

    [string]$ManifestPath = "tools/ffmpeg/ffmpeg-version.json"
  )

  $resolvedManifestPath = Resolve-RepositoryPath -WorkspaceRoot $WorkspaceRoot -Path $ManifestPath
  $manifest = Read-FfmpegJson -Path $resolvedManifestPath
  Assert-FfmpegManifest -Manifest $manifest
  return $manifest
}

function Get-FfmpegConfigureArguments {
  param(
    [Parameter(Mandatory = $true)]
    [object]$Manifest,

    [switch]$LegacyBootstrap
  )

  $arguments = New-Object System.Collections.Generic.List[string]
  foreach ($argument in @($Manifest.configure.baseArguments)) {
    $arguments.Add("$argument")
  }

  if (-not $LegacyBootstrap) {
    foreach ($argument in @($Manifest.configure.newBuildRequiredArguments)) {
      $arguments.Add("$argument")
    }
  }

  $capabilityGroups = @(
    [pscustomobject]@{ Property = 'demuxers'; Prefix = '--enable-demuxer' },
    [pscustomobject]@{ Property = 'decoders'; Prefix = '--enable-decoder' },
    [pscustomobject]@{ Property = 'muxers'; Prefix = '--enable-muxer' },
    [pscustomobject]@{ Property = 'encoders'; Prefix = '--enable-encoder' },
    [pscustomobject]@{ Property = 'parsers'; Prefix = '--enable-parser' },
    [pscustomobject]@{ Property = 'protocols'; Prefix = '--enable-protocol' },
    [pscustomobject]@{ Property = 'filters'; Prefix = '--enable-filter' }
  )

  foreach ($group in $capabilityGroups) {
    foreach ($valueObject in @($Manifest.configure.capabilities.($group.Property))) {
      $value = "$valueObject"
      if ($LegacyBootstrap -and $group.Property -eq 'parsers') {
        $alias = $Manifest.configure.legacyParserAliases.PSObject.Properties[$value]
        if ($null -ne $alias) {
          $value = "$($alias.Value)"
        }
      }

      $arguments.Add("$($group.Prefix)=$value")
    }
  }

  return @($arguments)
}

function Assert-FfmpegFileSha256 {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedSha256,

    [string]$Label = "file"
  )

  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "Required $Label was not found: $Path"
  }

  $actualSha256 = Get-FfmpegFileSha256 -Path $Path
  if (-not [string]::Equals(
      $actualSha256,
      $ExpectedSha256,
      [System.StringComparison]::Ordinal
    )) {
    throw "$Label SHA-256 mismatch. Expected $ExpectedSha256, found $actualSha256."
  }

  return $actualSha256
}

function Assert-FfmpegFileLength {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [Parameter(Mandatory = $true)]
    [long]$ExpectedBytes,

    [string]$Label = "file"
  )

  $actualBytes = (Get-Item -LiteralPath $Path).Length
  if ($actualBytes -ne $ExpectedBytes) {
    throw "$Label byte-length mismatch. Expected $ExpectedBytes, found $actualBytes."
  }
}

function Get-FfmpegVerifiedArchiveSha256 {
  param(
    [Parameter(Mandatory = $true)]
    [string]$ArchivePath,

    [string]$ExpectedSha256,

    [string]$Label = "FFmpeg archive"
  )

  if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
    throw "Required $Label was not found: $ArchivePath"
  }

  $computedSha256 = Get-FfmpegFileSha256 -Path $ArchivePath
  if ($ExpectedSha256) {
    if ($ExpectedSha256 -notmatch '^[0-9a-f]{64}$') {
      throw "$Label expected SHA-256 must be 64 lowercase hexadecimal characters."
    }

    if (-not [string]::Equals(
        $computedSha256,
        $ExpectedSha256,
        [System.StringComparison]::Ordinal
      )) {
      throw "$Label SHA-256 mismatch. Expected $ExpectedSha256, found $computedSha256."
    }
  }

  return $computedSha256
}

function Assert-FfmpegPrimaryKeyInventory {
  param(
    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]]$ColonListing,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedPrimaryFingerprint
  )

  $primaryKeyCount = 0
  $primaryFingerprints = New-Object System.Collections.Generic.List[string]
  $awaitingPrimaryFingerprint = $false

  foreach ($line in @($ColonListing)) {
    if ($line -match '^pub:') {
      $primaryKeyCount += 1
      $awaitingPrimaryFingerprint = $true
      continue
    }

    if ($line -match '^sub:') {
      $awaitingPrimaryFingerprint = $false
      continue
    }

    if ($awaitingPrimaryFingerprint -and $line -match '^fpr:(?:[^:]*:){8}([^:]+):') {
      $primaryFingerprints.Add($Matches[1])
      $awaitingPrimaryFingerprint = $false
    }
  }

  if ($primaryKeyCount -ne 1 -or $primaryFingerprints.Count -ne 1) {
    throw "Signing key file must contain exactly one primary key and one primary fingerprint."
  }

  if (-not [string]::Equals(
      $primaryFingerprints[0],
      $ExpectedPrimaryFingerprint,
      [System.StringComparison]::Ordinal
    )) {
    throw "Signing key primary fingerprint mismatch. Expected $ExpectedPrimaryFingerprint, found $($primaryFingerprints[0])."
  }
}

function Assert-FfmpegValidSignatureStatus {
  param(
    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]]$StatusLines,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedPrimaryFingerprint
  )

  $badSignatureLines = @($StatusLines | Where-Object {
      $_ -match '^\[GNUPG:\]\s+(?:BADSIG|ERRSIG|NO_PUBKEY|EXPKEYSIG|EXPSIG|REVKEYSIG)\b'
    })
  if ($badSignatureLines.Count -ne 0) {
    throw "Signature verification emitted a bad signature or key status."
  }

  $validSignatureLines = @($StatusLines | Where-Object {
      $_ -match '^\[GNUPG:\]\s+VALIDSIG\s+'
    })

  if ($validSignatureLines.Count -ne 1) {
    throw "Signature verification must emit exactly one VALIDSIG record; found $($validSignatureLines.Count)."
  }

  $parts = @($validSignatureLines[0] -split '\s+')
  if ($parts.Count -lt 4) {
    throw "Malformed VALIDSIG status record."
  }

  $signingFingerprint = $parts[2]
  $primaryFingerprint = $parts[$parts.Count - 1]
  if (-not [string]::Equals(
      $signingFingerprint,
      $ExpectedPrimaryFingerprint,
      [System.StringComparison]::Ordinal
    ) -or -not [string]::Equals(
      $primaryFingerprint,
      $ExpectedPrimaryFingerprint,
      [System.StringComparison]::Ordinal
    )) {
    throw "VALIDSIG fingerprint mismatch. Expected primary key $ExpectedPrimaryFingerprint."
  }
}

function Test-FfmpegArchiveEntry {
  param(
    [Parameter(Mandatory = $true)]
    [string]$EntryName,

    [Parameter(Mandatory = $true)]
    [string]$EntryType,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedTopLevelDirectory
  )

  if ($EntryType -notin @('-', 'd')) {
    throw "FFmpeg source archive contains a link, device, FIFO, socket, or other special entry: $EntryName"
  }

  if (-not $EntryName -or $EntryName -match '[\x00-\x1f\x7f]') {
    throw "FFmpeg source archive contains an empty or control-character path."
  }

  $normalized = $EntryName.Replace('\', '/')
  if ($normalized.StartsWith('/') -or
      $normalized.StartsWith('//') -or
      $normalized -match '^[A-Za-z]:' -or
      $normalized -match ':') {
    throw "FFmpeg source archive contains a rooted, UNC, drive, or alternate-stream path: $EntryName"
  }

  $trimmed = $normalized.TrimEnd('/')
  $segments = @($trimmed -split '/')
  if ($segments.Count -eq 0 -or
      $segments[0] -ne $ExpectedTopLevelDirectory -or
      @($segments | Where-Object { $_ -in @('', '.', '..') }).Count -ne 0) {
    throw "FFmpeg source archive path escapes or differs from the expected top-level directory: $EntryName"
  }

  return $true
}

function Test-FfmpegArchiveEntries {
  param(
    [Parameter(Mandatory = $true)]
    [string]$ArchivePath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedTopLevelDirectory,

    [string]$TarPath
  )

  if (-not $TarPath) {
    $systemTar = Join-Path $env:SystemRoot "System32\tar.exe"
    $TarPath = Resolve-FfmpegTool -Name "tar.exe" -PreferredPaths @($systemTar)
  }

  $nameResult = Invoke-StrictNativeCommand `
    -Executable $TarPath `
    -Arguments @('-tf', $ArchivePath) `
    -FailureMessage "Could not enumerate FFmpeg archive paths."
  $verboseResult = Invoke-StrictNativeCommand `
    -Executable $TarPath `
    -Arguments @('-tvf', $ArchivePath) `
    -FailureMessage "Could not enumerate FFmpeg archive entry types."

  $entryNames = @($nameResult.Output)
  $verboseEntries = @($verboseResult.Output)
  if ($entryNames.Count -eq 0 -or $entryNames.Count -ne $verboseEntries.Count) {
    throw "FFmpeg archive path/type enumeration was empty or inconsistent."
  }

  for ($index = 0; $index -lt $entryNames.Count; $index += 1) {
    $verboseLine = "$($verboseEntries[$index])"
    if (-not $verboseLine -or $verboseLine[0] -notin @('-', 'd', 'l', 'h', 'c', 'b', 'p', 's')) {
      throw "Could not determine FFmpeg archive entry type for: $($entryNames[$index])"
    }

    Test-FfmpegArchiveEntry `
      -EntryName "$($entryNames[$index])" `
      -EntryType "$($verboseLine[0])" `
      -ExpectedTopLevelDirectory $ExpectedTopLevelDirectory | Out-Null
  }

  return $true
}

function Save-FfmpegDownload {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Uri,

    [Parameter(Mandatory = $true)]
    [string]$DestinationPath,

    [string]$ExpectedSha256
  )

  if ($Uri -notmatch '^https://ffmpeg\.org/releases/ffmpeg-\d+\.\d+\.\d+\.tar\.xz(?:\.asc)?$') {
    throw "Refusing non-canonical FFmpeg download URL: $Uri"
  }

  $temporaryPath = "$DestinationPath.$([Guid]::NewGuid().ToString('N')).download"
  try {
    Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $temporaryPath
    if ($ExpectedSha256) {
      Assert-FfmpegFileSha256 `
        -Path $temporaryPath `
        -ExpectedSha256 $ExpectedSha256 `
        -Label "downloaded FFmpeg archive" | Out-Null
    }

    Move-Item -LiteralPath $temporaryPath -Destination $DestinationPath
  }
  finally {
    if (Test-Path -LiteralPath $temporaryPath -PathType Leaf) {
      Remove-Item -LiteralPath $temporaryPath -Force
    }
  }
}

function New-FfmpegVerificationDirectory {
  $root = Join-Path ([System.IO.Path]::GetTempPath()) (
    "stickerfit-ffmpeg-verify-" + [Guid]::NewGuid().ToString('N')
  )
  New-Item -ItemType Directory -Path $root | Out-Null
  [System.IO.File]::WriteAllText(
    (Join-Path $root '.stickerfit-ffmpeg-verify.marker'),
    'stickerfit-ffmpeg-verify-v1',
    [System.Text.UTF8Encoding]::new($false)
  )
  return $root
}

function Assert-NoReparsePointInFfmpegTree {
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
      throw "FFmpeg temporary tree contains a reparse-point directory: $currentDirectory"
    }

    foreach ($child in @(Get-ChildItem -LiteralPath $currentDirectory -Force)) {
      if (($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "FFmpeg temporary tree contains a reparse-point descendant: $($child.FullName)"
      }

      if (($child.Attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
        $pendingDirectories.Enqueue($child.FullName)
      }
    }
  }
}

function Remove-FfmpegVerificationDirectory {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  $fullPath = [System.IO.Path]::GetFullPath($Path).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $tempPrefix = $tempRoot + [System.IO.Path]::DirectorySeparatorChar
  $markerPath = Join-Path $fullPath '.stickerfit-ffmpeg-verify.marker'
  if (-not $fullPath.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not (Test-Path -LiteralPath $fullPath -PathType Container) -or
      -not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
    throw "Refusing to remove an unmarked FFmpeg verification directory: $Path"
  }

  $directoryItem = Get-Item -LiteralPath $fullPath -Force
  $markerItem = Get-Item -LiteralPath $markerPath -Force
  if (($directoryItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
      ($markerItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Refusing to remove a reparse-point FFmpeg verification directory or marker: $Path"
  }

  if ((Get-Content -Raw -LiteralPath $markerPath) -ne 'stickerfit-ffmpeg-verify-v1') {
    throw "Refusing to remove a differently owned FFmpeg verification directory: $Path"
  }

  Assert-NoReparsePointInFfmpegTree -RootPath $fullPath

  Remove-Item -LiteralPath $fullPath -Recurse -Force
}

function Invoke-FfmpegSourceVerification {
  param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceRoot,

    [string]$ManifestPath = "tools/ffmpeg/ffmpeg-version.json",

    [string]$DownloadCacheRoot = (Join-Path $env:TEMP "stickerfit-ffmpeg-cache"),

    [string]$VersionOverride,

    [string]$ExpectedSourceSha256,

    [string]$GpgPath,

    [string]$GpgvPath,

    [string]$TarPath
  )

  $manifest = Get-FfmpegManifest -WorkspaceRoot $WorkspaceRoot -ManifestPath $ManifestPath
  $version = "$($manifest.version)"
  $expectedSha256 = "$($manifest.release.archiveSha256)"
  if ($VersionOverride) {
    if ($VersionOverride -notmatch '^\d+\.\d+\.\d+$') {
      throw "Target FFmpeg version must be an exact three-part release: $VersionOverride"
    }

    if ($ExpectedSourceSha256 -and $ExpectedSourceSha256 -notmatch '^[0-9a-f]{64}$') {
      throw "ExpectedSourceSha256 must be 64 lowercase hexadecimal characters when provided."
    }

    if ($ExpectedSourceSha256 -and
        $VersionOverride -ne $version -and
        $ExpectedSourceSha256 -eq $expectedSha256) {
      throw "A new FFmpeg version cannot reuse the committed previous version's source SHA-256."
    }

    $version = $VersionOverride
    $expectedSha256 = $ExpectedSourceSha256
  }

  $archiveName = "ffmpeg-$version.tar.xz"
  $archiveUrl = "https://ffmpeg.org/releases/$archiveName"
  $signatureUrl = "$archiveUrl.asc"
  $expectedTopLevelDirectory = "ffmpeg-$version"

  if (-not (Test-Path -LiteralPath $DownloadCacheRoot -PathType Container)) {
    New-Item -ItemType Directory -Path $DownloadCacheRoot -Force | Out-Null
  }

  $archivePath = Join-Path $DownloadCacheRoot $archiveName
  $signaturePath = Join-Path $DownloadCacheRoot "$archiveName.asc"
  if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
    Save-FfmpegDownload `
      -Uri $archiveUrl `
      -DestinationPath $archivePath
  }

  if (-not (Test-Path -LiteralPath $signaturePath -PathType Leaf)) {
    Save-FfmpegDownload -Uri $signatureUrl -DestinationPath $signaturePath
  }

  $keyPath = Resolve-RepositoryPath `
    -WorkspaceRoot $WorkspaceRoot `
    -Path "$($manifest.release.signingKeyPath)"
  Assert-FfmpegFileSha256 `
    -Path $keyPath `
    -ExpectedSha256 "$($manifest.release.signingKeySha256)" `
    -Label "FFmpeg release signing key" | Out-Null

  if (-not $GpgPath) {
    $GpgPath = Resolve-FfmpegTool `
      -Name "gpg.exe" `
      -PreferredPaths @(
        "C:\Program Files\Git\usr\bin\gpg.exe",
        "C:\msys64\usr\bin\gpg.exe"
      )
  }

  if (-not $GpgvPath) {
    $GpgvPath = Resolve-FfmpegTool `
      -Name "gpgv.exe" `
      -PreferredPaths @(
        "C:\Program Files\Git\usr\bin\gpgv.exe",
        "C:\msys64\usr\bin\gpgv.exe"
      )
  }

  $verificationRoot = New-FfmpegVerificationDirectory
  try {
    $keyInventory = Invoke-StrictNativeCommand `
      -Executable $GpgPath `
      -Arguments @(
        '--no-options',
        '--homedir',
        '.',
        '--batch',
        '--with-colons',
        '--import-options',
        'show-only',
        '--import',
        $keyPath
      ) `
      -WorkingDirectory $verificationRoot `
      -FailureMessage "Could not inspect the isolated FFmpeg release signing key."
    Assert-FfmpegPrimaryKeyInventory `
      -ColonListing $keyInventory.Output `
      -ExpectedPrimaryFingerprint "$($manifest.releaseKeyFingerprint)"

    Invoke-StrictNativeCommand `
      -Executable $GpgPath `
      -Arguments @(
        '--no-options',
        '--homedir',
        '.',
        '--batch',
        '--yes',
        '--dearmor',
        '--output',
        './trustedkeys.gpg',
        $keyPath
      ) `
      -WorkingDirectory $verificationRoot `
      -FailureMessage "Could not create the isolated FFmpeg verification keyring." | Out-Null

    $signatureStatus = Invoke-StrictNativeCommand `
      -Executable $GpgvPath `
      -Arguments @(
        '--homedir',
        '.',
        '--status-fd',
        '1',
        '--keyring',
        './trustedkeys.gpg',
        $signaturePath,
        $archivePath
      ) `
      -WorkingDirectory $verificationRoot `
      -FailureMessage "FFmpeg source signature verification failed."
    Assert-FfmpegValidSignatureStatus `
      -StatusLines $signatureStatus.Output `
      -ExpectedPrimaryFingerprint "$($manifest.releaseKeyFingerprint)"
  }
  finally {
    Remove-FfmpegVerificationDirectory -Path $verificationRoot
  }

  # Hashing intentionally follows successful official signature validation.
  # Update mode may omit an expected hash; the computed value then becomes the
  # protected artifact authority recorded by the staging workflow.
  $computedSha256 = Get-FfmpegVerifiedArchiveSha256 `
    -ArchivePath $archivePath `
    -ExpectedSha256 $expectedSha256 `
    -Label "verified FFmpeg archive"

  if (-not $VersionOverride -and [long]$manifest.release.archiveBytes -gt 0) {
    Assert-FfmpegFileLength `
      -Path $archivePath `
      -ExpectedBytes ([long]$manifest.release.archiveBytes) `
      -Label "cached FFmpeg archive"
  }

  Test-FfmpegArchiveEntries `
    -ArchivePath $archivePath `
    -ExpectedTopLevelDirectory $expectedTopLevelDirectory `
    -TarPath $TarPath | Out-Null

  return [pscustomobject]@{
    Version = $version
    ArchivePath = $archivePath
    SignaturePath = $signaturePath
    ArchiveSha256 = $computedSha256
    SigningPrimaryFingerprint = "$($manifest.releaseKeyFingerprint)"
    ExpectedTopLevelDirectory = $expectedTopLevelDirectory
  }
}

function Assert-FfmpegJsonObjectSchema {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]]$RequiredProperties,

    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]]$AllowedProperties,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($null -eq $Value -or $Value -isnot [pscustomobject]) {
    throw "$Label must be a JSON object."
  }

  $actualProperties = @($Value.PSObject.Properties | ForEach-Object { $_.Name })
  foreach ($actualProperty in $actualProperties) {
    $isAllowed = $false
    foreach ($allowedProperty in $AllowedProperties) {
      if ([string]::Equals(
          $actualProperty,
          $allowedProperty,
          [System.StringComparison]::Ordinal
        )) {
        $isAllowed = $true
        break
      }
    }

    if (-not $isAllowed) {
      throw "$Label contains an unknown property: $actualProperty"
    }
  }

  foreach ($requiredProperty in $RequiredProperties) {
    $isPresent = $false
    foreach ($actualProperty in $actualProperties) {
      if ([string]::Equals(
          $actualProperty,
          $requiredProperty,
          [System.StringComparison]::Ordinal
        )) {
        $isPresent = $true
        break
      }
    }

    if (-not $isPresent) {
      throw "$Label is missing required property: $requiredProperty"
    }
  }
}

function Assert-FfmpegJsonString {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($Value -isnot [string]) {
    throw "$Label must be a JSON string."
  }
}

function Assert-FfmpegJsonBoolean {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($Value -isnot [bool]) {
    throw "$Label must be a JSON boolean."
  }
}

function Assert-FfmpegJsonInteger {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($Value -isnot [int] -and $Value -isnot [long]) {
    throw "$Label must be a JSON integer."
  }
}

function Assert-FfmpegJsonNull {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($null -ne $Value) {
    throw "$Label must be explicit JSON null."
  }
}

function Assert-FfmpegJsonStringArray {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  if ($Value -isnot [System.Array]) {
    throw "$Label must be a JSON array."
  }

  $index = 0
  foreach ($item in $Value) {
    Assert-FfmpegJsonString -Value $item -Label "$Label[$index]"
    $index += 1
  }
}

function Assert-FfmpegProvenanceToolMapSchema {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]]$Properties,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  Assert-FfmpegJsonObjectSchema `
    -Value $Value `
    -RequiredProperties $Properties `
    -AllowedProperties $Properties `
    -Label $Label

  foreach ($propertyName in $Properties) {
    Assert-FfmpegJsonString `
      -Value $Value.PSObject.Properties[$propertyName].Value `
      -Label "$Label.$propertyName"
  }
}

function Assert-FfmpegProvenanceFileRecordArraySchema {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Value,

    [Parameter(Mandatory = $true)]
    [string]$Label,

    [switch]$LegacyRuntimeDependency
  )

  if ($Value -isnot [System.Array]) {
    throw "$Label must be a JSON array."
  }

  $baseProperties = @('path', 'sha256', 'bytes')
  $recordProperties = if ($LegacyRuntimeDependency) {
    @(
      'path',
      'sha256',
      'bytes',
      'legacyOnly',
      'matchedInstalledMsys2Artifact',
      'installedArtifactSha256',
      'licensePath'
    )
  }
  else {
    $baseProperties
  }

  $index = 0
  foreach ($record in $Value) {
    $recordLabel = "$Label[$index]"
    Assert-FfmpegJsonObjectSchema `
      -Value $record `
      -RequiredProperties $recordProperties `
      -AllowedProperties $recordProperties `
      -Label $recordLabel
    Assert-FfmpegJsonString -Value $record.path -Label "$recordLabel.path"
    Assert-FfmpegJsonString -Value $record.sha256 -Label "$recordLabel.sha256"
    Assert-FfmpegJsonInteger -Value $record.bytes -Label "$recordLabel.bytes"

    if ($LegacyRuntimeDependency) {
      Assert-FfmpegJsonBoolean -Value $record.legacyOnly -Label "$recordLabel.legacyOnly"
      Assert-FfmpegJsonBoolean `
        -Value $record.matchedInstalledMsys2Artifact `
        -Label "$recordLabel.matchedInstalledMsys2Artifact"
      Assert-FfmpegJsonString `
        -Value $record.installedArtifactSha256 `
        -Label "$recordLabel.installedArtifactSha256"
      Assert-FfmpegJsonString -Value $record.licensePath -Label "$recordLabel.licensePath"
    }

    $index += 1
  }
}

function Assert-FfmpegProvenanceSchema {
  param(
    [Parameter(Mandatory = $true)]
    [AllowNull()]
    [object]$Provenance
  )

  $legacyTopLevelProperties = @(
    'schemaVersion',
    'legacyBootstrap',
    'sourceUrl',
    'version',
    'sourceArchiveSha256',
    'signingPrimaryFingerprint',
    'configureArgs',
    'toolchainVersions',
    'sourceDateEpoch',
    'vendorUpdateRunId',
    'exeSha256',
    'targetTriple',
    'artifact',
    'source',
    'build',
    'runtimeDependencies',
    'companionFiles'
  )
  $nonLegacyTopLevelProperties = @($legacyTopLevelProperties) + @('systemDependencies')

  Assert-FfmpegJsonObjectSchema `
    -Value $Provenance `
    -RequiredProperties @('schemaVersion', 'legacyBootstrap') `
    -AllowedProperties $nonLegacyTopLevelProperties `
    -Label 'FFmpeg provenance'
  Assert-FfmpegJsonInteger `
    -Value $Provenance.schemaVersion `
    -Label 'FFmpeg provenance.schemaVersion'
  if ($Provenance.schemaVersion -ne 1) {
    throw "Unsupported FFmpeg provenance schemaVersion: $($Provenance.schemaVersion)"
  }

  Assert-FfmpegJsonBoolean `
    -Value $Provenance.legacyBootstrap `
    -Label 'FFmpeg provenance.legacyBootstrap'
  $legacyBootstrap = $Provenance.legacyBootstrap
  $topLevelProperties = if ($legacyBootstrap) {
    $legacyTopLevelProperties
  }
  else {
    $nonLegacyTopLevelProperties
  }
  Assert-FfmpegJsonObjectSchema `
    -Value $Provenance `
    -RequiredProperties $topLevelProperties `
    -AllowedProperties $topLevelProperties `
    -Label 'FFmpeg provenance'

  foreach ($propertyName in @(
      'sourceUrl',
      'version',
      'sourceArchiveSha256',
      'signingPrimaryFingerprint',
      'exeSha256',
      'targetTriple'
    )) {
    Assert-FfmpegJsonString `
      -Value $Provenance.PSObject.Properties[$propertyName].Value `
      -Label "FFmpeg provenance.$propertyName"
  }
  Assert-FfmpegJsonStringArray `
    -Value $Provenance.configureArgs `
    -Label 'FFmpeg provenance.configureArgs'

  $artifactProperties = @('path', 'sha256', 'bytes', 'origin')
  Assert-FfmpegJsonObjectSchema `
    -Value $Provenance.artifact `
    -RequiredProperties $artifactProperties `
    -AllowedProperties $artifactProperties `
    -Label 'FFmpeg provenance.artifact'
  Assert-FfmpegJsonString `
    -Value $Provenance.artifact.path `
    -Label 'FFmpeg provenance.artifact.path'
  Assert-FfmpegJsonString `
    -Value $Provenance.artifact.sha256 `
    -Label 'FFmpeg provenance.artifact.sha256'
  Assert-FfmpegJsonInteger `
    -Value $Provenance.artifact.bytes `
    -Label 'FFmpeg provenance.artifact.bytes'

  $sourceProperties = @(
    'archiveUrl',
    'signatureUrl',
    'archiveSha256',
    'signingPrimaryFingerprint',
    'archiveSignatureVerifiedSeparately',
    'artifactBuiltFromVerifiedSource'
  )
  Assert-FfmpegJsonObjectSchema `
    -Value $Provenance.source `
    -RequiredProperties $sourceProperties `
    -AllowedProperties $sourceProperties `
    -Label 'FFmpeg provenance.source'
  foreach ($propertyName in @(
      'archiveUrl',
      'signatureUrl',
      'archiveSha256',
      'signingPrimaryFingerprint'
    )) {
    Assert-FfmpegJsonString `
      -Value $Provenance.source.PSObject.Properties[$propertyName].Value `
      -Label "FFmpeg provenance.source.$propertyName"
  }
  Assert-FfmpegJsonBoolean `
    -Value $Provenance.source.archiveSignatureVerifiedSeparately `
    -Label 'FFmpeg provenance.source.archiveSignatureVerifiedSeparately'
  Assert-FfmpegJsonBoolean `
    -Value $Provenance.source.artifactBuiltFromVerifiedSource `
    -Label 'FFmpeg provenance.source.artifactBuiltFromVerifiedSource'

  $legacyBuildProperties = @(
    'approvedWorkflow',
    'runId',
    'sourceDateEpoch',
    'toolchain',
    'deterministicFlags',
    'configurationObservation',
    'configureArguments'
  )
  $nonLegacyBuildProperties = @(
    'approvedWorkflow',
    'runId',
    'sourceCommit',
    'authority',
    'sourceDateEpoch',
    'toolchain',
    'toolPaths',
    'deterministicFlags',
    'configurationObservation',
    'configureArguments'
  )
  $buildProperties = if ($legacyBootstrap) {
    $legacyBuildProperties
  }
  else {
    $nonLegacyBuildProperties
  }
  Assert-FfmpegJsonObjectSchema `
    -Value $Provenance.build `
    -RequiredProperties $buildProperties `
    -AllowedProperties $buildProperties `
    -Label 'FFmpeg provenance.build'
  Assert-FfmpegJsonBoolean `
    -Value $Provenance.build.approvedWorkflow `
    -Label 'FFmpeg provenance.build.approvedWorkflow'
  Assert-FfmpegJsonString `
    -Value $Provenance.build.configurationObservation `
    -Label 'FFmpeg provenance.build.configurationObservation'
  Assert-FfmpegJsonStringArray `
    -Value $Provenance.build.configureArguments `
    -Label 'FFmpeg provenance.build.configureArguments'

  Assert-FfmpegProvenanceFileRecordArraySchema `
    -Value $Provenance.companionFiles `
    -Label 'FFmpeg provenance.companionFiles'

  if ($legacyBootstrap) {
    Assert-FfmpegJsonNull `
      -Value $Provenance.toolchainVersions `
      -Label 'FFmpeg provenance.toolchainVersions'
    Assert-FfmpegJsonNull `
      -Value $Provenance.sourceDateEpoch `
      -Label 'FFmpeg provenance.sourceDateEpoch'
    Assert-FfmpegJsonNull `
      -Value $Provenance.vendorUpdateRunId `
      -Label 'FFmpeg provenance.vendorUpdateRunId'
    Assert-FfmpegJsonNull `
      -Value $Provenance.artifact.origin `
      -Label 'FFmpeg provenance.artifact.origin'
    Assert-FfmpegJsonNull `
      -Value $Provenance.build.runId `
      -Label 'FFmpeg provenance.build.runId'
    Assert-FfmpegJsonNull `
      -Value $Provenance.build.sourceDateEpoch `
      -Label 'FFmpeg provenance.build.sourceDateEpoch'
    Assert-FfmpegJsonNull `
      -Value $Provenance.build.toolchain `
      -Label 'FFmpeg provenance.build.toolchain'
    Assert-FfmpegJsonNull `
      -Value $Provenance.build.deterministicFlags `
      -Label 'FFmpeg provenance.build.deterministicFlags'
    Assert-FfmpegProvenanceFileRecordArraySchema `
      -Value $Provenance.runtimeDependencies `
      -Label 'FFmpeg provenance.runtimeDependencies' `
      -LegacyRuntimeDependency
    return
  }

  Assert-FfmpegJsonInteger `
    -Value $Provenance.sourceDateEpoch `
    -Label 'FFmpeg provenance.sourceDateEpoch'
  Assert-FfmpegJsonInteger `
    -Value $Provenance.vendorUpdateRunId `
    -Label 'FFmpeg provenance.vendorUpdateRunId'
  Assert-FfmpegJsonString `
    -Value $Provenance.artifact.origin `
    -Label 'FFmpeg provenance.artifact.origin'

  $toolchainProperties = @(
    'pacman',
    'msys2Runtime',
    'bash',
    'gcc',
    'binutils',
    'make',
    'nasm',
    'pkgconf',
    'strip',
    'tar',
    'gpg',
    'gpgv',
    'vswhere',
    'dumpbin'
  )
  Assert-FfmpegProvenanceToolMapSchema `
    -Value $Provenance.toolchainVersions `
    -Properties $toolchainProperties `
    -Label 'FFmpeg provenance.toolchainVersions'
  Assert-FfmpegJsonInteger `
    -Value $Provenance.build.runId `
    -Label 'FFmpeg provenance.build.runId'
  Assert-FfmpegJsonString `
    -Value $Provenance.build.sourceCommit `
    -Label 'FFmpeg provenance.build.sourceCommit'
  Assert-FfmpegJsonInteger `
    -Value $Provenance.build.sourceDateEpoch `
    -Label 'FFmpeg provenance.build.sourceDateEpoch'
  Assert-FfmpegProvenanceToolMapSchema `
    -Value $Provenance.build.toolchain `
    -Properties $toolchainProperties `
    -Label 'FFmpeg provenance.build.toolchain'

  $toolPathProperties = @(
    'bash',
    'gpg',
    'gpgv',
    'gcc',
    'binutils',
    'make',
    'nasm',
    'pkgconf',
    'strip',
    'tar',
    'pacman',
    'vswhere',
    'dumpbin'
  )
  Assert-FfmpegProvenanceToolMapSchema `
    -Value $Provenance.build.toolPaths `
    -Properties $toolPathProperties `
    -Label 'FFmpeg provenance.build.toolPaths'
  Assert-FfmpegJsonStringArray `
    -Value $Provenance.build.deterministicFlags `
    -Label 'FFmpeg provenance.build.deterministicFlags'

  $authorityProperties = @(
    'runId',
    'repository',
    'workflowPath',
    'event',
    'headBranch',
    'headSha'
  )
  Assert-FfmpegJsonObjectSchema `
    -Value $Provenance.build.authority `
    -RequiredProperties $authorityProperties `
    -AllowedProperties $authorityProperties `
    -Label 'FFmpeg provenance.build.authority'
  Assert-FfmpegJsonInteger `
    -Value $Provenance.build.authority.runId `
    -Label 'FFmpeg provenance.build.authority.runId'
  foreach ($propertyName in @('repository', 'workflowPath', 'event', 'headBranch', 'headSha')) {
    Assert-FfmpegJsonString `
      -Value $Provenance.build.authority.PSObject.Properties[$propertyName].Value `
      -Label "FFmpeg provenance.build.authority.$propertyName"
  }

  Assert-FfmpegProvenanceFileRecordArraySchema `
    -Value $Provenance.runtimeDependencies `
    -Label 'FFmpeg provenance.runtimeDependencies'
  Assert-FfmpegJsonStringArray `
    -Value $Provenance.systemDependencies `
    -Label 'FFmpeg provenance.systemDependencies'
}

function Assert-ProvenanceFileRecords {
  param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceRoot,

    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [object[]]$Records,

    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]]$ExpectedNames,

    [Parameter(Mandatory = $true)]
    [string]$Label
  )

  $actualNames = @($Records | ForEach-Object {
      [System.IO.Path]::GetFileName("$($_.path)")
    })
  Assert-ExactSequence -Actual $actualNames -Expected $ExpectedNames -Label $Label

  for ($index = 0; $index -lt $Records.Count; $index += 1) {
    $record = $Records[$index]
    $expectedRelativePath = "src-tauri/binaries/$($ExpectedNames[$index])"
    if ("$($record.path)" -ne $expectedRelativePath) {
      throw "$Label path must be the protected vendor path: $expectedRelativePath"
    }

    $resolvedPath = Resolve-RepositoryPath -WorkspaceRoot $WorkspaceRoot -Path "$($record.path)"
    Assert-FfmpegFileSha256 `
      -Path $resolvedPath `
      -ExpectedSha256 "$($record.sha256)" `
      -Label "$Label '$($ExpectedNames[$index])'" | Out-Null
    Assert-FfmpegFileLength `
      -Path $resolvedPath `
      -ExpectedBytes ([long]$record.bytes) `
      -Label "$Label '$($ExpectedNames[$index])'"
  }
}

function Test-FfmpegVendorArtifacts {
  param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceRoot,

    [string]$ManifestPath = "tools/ffmpeg/ffmpeg-version.json",

    [switch]$AllowLegacyBootstrap
  )

  $manifest = Get-FfmpegManifest -WorkspaceRoot $WorkspaceRoot -ManifestPath $ManifestPath
  $provenancePath = Resolve-RepositoryPath `
    -WorkspaceRoot $WorkspaceRoot `
    -Path "$($manifest.vendor.provenancePath)"
  $provenance = Read-FfmpegJson -Path $provenancePath
  Assert-FfmpegProvenanceSchema -Provenance $provenance

  if ([int]$provenance.schemaVersion -ne 1) {
    throw "Unsupported FFmpeg provenance schemaVersion: $($provenance.schemaVersion)"
  }

  if ("$($provenance.version)" -ne "$($manifest.version)" -or
      "$($provenance.targetTriple)" -ne "$($manifest.targetTriple)") {
    throw "FFmpeg manifest and provenance version/target metadata disagree."
  }

  if ("$($provenance.sourceUrl)" -ne "$($provenance.source.archiveUrl)" -or
      "$($provenance.sourceArchiveSha256)" -ne "$($provenance.source.archiveSha256)" -or
      "$($provenance.signingPrimaryFingerprint)" -ne "$($provenance.source.signingPrimaryFingerprint)" -or
      "$($provenance.exeSha256)" -ne "$($provenance.artifact.sha256)" -or
      "$($provenance.sourceDateEpoch)" -ne "$($provenance.build.sourceDateEpoch)" -or
      "$($provenance.vendorUpdateRunId)" -ne "$($provenance.build.runId)") {
    throw "Canonical top-level FFmpeg provenance fields disagree with nested detail fields."
  }

  $topToolchainJson = $provenance.toolchainVersions | ConvertTo-Json -Depth 10 -Compress
  $nestedToolchainJson = $provenance.build.toolchain | ConvertTo-Json -Depth 10 -Compress
  if ($topToolchainJson -ne $nestedToolchainJson) {
    throw "Canonical top-level FFmpeg toolchainVersions disagree with build.toolchain."
  }

  Assert-ExactSequence `
    -Actual @($provenance.configureArgs) `
    -Expected @($provenance.build.configureArguments) `
    -Label "canonical FFmpeg configureArgs"

  if ($provenance.legacyBootstrap -isnot [bool]) {
    throw "FFmpeg provenance legacyBootstrap must be a JSON boolean."
  }

  $booleanContracts = @(
    [pscustomobject]@{
      Label = 'source.archiveSignatureVerifiedSeparately'
      Value = $provenance.source.archiveSignatureVerifiedSeparately
    },
    [pscustomobject]@{
      Label = 'source.artifactBuiltFromVerifiedSource'
      Value = $provenance.source.artifactBuiltFromVerifiedSource
    },
    [pscustomobject]@{
      Label = 'build.approvedWorkflow'
      Value = $provenance.build.approvedWorkflow
    }
  )
  foreach ($booleanContract in $booleanContracts) {
    if ($booleanContract.Value -isnot [bool]) {
      throw "FFmpeg provenance $($booleanContract.Label) must be a JSON boolean."
    }
  }

  $legacyBootstrap = [bool]$provenance.legacyBootstrap
  if ($legacyBootstrap -and -not $AllowLegacyBootstrap) {
    throw "Release verification rejects legacy FFmpeg bootstrap provenance. Complete the protected vendor update first."
  }

  if ($legacyBootstrap -and -not [bool]$manifest.vendor.legacyBootstrapAllowedForRoutineVerification) {
    throw "The manifest does not permit routine verification of legacy FFmpeg provenance."
  }

  if (-not $legacyBootstrap -and [bool]$manifest.vendor.legacyBootstrapAllowedForRoutineVerification) {
    throw "Non-legacy FFmpeg provenance requires the manifest legacy-bootstrap policy to be false."
  }

  $keyPath = Resolve-RepositoryPath `
    -WorkspaceRoot $WorkspaceRoot `
    -Path "$($manifest.release.signingKeyPath)"
  Assert-FfmpegFileSha256 `
    -Path $keyPath `
    -ExpectedSha256 "$($manifest.release.signingKeySha256)" `
    -Label "FFmpeg release signing key" | Out-Null

  $artifactPath = Resolve-RepositoryPath `
    -WorkspaceRoot $WorkspaceRoot `
    -Path "$($provenance.artifact.path)"
  if ("$($provenance.artifact.path)" -ne "$($manifest.vendor.executablePath)") {
    throw "FFmpeg provenance points at an unexpected executable path."
  }

  Assert-FfmpegFileSha256 `
    -Path $artifactPath `
    -ExpectedSha256 "$($provenance.artifact.sha256)" `
    -Label "tracked FFmpeg executable" | Out-Null
  Assert-FfmpegFileLength `
    -Path $artifactPath `
    -ExpectedBytes ([long]$provenance.artifact.bytes) `
    -Label "tracked FFmpeg executable"

  if ("$($manifest.expectedExeSha256)" -ne "$($provenance.exeSha256)") {
    throw "Manifest expectedExeSha256 does not match FFmpeg provenance."
  }

  if ("$($provenance.source.archiveUrl)" -ne "$($manifest.release.archiveUrl)" -or
      "$($provenance.source.signatureUrl)" -ne "$($manifest.release.signatureUrl)" -or
      "$($provenance.source.archiveSha256)" -ne "$($manifest.release.archiveSha256)" -or
      "$($provenance.source.signingPrimaryFingerprint)" -ne "$($manifest.release.primaryFingerprint)" -or
      -not [bool]$provenance.source.archiveSignatureVerifiedSeparately) {
    throw "FFmpeg provenance source metadata does not match the verified manifest."
  }

  $expectedConfigureArguments = @(Get-FfmpegConfigureArguments `
      -Manifest $manifest `
      -LegacyBootstrap:$legacyBootstrap)
  Assert-ExactSequence `
    -Actual @($provenance.build.configureArguments) `
    -Expected $expectedConfigureArguments `
    -Label "FFmpeg configure arguments"

  $embeddedText = [System.Text.Encoding]::ASCII.GetString(
    [System.IO.File]::ReadAllBytes($artifactPath)
  )
  $configureLine = $expectedConfigureArguments -join ' '
  if (-not $embeddedText.Contains($configureLine)) {
    throw "Tracked FFmpeg executable does not contain the provenance configure string."
  }

  $expectedRuntimeDependencies = @(
    $manifest.vendor.expectedRuntimeDependencies | ForEach-Object { "$_" }
  )
  $expectedCompanionFiles = @(
    $manifest.vendor.trackedCompanionFiles | ForEach-Object { "$_" }
  )
  Assert-ProvenanceFileRecords `
    -WorkspaceRoot $WorkspaceRoot `
    -Records @($provenance.runtimeDependencies) `
    -ExpectedNames $expectedRuntimeDependencies `
    -Label "FFmpeg runtime dependency"
  Assert-ProvenanceFileRecords `
    -WorkspaceRoot $WorkspaceRoot `
    -Records @($provenance.companionFiles) `
    -ExpectedNames $expectedCompanionFiles `
    -Label "FFmpeg companion file"

  if ($legacyBootstrap) {
    $legacyRuntimeDependencies = @($provenance.runtimeDependencies)
    if ($legacyRuntimeDependencies.Count -eq 1 -and
        ($legacyRuntimeDependencies[0].legacyOnly -isnot [bool] -or
          $legacyRuntimeDependencies[0].matchedInstalledMsys2Artifact -isnot [bool])) {
      throw "Legacy FFmpeg runtime dependency flags must be JSON booleans."
    }

    if ($legacyRuntimeDependencies.Count -ne 1 -or
        "$($legacyRuntimeDependencies[0].path)" -ne 'src-tauri/binaries/libwinpthread-1.dll' -or
        -not [bool]$legacyRuntimeDependencies[0].legacyOnly -or
        -not [bool]$legacyRuntimeDependencies[0].matchedInstalledMsys2Artifact -or
        "$($legacyRuntimeDependencies[0].installedArtifactSha256)" -ne "$($legacyRuntimeDependencies[0].sha256)" -or
        "$($legacyRuntimeDependencies[0].licensePath)" -ne 'src-tauri/binaries/LICENSE-libwinpthread.txt') {
      throw "Legacy FFmpeg runtime dependency metadata must pin the inspected libwinpthread artifact and license."
    }

    if ([bool]$provenance.source.artifactBuiltFromVerifiedSource -or
        [bool]$provenance.build.approvedWorkflow -or
        $null -ne $provenance.build.runId -or
        $null -ne $provenance.build.sourceDateEpoch -or
        $null -ne $provenance.build.toolchain -or
        $null -ne $provenance.build.deterministicFlags) {
      throw "Legacy FFmpeg provenance must retain explicit unknown/null build facts and no source-to-artifact claim."
    }
  }
  else {
    if ("$($provenance.version)" -eq '8.1.2' -and
        ([long]$provenance.sourceDateEpoch -ne 1781654400 -or
          [long]$provenance.build.sourceDateEpoch -ne 1781654400)) {
      throw "FFmpeg 8.1.2 provenance must use SOURCE_DATE_EPOCH 1781654400."
    }

    $requiredToolchainProperties = @(
      'pacman',
      'msys2Runtime',
      'bash',
      'gcc',
      'binutils',
      'make',
      'nasm',
      'pkgconf',
      'strip',
      'tar',
      'gpg',
      'gpgv',
      'vswhere',
      'dumpbin'
    )
    foreach ($propertyName in $requiredToolchainProperties) {
      $property = $provenance.toolchainVersions.PSObject.Properties[$propertyName]
      if ($null -eq $property -or -not "$($property.Value)") {
        throw "Release FFmpeg provenance is missing toolchain version: $propertyName"
      }
    }

    foreach ($pathPropertyName in @(
        'bash',
        'gpg',
        'gpgv',
        'gcc',
        'binutils',
        'make',
        'nasm',
        'pkgconf',
        'strip',
        'tar',
        'pacman',
        'vswhere',
        'dumpbin'
      )) {
      $pathProperty = $provenance.build.toolPaths.PSObject.Properties[$pathPropertyName]
      if ($null -eq $pathProperty -or -not "$($pathProperty.Value)") {
        throw "Release FFmpeg provenance is missing resolved tool path: $pathPropertyName"
      }
    }

    $authority = $provenance.build.authority
    if (-not [string]::Equals(
        "$($authority.runId)",
        "$($provenance.vendorUpdateRunId)",
        [System.StringComparison]::Ordinal
      ) -or -not [string]::Equals(
        "$($authority.repository)",
        'Ayumudayo/StickerFit',
        [System.StringComparison]::Ordinal
      ) -or -not [string]::Equals(
        "$($authority.workflowPath)",
        '.github/workflows/update-ffmpeg-vendor.yml',
        [System.StringComparison]::Ordinal
      ) -or -not [string]::Equals(
        "$($authority.event)",
        'workflow_dispatch',
        [System.StringComparison]::Ordinal
      ) -or -not [string]::Equals(
        "$($authority.headBranch)",
        'main',
        [System.StringComparison]::Ordinal
      ) -or "$($authority.headSha)" -cnotmatch '^[0-9a-f]{40}$' -or
      -not [string]::Equals(
        "$($provenance.build.sourceCommit)",
        "$($authority.headSha)",
        [System.StringComparison]::Ordinal
      )) {
      throw "Release FFmpeg provenance is missing exact protected workflow authority."
    }

    if (-not [bool]$provenance.source.artifactBuiltFromVerifiedSource -or
        -not [bool]$provenance.build.approvedWorkflow -or
      "$($provenance.build.runId)" -notmatch '^\d+$' -or
        "$($manifest.vendorUpdateRunId)" -ne "$($provenance.vendorUpdateRunId)" -or
        [long]$provenance.build.sourceDateEpoch -le 0 -or
        $null -eq $provenance.build.toolchain -or
        @($provenance.build.deterministicFlags).Count -eq 0) {
      throw "Release FFmpeg provenance is missing protected build and deterministic toolchain facts."
    }
  }

  return [pscustomobject]@{
    Version = "$($manifest.version)"
    TargetTriple = "$($manifest.targetTriple)"
    LegacyBootstrap = $legacyBootstrap
    ArtifactPath = $artifactPath
    ArtifactSha256 = "$($provenance.artifact.sha256)"
    ArtifactBytes = [long]$provenance.artifact.bytes
    Manifest = $manifest
    Provenance = $provenance
  }
}

Export-ModuleMember -Function @(
  'Assert-FfmpegFileSha256',
  'Assert-FfmpegPrimaryKeyInventory',
  'Assert-FfmpegValidSignatureStatus',
  'Get-FfmpegConfigureArguments',
  'Get-FfmpegManifest',
  'Get-FfmpegVerifiedArchiveSha256',
  'Invoke-FfmpegSourceVerification',
  'Invoke-StrictNativeCommand',
  'Resolve-FfmpegTool',
  'Resolve-RepositoryPath',
  'Test-FfmpegArchiveEntries',
  'Test-FfmpegArchiveEntry',
  'Test-FfmpegVendorArtifacts',
  'Write-FfmpegJson'
)
