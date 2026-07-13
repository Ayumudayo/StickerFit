[CmdletBinding()]
param(
  [string]$WorkspaceRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $PSBoundParameters.ContainsKey("WorkspaceRoot")) {
  $WorkspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
}

$modulePath = Join-Path $WorkspaceRoot "tools\ffmpeg\FfmpegSourceVerification.psm1"
Import-Module -Name $modulePath -Force

$expectedEolPolicy = @(
  '/.gitattributes text eol=lf',
  '/src/**/*.ts text eol=lf',
  '/src/**/*.tsx text eol=lf',
  '/src/**/*.css text eol=lf',
  '/tests/**/*.ts text eol=lf',
  '/package*.json text eol=lf',
  '/.github/**/*.yml text eol=lf',
  '/src-tauri/Cargo.toml text eol=lf',
  '/src-tauri/tauri.conf.json text eol=lf',
  '/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe -text',
  '/tools/test-release-sidecar.ps1 text eol=lf',
  '/tools/ffmpeg/FfmpegSourceVerification.psm1 text eol=lf',
  '/tools/ffmpeg/ffmpeg-release-signing-key.asc text eol=lf',
  '/tools/ffmpeg/ffmpeg-version.json text eol=lf',
  '/src-tauri/binaries/ffmpeg-provenance.json text eol=lf',
  '/src-tauri/binaries/LICENSE-ffmpeg.txt text eol=lf',
  '/src-tauri/binaries/LICENSE-libwinpthread.txt text eol=lf',
  '/src-tauri/tests/fixtures/LICENSE-chromium.txt text eol=lf',
  '/src-tauri/tests/fixtures/media-fixtures.json text eol=lf',
  '/src-tauri/binaries/LICENSE-ffmpeg-BtbN.txt text eol=crlf'
)
$gitattributesPath = Join-Path $WorkspaceRoot '.gitattributes'
$actualEolPolicy = @(Get-Content -LiteralPath $gitattributesPath)
if ($actualEolPolicy.Count -ne $expectedEolPolicy.Count) {
  throw "FFmpeg trust-pinned EOL policy membership changed."
}

for ($policyIndex = 0; $policyIndex -lt $expectedEolPolicy.Count; $policyIndex += 1) {
  if (-not [string]::Equals(
      $actualEolPolicy[$policyIndex],
      $expectedEolPolicy[$policyIndex],
      [System.StringComparison]::Ordinal
    )) {
    throw "FFmpeg trust-pinned EOL policy mismatch at index $policyIndex."
  }
}

$moduleTokens = $null
$moduleParseErrors = $null
$moduleAst = [System.Management.Automation.Language.Parser]::ParseFile(
  $modulePath,
  [ref]$moduleTokens,
  [ref]$moduleParseErrors
)
if ($moduleParseErrors.Count -ne 0) {
  throw "FFmpeg verification module must parse before Write-FfmpegJson contract checks."
}

$writeJsonFunctions = @($moduleAst.FindAll({
      param($node)
      $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Write-FfmpegJson'
    }, $true))
if ($writeJsonFunctions.Count -ne 1) {
  throw "Expected exactly one Write-FfmpegJson function."
}

$writeJsonSource = $writeJsonFunctions[0].Extent.Text
if (-not $writeJsonSource.Contains('$json + "`n"') -or
    -not $writeJsonSource.Contains('-replace "`r`n?", "`n"') -or
    $writeJsonSource.Contains('[Environment]::NewLine')) {
  throw "Write-FfmpegJson must normalize JSON to LF and terminate with exactly one explicit LF."
}

function Assert-True {
  param(
    [Parameter(Mandatory = $true)]
    [bool]$Condition,

    [Parameter(Mandatory = $true)]
    [string]$Message
  )

  if (-not $Condition) {
    throw $Message
  }
}

function Assert-Throws {
  param(
    [Parameter(Mandatory = $true)]
    [scriptblock]$Action,

    [Parameter(Mandatory = $true)]
    [string]$MessagePattern
  )

  try {
    & $Action
  }
  catch {
    if ($_.Exception.Message -notmatch $MessagePattern) {
      throw "Expected error matching '$MessagePattern', found '$($_.Exception.Message)'."
    }

    return
  }

  throw "Expected action to fail with a message matching '$MessagePattern'."
}

function Assert-NoReparsePointInTestTree {
  param(
    [Parameter(Mandatory = $true)]
    [string]$RootPath
  )

  $pendingDirectories = New-Object 'System.Collections.Generic.Queue[string]'
  $pendingDirectories.Enqueue([System.IO.Path]::GetFullPath($RootPath))
  while ($pendingDirectories.Count -gt 0) {
    $currentDirectory = $pendingDirectories.Dequeue()
    foreach ($child in @(Get-ChildItem -LiteralPath $currentDirectory -Force)) {
      if (($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "FFmpeg contract-test tree contains a reparse-point descendant."
      }

      if (($child.Attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
        $pendingDirectories.Enqueue($child.FullName)
      }
    }
  }
}

$unlaunchableNativePath = Join-Path (
  [System.IO.Path]::GetTempPath()
) "stickerfit-unlaunchable-$([guid]::NewGuid().ToString('N')).exe"
try {
  [System.IO.File]::WriteAllText(
    $unlaunchableNativePath,
    'This is deliberately not a native executable.',
    (New-Object System.Text.UTF8Encoding($false))
  )
  Assert-Throws `
    -Action {
      Invoke-StrictNativeCommand `
        -Executable $unlaunchableNativePath `
        -FailureMessage 'Unlaunchable native fixture was rejected.' | Out-Null
    } `
    -MessagePattern 'Unlaunchable native fixture was rejected\. Exit code: unavailable\.'
}
finally {
  Remove-Item -LiteralPath $unlaunchableNativePath -Force -ErrorAction SilentlyContinue
}

$manifest = Get-FfmpegManifest -WorkspaceRoot $WorkspaceRoot
$expectedFingerprint = 'FCF986EA15E6E293A5644F10B4322F04D67658D8'
Assert-True `
  -Condition ("$($manifest.release.primaryFingerprint)" -eq $expectedFingerprint) `
  -Message "Manifest release fingerprint changed unexpectedly."
Assert-True `
  -Condition ("$($manifest.releaseKeyFingerprint)" -eq "$($manifest.release.primaryFingerprint)") `
  -Message "Canonical releaseKeyFingerprint must match nested release metadata."
Assert-True `
  -Condition ("$($manifest.sourceArchiveSha256)" -eq "$($manifest.release.archiveSha256)") `
  -Message "Canonical sourceArchiveSha256 must match nested release metadata."
Assert-True `
  -Condition (@($manifest.configure.capabilities.parsers) -contains 'mpeg4video') `
  -Message "New-build parser contract must use mpeg4video."
Assert-True `
  -Condition (@($manifest.configure.capabilities.parsers) -notcontains 'mpeg4') `
  -Message "New-build parser contract must not use the legacy mpeg4 parser spelling."

$expectedRoot = 'ffmpeg-7.1.3'
foreach ($safeEntry in @(
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/'; Type = 'd' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/configure'; Type = '-' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/libavcodec/a.c'; Type = '-' }
  )) {
  Test-FfmpegArchiveEntry `
    -EntryName $safeEntry.Name `
    -EntryType $safeEntry.Type `
    -ExpectedTopLevelDirectory $expectedRoot | Out-Null
}

foreach ($unsafeEntry in @(
    [pscustomobject]@{ Name = '/etc/passwd'; Type = '-'; Pattern = 'rooted' },
    [pscustomobject]@{ Name = '\\server\share\file'; Type = '-'; Pattern = 'rooted' },
    [pscustomobject]@{ Name = 'C:\escape\file'; Type = '-'; Pattern = 'rooted' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/../escape'; Type = '-'; Pattern = 'escapes' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3\..\escape'; Type = '-'; Pattern = 'escapes' },
    [pscustomobject]@{ Name = 'other-root/file'; Type = '-'; Pattern = 'expected top-level' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/link'; Type = 'l'; Pattern = 'link' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/hardlink'; Type = 'h'; Pattern = 'link' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/device'; Type = 'c'; Pattern = 'device' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/fifo'; Type = 'p'; Pattern = 'FIFO' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/socket'; Type = 's'; Pattern = 'socket' },
    [pscustomobject]@{ Name = 'ffmpeg-7.1.3/unknown'; Type = 'x'; Pattern = 'special' }
  )) {
  $entry = $unsafeEntry
  Assert-Throws `
    -Action {
      Test-FfmpegArchiveEntry `
        -EntryName $entry.Name `
        -EntryType $entry.Type `
        -ExpectedTopLevelDirectory $expectedRoot | Out-Null
    } `
    -MessagePattern $entry.Pattern
}

$validKeyInventory = @(
  'pub:-:2048:1:B4322F04D67658D8:1303817565:::-:::scESC::::::23::0:',
  'fpr:::::::::FCF986EA15E6E293A5644F10B4322F04D67658D8:',
  'uid:-::::1303817565:::::::::',
  'sub:-:2048:1:50EE8DF19C3345A2:1303817565::::::e::::::23:',
  'fpr:::::::::5F2EDE9A44501BB559871F8650EE8DF19C3345A2:'
)
Assert-FfmpegPrimaryKeyInventory `
  -ColonListing $validKeyInventory `
  -ExpectedPrimaryFingerprint $expectedFingerprint
Assert-Throws `
  -Action {
    Assert-FfmpegPrimaryKeyInventory `
      -ColonListing ($validKeyInventory + $validKeyInventory) `
      -ExpectedPrimaryFingerprint $expectedFingerprint
  } `
  -MessagePattern 'exactly one primary key'
Assert-Throws `
  -Action {
    Assert-FfmpegPrimaryKeyInventory `
      -ColonListing $validKeyInventory `
      -ExpectedPrimaryFingerprint 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
  } `
  -MessagePattern 'fingerprint mismatch'
Assert-Throws `
  -Action {
    Assert-FfmpegValidSignatureStatus `
      -StatusLines @('[GNUPG:] BADSIG B4322F04D67658D8 FFmpeg release signing key') `
      -ExpectedPrimaryFingerprint $expectedFingerprint
  } `
  -MessagePattern 'bad signature'
Assert-Throws `
  -Action {
    Assert-FfmpegValidSignatureStatus `
      -StatusLines @('[GNUPG:] NO_PUBKEY B4322F04D67658D8') `
      -ExpectedPrimaryFingerprint $expectedFingerprint
  } `
  -MessagePattern 'bad signature'
Assert-Throws `
  -Action {
    Assert-FfmpegValidSignatureStatus `
      -StatusLines @('[GNUPG:] NEWSIG ffmpeg-devel@ffmpeg.org') `
      -ExpectedPrimaryFingerprint $expectedFingerprint
  } `
  -MessagePattern 'exactly one VALIDSIG'

$validSignatureStatus = @(
  '[GNUPG:] NEWSIG ffmpeg-devel@ffmpeg.org',
  '[GNUPG:] VALIDSIG FCF986EA15E6E293A5644F10B4322F04D67658D8 2025-11-21 1763687854 0 4 0 1 10 00 FCF986EA15E6E293A5644F10B4322F04D67658D8'
)
Assert-FfmpegValidSignatureStatus `
  -StatusLines $validSignatureStatus `
  -ExpectedPrimaryFingerprint $expectedFingerprint
Assert-Throws `
  -Action {
    Assert-FfmpegValidSignatureStatus `
      -StatusLines ($validSignatureStatus + $validSignatureStatus[1]) `
      -ExpectedPrimaryFingerprint $expectedFingerprint
  } `
  -MessagePattern 'exactly one VALIDSIG'
Assert-Throws `
  -Action {
    Assert-FfmpegValidSignatureStatus `
      -StatusLines @('[GNUPG:] VALIDSIG AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA 0 0 0 0 0 0 0 0 0 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA') `
      -ExpectedPrimaryFingerprint $expectedFingerprint
  } `
  -MessagePattern 'fingerprint mismatch'

$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
  'stickerfit-ffmpeg-contract-' + [Guid]::NewGuid().ToString('N')
)
$markerPath = Join-Path $temporaryRoot '.stickerfit-test.marker'
try {
  New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
  [System.IO.File]::WriteAllText($markerPath, 'stickerfit-ffmpeg-contract-v1')
  $hashFixture = Join-Path $temporaryRoot 'abc.txt'
  [System.IO.File]::WriteAllText(
    $hashFixture,
    'abc',
    [System.Text.UTF8Encoding]::new($false)
  )
  $jsonFixture = Join-Path $temporaryRoot 'write-json-fixture.json'
  Write-FfmpegJson `
    -Path $jsonFixture `
    -Value ([ordered]@{
      alpha = 1
      nested = [ordered]@{ beta = 2 }
    })
  $jsonBytes = [System.IO.File]::ReadAllBytes($jsonFixture)
  Assert-True `
    -Condition ($jsonBytes.Length -gt 1 -and
      $jsonBytes[$jsonBytes.Length - 1] -eq 10 -and
      $jsonBytes[$jsonBytes.Length - 2] -ne 10 -and
      $jsonBytes[$jsonBytes.Length - 2] -ne 13 -and
      $jsonBytes -notcontains 13) `
    -Message "Write-FfmpegJson output must use LF only and end in exactly one LF."
  Assert-FfmpegFileSha256 `
    -Path $hashFixture `
    -ExpectedSha256 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad' `
    -Label 'hash fixture' | Out-Null
  $computedWithoutExpectedHash = Get-FfmpegVerifiedArchiveSha256 `
    -ArchivePath $hashFixture `
    -Label 'update archive fixture'
  Assert-True `
    -Condition ($computedWithoutExpectedHash -eq 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad') `
    -Message "Update verification must record a computed source hash when no expected hash is supplied."
  Assert-Throws `
    -Action {
      Assert-FfmpegFileSha256 `
        -Path $hashFixture `
        -ExpectedSha256 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' `
        -Label 'tampered cache' | Out-Null
    } `
    -MessagePattern 'SHA-256 mismatch'
  # Model the orchestrator order deterministically: a valid signature status is
  # accepted first, then a supplied routine/update authority is compared with
  # the bytes. This does not generate an ephemeral signing key.
  Assert-FfmpegValidSignatureStatus `
    -StatusLines $validSignatureStatus `
    -ExpectedPrimaryFingerprint $expectedFingerprint
  Assert-Throws `
    -Action {
      Get-FfmpegVerifiedArchiveSha256 `
        -ArchivePath $hashFixture `
        -ExpectedSha256 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' `
        -Label 'signed but wrong committed source hash' | Out-Null
    } `
    -MessagePattern 'SHA-256 mismatch'

  $buildScriptPath = Join-Path $WorkspaceRoot 'tools\ffmpeg\build-minimal-ffmpeg.ps1'
  Assert-Throws `
    -Action {
      & $buildScriptPath `
        -VerifyVendorArtifacts `
        -FfmpegSourceRoot $temporaryRoot
    } `
    -MessagePattern 'not approved'
}
finally {
  if (Test-Path -LiteralPath $temporaryRoot -PathType Container) {
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf) -or
        (Get-Content -Raw -LiteralPath $markerPath) -ne 'stickerfit-ffmpeg-contract-v1') {
      throw "Refusing to remove an unmarked FFmpeg contract-test directory."
    }

    $temporaryDirectoryItem = Get-Item -LiteralPath $temporaryRoot -Force
    $temporaryMarkerItem = Get-Item -LiteralPath $markerPath -Force
    if (($temporaryDirectoryItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        ($temporaryMarkerItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Refusing to remove a reparse-point FFmpeg contract-test directory."
    }

    Assert-NoReparsePointInTestTree -RootPath $temporaryRoot

    Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
  }
}

$buildScriptPath = Join-Path $WorkspaceRoot 'tools\ffmpeg\build-minimal-ffmpeg.ps1'
$buildTokens = $null
$buildParseErrors = $null
$buildAst = [System.Management.Automation.Language.Parser]::ParseFile(
  $buildScriptPath,
  [ref]$buildTokens,
  [ref]$buildParseErrors
)
Assert-True `
  -Condition ($buildParseErrors.Count -eq 0) `
  -Message "FFmpeg build orchestrator must parse before parameter-contract checks."
$parameterNames = @($buildAst.ParamBlock.Parameters | ForEach-Object {
    $_.Name.VariablePath.UserPath
  })
foreach ($requiredParameterName in @(
    'VerifySourceOnly',
    'VerifyVendorArtifacts',
    'UpdateVendorArtifacts',
    'TargetVersion',
    'ExpectedSourceSha256',
    'SourceDateEpoch',
    'StagingRoot',
    'FfmpegSourceRoot',
    'BashPath',
    'GpgPath',
    'GpgvPath'
  )) {
  Assert-True `
    -Condition ($parameterNames -contains $requiredParameterName) `
    -Message "FFmpeg build orchestrator parameter contract is missing: $requiredParameterName"
}

$expectedHashParameter = @($buildAst.ParamBlock.Parameters | Where-Object {
    $_.Name.VariablePath.UserPath -eq 'ExpectedSourceSha256'
  })
$mandatoryExpectedHashAttributes = @($expectedHashParameter.Attributes | Where-Object {
    $_.TypeName.Name -eq 'Parameter' -and $_.NamedArguments.ArgumentName -contains 'Mandatory'
  })
Assert-True `
  -Condition ($mandatoryExpectedHashAttributes.Count -eq 0) `
  -Message "ExpectedSourceSha256 must remain optional for protected update compute-and-record mode."

$buildSource = Get-Content -Raw -LiteralPath $buildScriptPath
Assert-True `
  -Condition ($buildSource.Contains('Select exactly one mode: -VerifySourceOnly, -VerifyVendorArtifacts, or -UpdateVendorArtifacts.')) `
  -Message "FFmpeg orchestrator must retain mutually exclusive source/vendor/update modes."
Assert-True `
  -Condition ($buildSource.Contains('Unpacked local FFmpeg source roots are not approved.')) `
  -Message "FFmpeg orchestrator must reject unapproved unpacked local source roots."
Assert-True `
  -Condition ($buildSource.Contains("[Alias('OutputDirectory')]")) `
  -Message "Protected update output-directory alias is missing."
Assert-True `
  -Condition ($buildSource.Contains("'Ayumudayo/StickerFit'")) `
  -Message "Protected vendor update must pin the exact repository authority."
foreach ($requiredTransitionContract in @(
    "`$PSBoundParameters.ContainsKey('DownloadCacheRoot')",
    'stickerfit-ffmpeg-download-cache-$($env:GITHUB_RUN_ID)',
    "destination = 'src-tauri/tauri.conf.json'",
    "'binaries/LICENSE-ffmpeg.txt' = 'LICENSE-ffmpeg.txt'",
    "'binaries/ffmpeg-provenance.json' = 'ffmpeg-provenance.json'"
  )) {
  Assert-True `
    -Condition ($buildSource.Contains($requiredTransitionContract)) `
    -Message "Protected vendor transition contract is missing: $requiredTransitionContract"
}
$protectedCacheResetIndex = $buildSource.IndexOf('$protectedDownloadCacheRoot = Reset-ProtectedStagingRoot')
$updateSourceVerificationIndex = $buildSource.IndexOf('$verifiedSource = Invoke-FfmpegSourceVerification', $protectedCacheResetIndex)
$protectedCacheCleanupIndex = $buildSource.LastIndexOf('-Path $protectedDownloadCacheRoot')
Assert-True `
  -Condition ($protectedCacheResetIndex -ge 0 -and
    $updateSourceVerificationIndex -gt $protectedCacheResetIndex -and
    $protectedCacheCleanupIndex -gt $updateSourceVerificationIndex) `
  -Message "Protected update cache must be boundary-reset before source verification and safely removed afterward."
$protectedContextFunctions = @($buildAst.FindAll({
      param($node)
      $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Assert-ProtectedVendorUpdateContext'
    }, $true))
Assert-True `
  -Condition ($protectedContextFunctions.Count -eq 1) `
  -Message "Expected exactly one protected vendor-update context function."
$protectedContextSource = $protectedContextFunctions[0].Extent.Text
Assert-True `
  -Condition ($protectedContextSource.Contains(
      "'Ayumudayo/StickerFit/.github/workflows/update-ffmpeg-vendor.yml@refs/heads/main'"
    ) -and
    $protectedContextSource.Contains("`$env:GITHUB_SHA -cnotmatch '^[0-9a-f]{40}$'") -and
    ([regex]::Matches(
        $protectedContextSource,
        [regex]::Escape('[System.StringComparison]::Ordinal')
      )).Count -ge 7) `
  -Message "Protected vendor update must enforce exact ordinal workflow authority and lowercase commit identity."
foreach ($forbiddenAuthorityComparison in @(
    '$env:GITHUB_ACTIONS -ne',
    '$env:GITHUB_REF -ne',
    '$env:GITHUB_WORKFLOW_REF -notmatch',
    '$env:STICKERFIT_PROTECTED_FFMPEG_UPDATE -ne',
    '$env:GITHUB_EVENT_NAME -ne'
  )) {
  Assert-True `
    -Condition (-not $protectedContextSource.Contains($forbiddenAuthorityComparison)) `
    -Message "Protected vendor update retained a case-insensitive authority comparison: $forbiddenAuthorityComparison"
}
Assert-True `
  -Condition ($buildSource.Contains('[System.IO.FileAttributes]::ReparsePoint')) `
  -Message "Protected staging cleanup must reject reparse points."
$moduleSource = Get-Content -Raw -LiteralPath $modulePath
Assert-True `
  -Condition ($moduleSource.Contains('[System.IO.FileAttributes]::ReparsePoint')) `
  -Message "FFmpeg signature-verification cleanup must reject reparse points."
$sourceVerificationFunctions = @($moduleAst.FindAll({
      param($node)
      $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Invoke-FfmpegSourceVerification'
    }, $true))
Assert-True `
  -Condition ($sourceVerificationFunctions.Count -eq 1) `
  -Message "Expected exactly one FFmpeg source-verification orchestrator."
$sourceVerificationSource = $sourceVerificationFunctions[0].Extent.Text
$signatureValidationIndex = $sourceVerificationSource.IndexOf('Assert-FfmpegValidSignatureStatus')
$archiveHashIndex = $sourceVerificationSource.IndexOf('$computedSha256 = Get-FfmpegVerifiedArchiveSha256')
Assert-True `
  -Condition ($signatureValidationIndex -ge 0 -and
    $archiveHashIndex -gt $signatureValidationIndex) `
  -Message "Production source verification must validate the official signature before hashing the archive."
foreach ($treeContract in @(
    [pscustomobject]@{
      Ast = $buildAst
      FunctionName = 'Assert-NoReparsePointInProtectedTree'
      Source = $buildSource
      MinimumReferences = 3
    },
    [pscustomobject]@{
      Ast = $moduleAst
      FunctionName = 'Assert-NoReparsePointInFfmpegTree'
      Source = $moduleSource
      MinimumReferences = 2
    }
  )) {
  $treeFunctions = @($treeContract.Ast.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
          $node.Name -eq $treeContract.FunctionName
      }, $true))
  Assert-True `
    -Condition ($treeFunctions.Count -eq 1) `
    -Message "Expected exactly one $($treeContract.FunctionName) function."
  $treeSource = $treeFunctions[0].Extent.Text
  $childReparseCheckIndex = $treeSource.IndexOf('$child.Attributes -band [System.IO.FileAttributes]::ReparsePoint')
  $childEnqueueIndex = $treeSource.IndexOf('$pendingDirectories.Enqueue($child.FullName)')
  Assert-True `
    -Condition ($childReparseCheckIndex -ge 0 -and
      $childEnqueueIndex -gt $childReparseCheckIndex -and
      -not $treeSource.Contains('Get-ChildItem -Recurse')) `
    -Message "$($treeContract.FunctionName) must reject a child reparse point before enqueueing directories without recursive traversal."
  Assert-True `
    -Condition (([regex]::Matches(
          $treeContract.Source,
          [regex]::Escape($treeContract.FunctionName)
        )).Count -ge $treeContract.MinimumReferences) `
    -Message "$($treeContract.FunctionName) is not applied at every recursive-delete site."
}

Assert-True `
  -Condition ($moduleSource.Contains('must be a JSON boolean') -and
    $moduleSource.Contains('FFmpeg 8.1.2 provenance must use SOURCE_DATE_EPOCH 1781654400.') -and
    $moduleSource.Contains('Non-legacy FFmpeg provenance requires the manifest legacy-bootstrap policy to be false.')) `
  -Message "Vendor validation must retain strict boolean, epoch, and non-legacy manifest-policy gates."
$vendorArtifactFunctions = @($moduleAst.FindAll({
      param($node)
      $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Test-FfmpegVendorArtifacts'
    }, $true))
Assert-True `
  -Condition ($vendorArtifactFunctions.Count -eq 1) `
  -Message "Expected exactly one FFmpeg vendor-artifact validation function."
$vendorArtifactSource = $vendorArtifactFunctions[0].Extent.Text
$schemaValidationIndex = $vendorArtifactSource.IndexOf('Assert-FfmpegProvenanceSchema -Provenance $provenance')
$schemaCoercionIndex = $vendorArtifactSource.IndexOf('if ([int]$provenance.schemaVersion -ne 1)')
Assert-True `
  -Condition ($schemaValidationIndex -ge 0 -and
    $schemaCoercionIndex -gt $schemaValidationIndex) `
  -Message "Closed provenance schema validation must run before legacy semantic coercions."
$authorityBlock = [regex]::Match(
  $vendorArtifactSource,
  '(?s)\$authority = \$provenance\.build\.authority.*?throw "Release FFmpeg provenance is missing exact protected workflow authority\."\s*\}'
)
Assert-True `
  -Condition ($authorityBlock.Success -and
    ([regex]::Matches(
        $authorityBlock.Value,
        [regex]::Escape('[System.StringComparison]::Ordinal')
      )).Count -eq 6 -and
    $authorityBlock.Value.Contains('"$($authority.headSha)" -cnotmatch') -and
    -not $authorityBlock.Value.Contains(' -ne ') -and
    -not $authorityBlock.Value.Contains(' -notmatch ')) `
  -Message "Release provenance must validate every protected workflow authority field with exact ordinal equality."

$releaseSidecarPath = Join-Path $WorkspaceRoot 'tools\test-release-sidecar.ps1'
$releaseTokens = $null
$releaseParseErrors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
  $releaseSidecarPath,
  [ref]$releaseTokens,
  [ref]$releaseParseErrors
) | Out-Null
Assert-True `
  -Condition ($releaseParseErrors.Count -eq 0) `
  -Message "Release sidecar policy must parse before source-contract inspection."
$releaseSource = Get-Content -Raw -LiteralPath $releaseSidecarPath
foreach ($requiredReleaseContract in @(
    'Assert-ExactFileBytes',
    'Assert-NonLegacyTauriResourceContract',
    'Get-ReleaseFilesWithoutReparseTraversal',
    "'binaries/LICENSE-ffmpeg.txt' = 'LICENSE-ffmpeg.txt'",
    "'binaries/ffmpeg-provenance.json' = 'ffmpeg-provenance.json'",
    "Join-Path `$resolvedReleaseDirectory 'LICENSE-ffmpeg.txt'",
    "Join-Path `$resolvedReleaseDirectory 'ffmpeg-provenance.json'",
    "'libwinpthread-1.dll'",
    "'LICENSE-ffmpeg-BtbN.txt'",
    "'LICENSE-libwinpthread.txt'"
  )) {
  Assert-True `
    -Condition ($releaseSource.Contains($requiredReleaseContract)) `
    -Message "Release sidecar resource contract is missing: $requiredReleaseContract"
}

$currentVendor = Test-FfmpegVendorArtifacts `
  -WorkspaceRoot $WorkspaceRoot `
  -AllowLegacyBootstrap
if ($currentVendor.LegacyBootstrap) {
  Assert-Throws `
    -Action { Test-FfmpegVendorArtifacts -WorkspaceRoot $WorkspaceRoot | Out-Null } `
    -MessagePattern 'rejects legacy'
}
else {
  $releaseVendor = Test-FfmpegVendorArtifacts -WorkspaceRoot $WorkspaceRoot
  Assert-True `
    -Condition (-not $releaseVendor.LegacyBootstrap) `
    -Message "Approved non-legacy vendor verification must pass without the legacy compatibility switch."
}

Write-Host 'FFmpeg source/vendor contract checks passed.'
