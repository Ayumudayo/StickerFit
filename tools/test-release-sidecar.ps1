[CmdletBinding()]
param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
  [string]$PolicyRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
  [string]$ReleaseDirectory,
  [string]$InstallerPath,
  [string]$SevenZipPath,
  [switch]$StaticOnly,
  [switch]$RequireNonLegacyProvenance
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-NormalizedFullPath {
  param([Parameter(Mandatory = $true)][string]$Path)

  $fullPath = [System.IO.Path]::GetFullPath($Path)
  $root = [System.IO.Path]::GetPathRoot($fullPath)
  if ([string]::Equals($fullPath, $root, [System.StringComparison]::OrdinalIgnoreCase)) {
    return $root
  }

  return $fullPath.TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
}

function Assert-NoReparsePointInPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Boundary,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $resolvedPath = Get-NormalizedFullPath -Path $Path
  $resolvedBoundary = Get-NormalizedFullPath -Path $Boundary
  $boundaryPrefix = if ([string]::Equals(
      $resolvedBoundary,
      [System.IO.Path]::GetPathRoot($resolvedBoundary),
      [System.StringComparison]::OrdinalIgnoreCase
    )) {
    $resolvedBoundary
  }
  else {
    $resolvedBoundary + [System.IO.Path]::DirectorySeparatorChar
  }
  if (-not [string]::Equals($resolvedPath, $resolvedBoundary, [System.StringComparison]::OrdinalIgnoreCase) -and
      -not $resolvedPath.StartsWith($boundaryPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "$Label is outside its verified boundary: $resolvedPath"
  }

  $cursor = $resolvedPath
  while ($true) {
    if (-not (Test-Path -LiteralPath $cursor)) {
      throw "$Label contains a missing path segment: $cursor"
    }
    $item = Get-Item -LiteralPath $cursor -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "$Label contains a reparse-point path segment: $cursor"
    }
    if ([string]::Equals($cursor, $resolvedBoundary, [System.StringComparison]::OrdinalIgnoreCase)) {
      break
    }
    $parent = [System.IO.Directory]::GetParent($cursor)
    if ($null -eq $parent) {
      throw "$Label did not reach its verified boundary."
    }
    $cursor = Get-NormalizedFullPath -Path $parent.FullName
  }

  return $resolvedPath
}

function Get-CanonicalSevenZipPath {
  $programFiles = [Environment]::GetFolderPath(
    [Environment+SpecialFolder]::ProgramFiles
  )
  if ([string]::IsNullOrWhiteSpace($programFiles)) {
    throw "Could not resolve the canonical 64-bit Program Files directory."
  }

  return [System.IO.Path]::GetFullPath(
    (Join-Path $programFiles '7-Zip\7z.exe')
  )
}

function Get-ExactJsonProperty {
  param(
    [Parameter(Mandatory = $true)][object]$Object,
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $properties = @($Object.PSObject.Properties | Where-Object {
      $_.MemberType -eq [System.Management.Automation.PSMemberTypes]::NoteProperty -and
      [string]::Equals([string]$_.Name, $Name, [System.StringComparison]::Ordinal)
    })
  if ($properties.Count -ne 1) {
    throw "$Label must contain exactly one case-sensitive '$Name' property."
  }
  Write-Output -NoEnumerate $properties[0].Value
}

function Write-NewUtf8File {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Content
  )

  $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($Content)
  $stream = [System.IO.FileStream]::new(
    $Path,
    [System.IO.FileMode]::CreateNew,
    [System.IO.FileAccess]::Write,
    [System.IO.FileShare]::None,
    4096,
    [System.IO.FileOptions]::WriteThrough
  )
  try {
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  }
  finally {
    $stream.Dispose()
  }
}

function Assert-ExactFileBytes {
  param(
    [Parameter(Mandatory = $true)]
    [string]$ActualPath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedPath,

    [Parameter(Mandatory = $true)]
    [string]$Label,

    [Parameter(Mandatory = $true)]
    [string]$ActualBoundary,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedBoundary
  )

  $ActualPath = Assert-NoReparsePointInPath -Path $ActualPath -Boundary $ActualBoundary -Label "$Label packaged path"
  $ExpectedPath = Assert-NoReparsePointInPath -Path $ExpectedPath -Boundary $ExpectedBoundary -Label "$Label source path"
  if (-not (Test-Path -LiteralPath $ActualPath -PathType Leaf) -or
      -not (Test-Path -LiteralPath $ExpectedPath -PathType Leaf)) {
    throw "$Label source or packaged file is missing."
  }
  $actualItem = Get-Item -LiteralPath $ActualPath -Force
  $expectedItem = Get-Item -LiteralPath $ExpectedPath -Force
  $expectedActualName = [System.IO.Path]::GetFileName($ActualPath)
  if (-not [string]::Equals($actualItem.Name, $expectedActualName, [System.StringComparison]::Ordinal)) {
    throw "$Label has a case-variant packaged filename: $($actualItem.Name)"
  }

  if ([long]$actualItem.Length -ne [long]$expectedItem.Length) {
    throw "$Label byte length differs from the protected repository source."
  }

  $expectedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ExpectedPath).Hash
  $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ActualPath).Hash
  if (-not [string]::Equals($actualHash, $expectedHash, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "$Label bytes differ from the protected repository source."
  }
}

function Get-ReleaseFilesWithoutReparseTraversal {
  param([Parameter(Mandatory = $true)][string]$RootPath)

  $rootItem = Get-Item -LiteralPath $RootPath -Force
  if (($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Release root must not be a reparse point: $RootPath"
  }

  $files = New-Object 'System.Collections.Generic.List[System.IO.FileInfo]'
  $pending = New-Object 'System.Collections.Generic.Queue[string]'
  $pending.Enqueue([System.IO.Path]::GetFullPath($RootPath))
  while ($pending.Count -gt 0) {
    $directory = $pending.Dequeue()
    foreach ($child in @(Get-ChildItem -LiteralPath $directory -Force)) {
      if (($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Release payload contains an unsupported reparse point: $($child.FullName)"
      }
      if (($child.Attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
        $pending.Enqueue($child.FullName)
      }
      else {
        $files.Add($child)
      }
    }
  }

  return @($files)
}

function Get-ExactPayloadFile {
  param(
    [Parameter(Mandatory = $true)][object[]]$Files,
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $matches = @($Files | Where-Object {
      [string]::Equals($_.Name, $Name, [System.StringComparison]::OrdinalIgnoreCase)
    })
  if ($matches.Count -ne 1) {
    throw "$Label must contain exactly one '$Name' file; found $($matches.Count)."
  }
  if (-not [string]::Equals($matches[0].Name, $Name, [System.StringComparison]::Ordinal)) {
    throw "$Label contains a case-variant filename '$($matches[0].Name)' instead of '$Name'."
  }
  return $matches[0]
}

function Assert-NoLegacyPayloadFiles {
  param(
    [Parameter(Mandatory = $true)][object[]]$Files,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $legacyNames = @(
    'libwinpthread-1.dll',
    'LICENSE-ffmpeg-BtbN.txt',
    'LICENSE-libwinpthread.txt'
  )
  $legacyFiles = @($Files | Where-Object {
      $name = $_.Name
      @($legacyNames | Where-Object {
          [string]::Equals($name, $_, [System.StringComparison]::OrdinalIgnoreCase)
        }).Count -ne 0
    })
  if ($legacyFiles.Count -ne 0) {
    throw "$Label contains legacy-only resources: $($legacyFiles.FullName -join ', ')"
  }
}

function Assert-ExtractedInstallerPayload {
  param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$SevenZip,
    [Parameter(Mandatory = $true)][string]$ReleaseRoot,
    [Parameter(Mandatory = $true)][string]$Workspace,
    [Parameter(Mandatory = $true)]$Vendor,
    [Parameter(Mandatory = $true)][string]$TrackedLicense,
    [Parameter(Mandatory = $true)][string]$TrackedProvenance
  )

  $nsisRoot = Join-Path $ReleaseRoot 'bundle\nsis'
  $nsisRoot = Assert-NoReparsePointInPath -Path $nsisRoot -Boundary $ReleaseRoot -Label 'NSIS bundle directory'
  $looseDesktop = Assert-NoReparsePointInPath `
    -Path (Join-Path $ReleaseRoot 'desktop.exe') `
    -Boundary $ReleaseRoot `
    -Label 'loose desktop executable'
  if (-not (Test-Path -LiteralPath $looseDesktop -PathType Leaf)) {
    throw "Loose desktop executable is missing: $looseDesktop"
  }
  $Installer = Assert-NoReparsePointInPath -Path $Installer -Boundary $nsisRoot -Label 'NSIS installer'
  if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
    throw "NSIS installer is missing: $Installer"
  }
  $installerItem = Get-Item -LiteralPath $Installer -Force
  if ([long]$installerItem.Length -le 0 -or [long]$installerItem.Length -gt 1GB) {
    throw "NSIS installer has an invalid physical size: $($installerItem.Length) bytes."
  }
  if (-not [string]::Equals(
      $installerItem.Name,
      [System.IO.Path]::GetFileName($Installer),
      [System.StringComparison]::Ordinal
    )) {
    throw "NSIS installer path uses case-variant filename casing."
  }
  $nsisChildren = @(Get-ChildItem -LiteralPath $nsisRoot -Force)
  if ($nsisChildren.Count -ne 1 -or
      $nsisChildren[0].PSIsContainer -or
      ($nsisChildren[0].Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
      -not [string]::Equals($nsisChildren[0].FullName, $Installer, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not [string]::Equals($nsisChildren[0].Extension, '.exe', [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "NSIS bundle directory must contain exactly the selected regular installer executable."
  }

  $sevenZipFullPath = [System.IO.Path]::GetFullPath($SevenZip)
  $canonicalSevenZipPath = Get-CanonicalSevenZipPath
  if (-not [string]::Equals(
      $sevenZipFullPath,
      $canonicalSevenZipPath,
      [System.StringComparison]::OrdinalIgnoreCase
    )) {
    throw "NSIS inspection requires canonical 7-Zip: $canonicalSevenZipPath"
  }
  $sevenZipDriveRoot = [System.IO.Path]::GetPathRoot($sevenZipFullPath)
  $sevenZipFullPath = Assert-NoReparsePointInPath `
    -Path $sevenZipFullPath `
    -Boundary $sevenZipDriveRoot `
    -Label '7-Zip extractor'
  $sevenZipItem = Get-Item -LiteralPath $sevenZipFullPath -Force
  if (-not [string]::Equals($sevenZipItem.Name, '7z.exe', [System.StringComparison]::Ordinal)) {
    throw "NSIS inspection requires the canonical 7z.exe executable."
  }
  $sevenZipVersionResult = Invoke-StrictNativeCommand `
    -Executable $sevenZipFullPath `
    -Arguments @('i') `
    -FailureMessage 'Could not query canonical 7-Zip.'
  $sevenZipVersionText = $sevenZipVersionResult.Output -join [Environment]::NewLine
  $sevenZipVersionMatch = [regex]::Match(
    $sevenZipVersionText,
    '(?m)^7-Zip\s+(\d+)\.(\d+)(?:\.(\d+))?(?:\s|$)'
  )
  if (-not $sevenZipVersionMatch.Success) {
    throw 'Could not parse the canonical 7-Zip version.'
  }
  $sevenZipPatch = if ($sevenZipVersionMatch.Groups[3].Success) {
    [int]$sevenZipVersionMatch.Groups[3].Value
  }
  else {
    0
  }
  $sevenZipVersion = New-Object Version -ArgumentList @(
    [int]$sevenZipVersionMatch.Groups[1].Value,
    [int]$sevenZipVersionMatch.Groups[2].Value,
    $sevenZipPatch
  )
  if ($sevenZipVersion -lt [Version]'26.2.0') {
    throw "Canonical 7-Zip $sevenZipVersion is too old. Required >= 26.2.0."
  }

  $installerHashBeforeListing = (Get-FileHash -Algorithm SHA256 -LiteralPath $Installer).Hash.ToLowerInvariant()
  $installerBytesBeforeListing = [long](Get-Item -LiteralPath $Installer -Force).Length
  $listingLines = New-Object 'System.Collections.Generic.List[string]'
  $listingCharacterCount = 0
  & $sevenZipFullPath 'l' '-tNsis' '-slt' $Installer 2>&1 |
    ForEach-Object {
      $line = "$_"
      $listingCharacterCount += $line.Length
      if ($listingLines.Count -ge 200000 -or $listingCharacterCount -gt 16MB) {
        throw 'NSIS member listing exceeds the bounded parser input limit.'
      }
      $listingLines.Add($line)
    }
  $listingExitCode = $LASTEXITCODE
  if ($listingExitCode -ne 0) {
    throw "Could not enumerate the NSIS installer before extraction (exit $listingExitCode)."
  }
  $memberRecords = New-Object 'System.Collections.Generic.List[object]'
  $currentRecord = $null
  $inMemberSection = $false
  foreach ($line in $listingLines) {
    if ($line -match '^-{5,}$') {
      if ($null -ne $currentRecord -and $currentRecord.Count -ne 0) {
        $memberRecords.Add($currentRecord)
      }
      $inMemberSection = $true
      $currentRecord = [System.Collections.Generic.Dictionary[string, string]]::new(
        [System.StringComparer]::Ordinal
      )
      continue
    }
    if (-not $inMemberSection) {
      continue
    }
    if ([string]::IsNullOrWhiteSpace($line)) {
      if ($null -ne $currentRecord -and $currentRecord.Count -ne 0) {
        $memberRecords.Add($currentRecord)
        $currentRecord = [System.Collections.Generic.Dictionary[string, string]]::new(
          [System.StringComparer]::Ordinal
        )
      }
      continue
    }

    $propertyMatch = [regex]::Match($line, '^([^=]+?) = (.*)$')
    if (-not $propertyMatch.Success) {
      throw "NSIS member listing contains an unrecognized line: $line"
    }
    $propertyName = $propertyMatch.Groups[1].Value.TrimEnd()
    if ($currentRecord.ContainsKey($propertyName)) {
      throw "NSIS member listing repeats property '$propertyName' in one record."
    }
    $currentRecord.Add($propertyName, $propertyMatch.Groups[2].Value)
  }
  if ($null -ne $currentRecord -and $currentRecord.Count -ne 0) {
    $memberRecords.Add($currentRecord)
  }
  if ($memberRecords.Count -eq 0 -or $memberRecords.Count -gt 10000) {
    throw "NSIS listing has an invalid member count: $($memberRecords.Count)."
  }

  $listedPaths = New-Object 'System.Collections.Generic.List[string]'
  $listedPathSet = New-Object 'System.Collections.Generic.HashSet[string]' (
    [System.StringComparer]::OrdinalIgnoreCase
  )
  $listedSizesByPath = [System.Collections.Generic.Dictionary[string, long]]::new(
    [System.StringComparer]::OrdinalIgnoreCase
  )
  [long]$listedExpandedBytes = 0
  $listedFileCount = 0
  foreach ($record in $memberRecords) {
    if (-not $record.ContainsKey('Path') -or [string]::IsNullOrWhiteSpace($record['Path'])) {
      throw 'Every NSIS member record must contain one exact Path property.'
    }
    $normalizedPath = $record['Path'].Replace('/', '\')
    if ([System.IO.Path]::IsPathRooted($normalizedPath) -or
        $normalizedPath.StartsWith('\', [System.StringComparison]::Ordinal) -or
        $normalizedPath -match '^[A-Za-z]:' -or
        $normalizedPath.Contains(':')) {
      throw "NSIS listing contains a rooted, drive-qualified, or stream path: $normalizedPath"
    }
    $segments = @($normalizedPath.Split([char]'\'))
    if ($segments.Count -eq 0 -or @($segments | Where-Object {
          [string]::IsNullOrWhiteSpace($_) -or $_ -eq '.' -or $_ -eq '..'
        }).Count -ne 0) {
      throw "NSIS listing contains an empty, current, or parent path segment: $normalizedPath"
    }
    $invalidFileNameCharacters = [System.IO.Path]::GetInvalidFileNameChars()
    foreach ($segment in $segments) {
      if ($segment.IndexOfAny($invalidFileNameCharacters) -ge 0 -or
          $segment.EndsWith('.', [System.StringComparison]::Ordinal) -or
          $segment.EndsWith(' ', [System.StringComparison]::Ordinal) -or
          $segment -match '(?i)^(?:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|CLOCK\$|COM(?:[1-9]|[¹²³])|LPT(?:[1-9]|[¹²³]))(?:\..*)?$') {
        throw "NSIS listing contains an unsafe Windows path segment '$segment': $normalizedPath"
      }
    }
    if (-not $listedPathSet.Add($normalizedPath)) {
      throw "NSIS listing contains a case-folded duplicate path: $normalizedPath"
    }

    foreach ($unsupportedProperty in @('Symbolic Link', 'Hard Link', 'Reparse Point', 'Alternate Stream')) {
      if ($record.ContainsKey($unsupportedProperty)) {
        throw "NSIS listing contains unsupported link/reparse metadata '$unsupportedProperty': $normalizedPath"
      }
    }
    if (($record.ContainsKey('Anti') -and $record['Anti'] -eq '+') -or
        ($record.ContainsKey('Encrypted') -and $record['Encrypted'] -eq '+')) {
      throw "NSIS listing contains an anti or encrypted member: $normalizedPath"
    }
    # The official 7-Zip NSIS handler exposes each payload item as a Path/Size
    # record and does not emit the generic Folder property used by other archive
    # handlers. Treat every NSIS item as a sized payload record.
    if (-not $record.ContainsKey('Size')) {
      throw "NSIS payload member is missing exact Size metadata: $normalizedPath"
    }
    [long]$listedSize = 0
    if (-not [long]::TryParse($record['Size'], [ref]$listedSize) -or $listedSize -lt 0) {
      throw "NSIS payload member has an invalid size: $normalizedPath"
    }
    if ($listedSize -gt (1GB - $listedExpandedBytes)) {
      throw 'Listed NSIS payload exceeds the 1 GiB inspection limit.'
    }
    $listedExpandedBytes += $listedSize
    $listedFileCount += 1
    $listedPaths.Add($normalizedPath)
    $listedSizesByPath.Add($normalizedPath, $listedSize)
  }
  if ($listedFileCount -eq 0) {
    throw 'NSIS listing does not contain a regular file member.'
  }
  foreach ($requiredName in @('desktop.exe', 'ffmpeg.exe', 'LICENSE-ffmpeg.txt', 'ffmpeg-provenance.json')) {
    $listedMatches = @($listedPaths | Where-Object {
        $leafName = [System.IO.Path]::GetFileName(($_ -replace '/', '\'))
        [string]::Equals($leafName, $requiredName, [System.StringComparison]::OrdinalIgnoreCase)
      })
    if ($listedMatches.Count -ne 1) {
      throw "NSIS listing must contain exactly one '$requiredName' member; found $($listedMatches.Count)."
    }
    if ($listedSizesByPath[$listedMatches[0]] -le 0) {
      throw "NSIS listing declares an empty required payload member: $requiredName"
    }
    $listedLeafName = [System.IO.Path]::GetFileName(($listedMatches[0] -replace '/', '\'))
    if (-not [string]::Equals($listedLeafName, $requiredName, [System.StringComparison]::Ordinal)) {
      throw "NSIS listing contains case-variant member '$listedLeafName' instead of '$requiredName'."
    }
  }
  $installerHashAfterListing = (Get-FileHash -Algorithm SHA256 -LiteralPath $Installer).Hash.ToLowerInvariant()
  if (-not [string]::Equals(
      $installerHashAfterListing,
      $installerHashBeforeListing,
      [System.StringComparison]::Ordinal
    ) -or [long](Get-Item -LiteralPath $Installer -Force).Length -ne $installerBytesBeforeListing) {
    throw 'NSIS installer changed during safe member listing.'
  }

  $inspectionTemp = if (-not [string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    $env:RUNNER_TEMP
  }
  else {
    [System.IO.Path]::GetTempPath()
  }
  if ([string]::IsNullOrWhiteSpace($inspectionTemp) -or
      -not (Test-Path -LiteralPath $inspectionTemp -PathType Container)) {
    throw "A canonical temporary directory is required for isolated NSIS inspection."
  }
  $runnerTemp = Get-NormalizedFullPath -Path $inspectionTemp
  $runnerTemp = Assert-NoReparsePointInPath `
    -Path $runnerTemp `
    -Boundary ([System.IO.Path]::GetPathRoot($runnerTemp)) `
    -Label 'runner temporary directory'
  $inspectionId = [System.Guid]::NewGuid().ToString('N')
  $inspectionRoot = Join-Path $runnerTemp "stickerfit-nsis-inspection-$inspectionId"
  if (Test-Path -LiteralPath $inspectionRoot) {
    throw "Fresh NSIS inspection directory unexpectedly already exists: $inspectionRoot"
  }
  [void](New-Item -ItemType Directory -Path $inspectionRoot)
  $inspectionRoot = Assert-NoReparsePointInPath `
    -Path $inspectionRoot `
    -Boundary $runnerTemp `
    -Label 'NSIS inspection directory'
  $markerName = ".stickerfit-nsis-inspection-$inspectionId.marker"
  $markerPath = Join-Path $inspectionRoot $markerName
  $markerValue = "stickerfit-nsis-inspection-v1:$inspectionId"
  Write-NewUtf8File -Path $markerPath -Content $markerValue

  $installerHashBefore = (Get-FileHash -Algorithm SHA256 -LiteralPath $Installer).Hash.ToLowerInvariant()
  if (-not [string]::Equals(
      $installerHashBefore,
      $installerHashBeforeListing,
      [System.StringComparison]::Ordinal
    ) -or [long](Get-Item -LiteralPath $Installer -Force).Length -ne $installerBytesBeforeListing) {
    throw 'NSIS installer changed between member listing and extraction.'
  }
  $safeToRemove = $false
  try {
    Invoke-StrictNativeCommand `
      -Executable $sevenZipFullPath `
      -Arguments @('x', '-tNsis', '-y', "-o$inspectionRoot", $Installer) `
      -FailureMessage 'Could not statically extract the NSIS installer.' | Out-Null

    $extractedFiles = @(Get-ReleaseFilesWithoutReparseTraversal -RootPath $inspectionRoot)
    $safeToRemove = $true
    $payloadFiles = @($extractedFiles | Where-Object {
        -not [string]::Equals($_.Name, $markerName, [System.StringComparison]::Ordinal)
      })
    if ($payloadFiles.Count -eq 0 -or $payloadFiles.Count -gt 10000) {
      throw "Extracted NSIS payload has an invalid file count: $($payloadFiles.Count)"
    }
    [long]$expandedBytes = 0
    foreach ($file in $payloadFiles) {
      $expandedBytes += [long]$file.Length
      if ($expandedBytes -gt 1GB) {
        throw "Extracted NSIS payload exceeds the 1 GiB inspection limit."
      }
    }

    Assert-NoLegacyPayloadFiles -Files $payloadFiles -Label 'extracted NSIS payload'
    $extractedDesktop = Get-ExactPayloadFile -Files $payloadFiles -Name 'desktop.exe' -Label 'extracted NSIS payload'
    $extractedFfmpeg = Get-ExactPayloadFile -Files $payloadFiles -Name 'ffmpeg.exe' -Label 'extracted NSIS payload'
    $extractedLicense = Get-ExactPayloadFile -Files $payloadFiles -Name 'LICENSE-ffmpeg.txt' -Label 'extracted NSIS payload'
    $extractedProvenance = Get-ExactPayloadFile -Files $payloadFiles -Name 'ffmpeg-provenance.json' -Label 'extracted NSIS payload'
    if (-not [string]::Equals($extractedDesktop.DirectoryName, $extractedFfmpeg.DirectoryName, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not [string]::Equals($extractedFfmpeg.DirectoryName, $extractedLicense.DirectoryName, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not [string]::Equals($extractedFfmpeg.DirectoryName, $extractedProvenance.DirectoryName, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Extracted NSIS desktop, FFmpeg, license, and provenance must share one install directory."
    }

    Assert-ExactFileBytes `
      -ActualPath $extractedDesktop.FullName `
      -ExpectedPath $looseDesktop `
      -Label 'extracted NSIS desktop executable' `
      -ActualBoundary $inspectionRoot `
      -ExpectedBoundary $ReleaseRoot
    $trackedExecutable = Resolve-RepositoryPath `
      -WorkspaceRoot $Workspace `
      -Path "$($Vendor.Manifest.vendor.executablePath)"
    Assert-ExactFileBytes `
      -ActualPath $extractedFfmpeg.FullName `
      -ExpectedPath $trackedExecutable `
      -Label 'extracted NSIS FFmpeg sidecar' `
      -ActualBoundary $inspectionRoot `
      -ExpectedBoundary $Workspace
    Assert-ExactFileBytes `
      -ActualPath $extractedLicense.FullName `
      -ExpectedPath $TrackedLicense `
      -Label 'extracted NSIS normalized FFmpeg license' `
      -ActualBoundary $inspectionRoot `
      -ExpectedBoundary $Workspace
    Assert-ExactFileBytes `
      -ActualPath $extractedProvenance.FullName `
      -ExpectedPath $TrackedProvenance `
      -Label 'extracted NSIS FFmpeg provenance' `
      -ActualBoundary $inspectionRoot `
      -ExpectedBoundary $Workspace
    Assert-FfmpegFileSha256 `
      -Path $extractedFfmpeg.FullName `
      -ExpectedSha256 $Vendor.ArtifactSha256 `
      -Label 'extracted NSIS FFmpeg sidecar' | Out-Null
    if ([long]$extractedFfmpeg.Length -ne [long]$Vendor.ArtifactBytes) {
      throw "Extracted NSIS FFmpeg byte length does not match protected provenance."
    }

    $installerHashAfter = (Get-FileHash -Algorithm SHA256 -LiteralPath $Installer).Hash.ToLowerInvariant()
    if (-not [string]::Equals($installerHashBefore, $installerHashAfter, [System.StringComparison]::Ordinal) -or
        [long](Get-Item -LiteralPath $Installer -Force).Length -ne $installerBytesBeforeListing) {
      throw "NSIS installer changed during static payload inspection."
    }
  }
  finally {
    if ($safeToRemove -and (Test-Path -LiteralPath $inspectionRoot -PathType Container)) {
      [void](Assert-NoReparsePointInPath `
        -Path $inspectionRoot `
        -Boundary $runnerTemp `
        -Label 'NSIS inspection cleanup directory')
      [void](Get-ReleaseFilesWithoutReparseTraversal -RootPath $inspectionRoot)
      if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf) -or
          (Get-Content -Raw -LiteralPath $markerPath) -ne $markerValue) {
        throw "Refusing to remove an unmarked NSIS inspection directory."
      }
      Remove-Item -LiteralPath $inspectionRoot -Recurse -Force
    }
    elseif (Test-Path -LiteralPath $inspectionRoot) {
      Write-Warning "Unsafe or incomplete NSIS inspection directory was left for ephemeral-runner cleanup: $inspectionRoot"
    }
  }
}

function Assert-NonLegacyTauriResourceContract {
  param(
    [Parameter(Mandatory = $true)][string]$Workspace,
    [Parameter(Mandatory = $true)][string]$PackagedRoot
  )

  $configPath = Join-Path $Workspace 'src-tauri\tauri.conf.json'
  [void](Assert-NoReparsePointInPath `
    -Path $configPath `
    -Boundary $Workspace `
    -Label "release Tauri configuration")
  $config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
  $bundle = Get-ExactJsonProperty -Object $config -Name 'bundle' -Label 'release Tauri configuration'
  $resources = Get-ExactJsonProperty -Object $bundle -Name 'resources' -Label 'release Tauri bundle'
  $expectedResources = [ordered]@{
    'binaries/LICENSE-ffmpeg.txt' = 'LICENSE-ffmpeg.txt'
    'binaries/ffmpeg-provenance.json' = 'ffmpeg-provenance.json'
  }
  $resourceProperties = @($resources.PSObject.Properties | Where-Object {
      $_.MemberType -eq [System.Management.Automation.PSMemberTypes]::NoteProperty
    })
  if ($resourceProperties.Count -ne $expectedResources.Count) {
    throw 'Release Tauri config must contain the exact non-legacy FFmpeg resource map.'
  }

  foreach ($entry in $expectedResources.GetEnumerator()) {
    $matchingProperties = @($resourceProperties | Where-Object {
        [string]::Equals([string]$_.Name, [string]$entry.Key, [System.StringComparison]::Ordinal)
      })
    if ($matchingProperties.Count -ne 1 -or
        $matchingProperties[0].Value -isnot [string] -or
        -not [string]::Equals([string]$matchingProperties[0].Value, [string]$entry.Value, [System.StringComparison]::Ordinal)) {
      throw "Release Tauri config has an unexpected resource mapping for '$($entry.Key)'."
    }

    $sourceRelativePath = 'src-tauri\' + ($entry.Key -replace '/', '\')
    Assert-ExactFileBytes `
      -ActualPath (Join-Path $PackagedRoot $entry.Value) `
      -ExpectedPath (Join-Path $Workspace $sourceRelativePath) `
      -Label "configured release resource '$($entry.Value)'" `
      -ActualBoundary $PackagedRoot `
      -ExpectedBoundary $Workspace
  }

  $externalBinValue = Get-ExactJsonProperty -Object $bundle -Name 'externalBin' -Label 'release Tauri bundle'
  if ($externalBinValue -isnot [System.Array]) {
    throw 'Release Tauri config externalBin must be an exact JSON array.'
  }
  $externalBins = @($externalBinValue)
  if ($externalBins.Count -ne 1 -or
      $externalBins[0] -isnot [string] -or
      -not [string]::Equals([string]$externalBins[0], 'binaries/ffmpeg', [System.StringComparison]::Ordinal)) {
    throw 'Release Tauri config must contain only the protected FFmpeg externalBin mapping.'
  }
}

# Release verification is intentionally strict even when callers omit the
# compatibility switch. The switch remains part of the workflow-facing CLI so
# release YAML can state the policy explicitly.
$resolvedWorkspaceRoot = Get-NormalizedFullPath -Path $WorkspaceRoot
$workspaceDriveRoot = [System.IO.Path]::GetPathRoot($resolvedWorkspaceRoot)
$resolvedWorkspaceRoot = Assert-NoReparsePointInPath `
  -Path $resolvedWorkspaceRoot `
  -Boundary $workspaceDriveRoot `
  -Label "release workspace"
$WorkspaceRoot = $resolvedWorkspaceRoot
$resolvedPolicyRoot = Get-NormalizedFullPath -Path $PolicyRoot
$policyDriveRoot = [System.IO.Path]::GetPathRoot($resolvedPolicyRoot)
$resolvedPolicyRoot = Assert-NoReparsePointInPath `
  -Path $resolvedPolicyRoot `
  -Boundary $policyDriveRoot `
  -Label "release verification policy root"
$PolicyRoot = $resolvedPolicyRoot
$modulePath = Join-Path $resolvedPolicyRoot "tools\ffmpeg\FfmpegSourceVerification.psm1"
[void](Assert-NoReparsePointInPath `
  -Path $modulePath `
  -Boundary $resolvedPolicyRoot `
  -Label "release verification module")
Import-Module -Name $modulePath -Force

$vendor = Test-FfmpegVendorArtifacts -WorkspaceRoot $resolvedWorkspaceRoot
if ($vendor.LegacyBootstrap) {
  throw "Release sidecar validation rejects legacy FFmpeg provenance."
}

if (-not $RequireNonLegacyProvenance) {
  Write-Verbose "Non-legacy provenance is mandatory for every release-sidecar validation."
}

$expectedReleaseDirectory = Get-NormalizedFullPath -Path (
  Join-Path $resolvedWorkspaceRoot "src-tauri\target\release"
)
if (-not $ReleaseDirectory) { $ReleaseDirectory = $expectedReleaseDirectory }
$resolvedReleaseDirectory = Get-NormalizedFullPath -Path $ReleaseDirectory
if (-not [string]::Equals(
    $resolvedReleaseDirectory,
    $expectedReleaseDirectory,
    [System.StringComparison]::OrdinalIgnoreCase
  )) {
  throw "Release directory must be the canonical workspace release directory: $expectedReleaseDirectory"
}
if (-not (Test-Path -LiteralPath $resolvedReleaseDirectory -PathType Container)) {
  throw "Release directory was not found: $resolvedReleaseDirectory"
}
$resolvedReleaseDirectory = Assert-NoReparsePointInPath `
  -Path $resolvedReleaseDirectory `
  -Boundary $resolvedWorkspaceRoot `
  -Label "release directory"

$canonicalNsisRoot = Join-Path $resolvedReleaseDirectory 'bundle\nsis'
$canonicalNsisRoot = Assert-NoReparsePointInPath `
  -Path $canonicalNsisRoot `
  -Boundary $resolvedReleaseDirectory `
  -Label 'canonical NSIS bundle directory'
$canonicalNsisEntries = @(Get-ChildItem -LiteralPath $canonicalNsisRoot -Force)
if ($canonicalNsisEntries.Count -ne 1 -or
    $canonicalNsisEntries[0].PSIsContainer -or
    ($canonicalNsisEntries[0].Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
    -not [string]::Equals($canonicalNsisEntries[0].Extension, '.exe', [System.StringComparison]::OrdinalIgnoreCase)) {
  throw 'Canonical NSIS bundle directory must contain exactly one regular installer executable.'
}
$canonicalInstallerPath = $canonicalNsisEntries[0].FullName
if ([string]::IsNullOrWhiteSpace($InstallerPath)) {
  $InstallerPath = $canonicalInstallerPath
}
elseif (-not [string]::Equals(
    [System.IO.Path]::GetFullPath($InstallerPath),
    $canonicalInstallerPath,
    [System.StringComparison]::OrdinalIgnoreCase
  )) {
  throw "InstallerPath must select the only canonical NSIS installer: $canonicalInstallerPath"
}

$canonicalSevenZipPath = Get-CanonicalSevenZipPath
if ([string]::IsNullOrWhiteSpace($SevenZipPath)) {
  $SevenZipPath = $canonicalSevenZipPath
}
elseif (-not [string]::Equals(
    [System.IO.Path]::GetFullPath($SevenZipPath),
    $canonicalSevenZipPath,
    [System.StringComparison]::OrdinalIgnoreCase
  )) {
  throw "SevenZipPath must select canonical 7-Zip: $canonicalSevenZipPath"
}

$ffmpegPath = Join-Path $resolvedReleaseDirectory "ffmpeg.exe"
if (-not (Test-Path -LiteralPath $ffmpegPath -PathType Leaf)) {
  throw "Release FFmpeg sidecar was not found: $ffmpegPath"
}
$ffmpegPath = Assert-NoReparsePointInPath `
  -Path $ffmpegPath `
  -Boundary $resolvedWorkspaceRoot `
  -Label "release FFmpeg sidecar"
$ffmpegItem = Get-Item -LiteralPath $ffmpegPath -Force
if (-not [string]::Equals($ffmpegItem.Name, "ffmpeg.exe", [System.StringComparison]::Ordinal)) {
  throw "Release FFmpeg sidecar filename must be exactly 'ffmpeg.exe'."
}

$releaseFiles = @(Get-ReleaseFilesWithoutReparseTraversal -RootPath $resolvedReleaseDirectory)
Assert-NoLegacyPayloadFiles -Files $releaseFiles -Label 'loose release payload'

Assert-NonLegacyTauriResourceContract `
  -Workspace $WorkspaceRoot `
  -PackagedRoot $resolvedReleaseDirectory

if (@($vendor.Provenance.runtimeDependencies).Count -ne 0 -or
    @($vendor.Manifest.vendor.expectedRuntimeDependencies).Count -ne 0) {
  throw "Release FFmpeg metadata must declare no bundled runtime DLL dependencies."
}

$packagedLicensePath = Join-Path $resolvedReleaseDirectory 'LICENSE-ffmpeg.txt'
$licenseRecords = @($vendor.Provenance.companionFiles | Where-Object {
    [System.IO.Path]::GetFileName("$($_.path)") -eq 'LICENSE-ffmpeg.txt'
  })
if ($licenseRecords.Count -ne 1) {
  throw "Non-legacy provenance must contain exactly one normalized FFmpeg license record."
}

$licenseRecord = $licenseRecords[0]
$trackedLicensePath = Resolve-RepositoryPath `
  -WorkspaceRoot $WorkspaceRoot `
  -Path "$($licenseRecord.path)"
[void](Assert-NoReparsePointInPath `
  -Path $packagedLicensePath `
  -Boundary $resolvedWorkspaceRoot `
  -Label "packaged normalized FFmpeg license")
[void](Assert-NoReparsePointInPath `
  -Path $trackedLicensePath `
  -Boundary $resolvedWorkspaceRoot `
  -Label "tracked normalized FFmpeg license")
Assert-FfmpegFileSha256 `
  -Path $packagedLicensePath `
  -ExpectedSha256 "$($licenseRecord.sha256)" `
  -Label "packaged normalized FFmpeg license" | Out-Null
if ((Get-Item -LiteralPath $packagedLicensePath).Length -ne [long]$licenseRecord.bytes) {
  throw "Packaged normalized FFmpeg license byte length does not match protected provenance."
}

Assert-ExactFileBytes `
  -ActualPath $packagedLicensePath `
  -ExpectedPath $trackedLicensePath `
  -Label "packaged normalized FFmpeg license" `
  -ActualBoundary $resolvedReleaseDirectory `
  -ExpectedBoundary $resolvedWorkspaceRoot

$packagedProvenancePath = Join-Path $resolvedReleaseDirectory 'ffmpeg-provenance.json'
$trackedProvenancePath = Resolve-RepositoryPath `
  -WorkspaceRoot $WorkspaceRoot `
  -Path "$($vendor.Manifest.vendor.provenancePath)"
[void](Assert-NoReparsePointInPath `
  -Path $packagedProvenancePath `
  -Boundary $resolvedWorkspaceRoot `
  -Label "packaged FFmpeg provenance")
[void](Assert-NoReparsePointInPath `
  -Path $trackedProvenancePath `
  -Boundary $resolvedWorkspaceRoot `
  -Label "tracked FFmpeg provenance")
$trackedProvenanceSha256 = (Get-FileHash `
    -Algorithm SHA256 `
    -LiteralPath $trackedProvenancePath).Hash.ToLowerInvariant()
Assert-FfmpegFileSha256 `
  -Path $packagedProvenancePath `
  -ExpectedSha256 $trackedProvenanceSha256 `
  -Label "packaged FFmpeg provenance" | Out-Null
Assert-ExactFileBytes `
  -ActualPath $packagedProvenancePath `
  -ExpectedPath $trackedProvenancePath `
  -Label "packaged FFmpeg provenance" `
  -ActualBoundary $resolvedReleaseDirectory `
  -ExpectedBoundary $resolvedWorkspaceRoot

Assert-FfmpegFileSha256 `
  -Path $ffmpegPath `
  -ExpectedSha256 $vendor.ArtifactSha256 `
  -Label "packaged release FFmpeg sidecar" | Out-Null
$packagedBytes = (Get-Item -LiteralPath $ffmpegPath).Length
if ($packagedBytes -ne $vendor.ArtifactBytes) {
  throw "Packaged release FFmpeg sidecar byte length does not match protected provenance."
}

$installerHashBeforeStaticVerification = (Get-FileHash `
    -Algorithm SHA256 `
    -LiteralPath $InstallerPath).Hash.ToLowerInvariant()
$installerBytesBeforeStaticVerification = [long](Get-Item -LiteralPath $InstallerPath -Force).Length
Assert-ExtractedInstallerPayload `
  -Installer $InstallerPath `
  -SevenZip $SevenZipPath `
  -ReleaseRoot $resolvedReleaseDirectory `
  -Workspace $resolvedWorkspaceRoot `
  -Vendor $vendor `
  -TrackedLicense $trackedLicensePath `
  -TrackedProvenance $trackedProvenancePath
$installerHashAfterStaticVerification = (Get-FileHash `
    -Algorithm SHA256 `
    -LiteralPath $InstallerPath).Hash.ToLowerInvariant()
if (-not [string]::Equals(
    $installerHashAfterStaticVerification,
    $installerHashBeforeStaticVerification,
    [System.StringComparison]::Ordinal
  ) -or [long](Get-Item -LiteralPath $InstallerPath -Force).Length -ne $installerBytesBeforeStaticVerification) {
  throw 'NSIS installer changed across static verification.'
}

if (-not $StaticOnly) {
  $versionResult = Invoke-StrictNativeCommand `
    -Executable $ffmpegPath `
    -Arguments @('-hide_banner', '-version') `
    -FailureMessage "Packaged FFmpeg sidecar failed to start."
  $versionText = $versionResult.Output -join [Environment]::NewLine
  $escapedVersion = [regex]::Escape($vendor.Version)
  if ($versionText -notmatch "(?m)^ffmpeg version\s+$escapedVersion(?:\s|$)") {
    throw "Packaged FFmpeg version output does not match protected provenance version $($vendor.Version)."
  }

  $capabilityCommands = @(
    [pscustomobject]@{ Property = 'demuxers'; Argument = '-demuxers' },
    [pscustomobject]@{ Property = 'decoders'; Argument = '-decoders' },
    [pscustomobject]@{ Property = 'muxers'; Argument = '-muxers' },
    [pscustomobject]@{ Property = 'encoders'; Argument = '-encoders' },
    [pscustomobject]@{ Property = 'parsers'; Argument = '-parsers' },
    [pscustomobject]@{ Property = 'protocols'; Argument = '-protocols' },
    [pscustomobject]@{ Property = 'filters'; Argument = '-filters' }
  )

  foreach ($capabilityCommand in $capabilityCommands) {
    $result = Invoke-StrictNativeCommand `
      -Executable $ffmpegPath `
      -Arguments @('-hide_banner', $capabilityCommand.Argument) `
      -FailureMessage "Packaged FFmpeg could not enumerate $($capabilityCommand.Property)."
    $outputText = $result.Output -join [Environment]::NewLine
    foreach ($capabilityObject in @(
        $vendor.Manifest.configure.capabilities.($capabilityCommand.Property)
      )) {
      $capability = "$capabilityObject"
      $escapedCapability = [regex]::Escape($capability)
      if ($outputText -notmatch "(?m)^\s*(?:[A-Z\.]{1,8}\s+)?$escapedCapability(?:\s|$)") {
        throw "Packaged FFmpeg is missing required $($capabilityCommand.Property) capability: $capability"
      }
    }
  }

  $installerHashAfterNativeChecks = (Get-FileHash `
      -Algorithm SHA256 `
      -LiteralPath $InstallerPath).Hash.ToLowerInvariant()
  if (-not [string]::Equals(
      $installerHashAfterNativeChecks,
      $installerHashAfterStaticVerification,
      [System.StringComparison]::Ordinal
    )) {
    throw 'NSIS installer changed after static verification during native capability checks.'
  }
}

$verificationMode = if ($StaticOnly) { 'static-only' } else { 'static and native-capability' }
Write-Host "Release FFmpeg sidecar OK: version $($vendor.Version), $verificationMode, loose and extracted NSIS payloads, exact protected bytes, no legacy DLL."
