[CmdletBinding()]
param(
  [switch]$SelfTest,
  [switch]$DetectOnly,
  [string]$Repository,
  [int]$PullRequestNumber,
  [string]$BaseSha,
  [string]$HeadSha,
  [string]$AuthorLogin,
  [string]$AuthorAssociation,
  [string]$HeadRepository,
  [string]$GitHubToken = $env:GITHUB_TOKEN,
  [string]$ResultPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:ApprovedVendorVersion = "8.1.2"
$script:ApprovedSourceDateEpoch = [long]1781654400
$script:ApprovedVendorWorkflowRef = "Ayumudayo/StickerFit/.github/workflows/update-ffmpeg-vendor.yml@refs/heads/main"
$script:VendorAuthorityPaths = @(
  ".gitattributes",
  ".github/workflows/update-ffmpeg-vendor.yml",
  "tools/ffmpeg/build-minimal-ffmpeg.ps1",
  "tools/ffmpeg/FfmpegSourceVerification.psm1",
  "tools/ffmpeg/ffmpeg-release-signing-key.asc",
  "tools/ffmpeg/ffmpeg-version.json",
  "src-tauri/tauri.conf.json"
)

$script:ProtectedExactPaths = @(
  ".gitattributes",
  ".github/CODEOWNERS",
  "tools/ffmpeg/ffmpeg-release-signing-key.asc",
  "tools/ffmpeg/ffmpeg-version.json",
  "tools/ffmpeg/FfmpegSourceVerification.psm1",
  "tools/ffmpeg/build-minimal-ffmpeg.ps1",
  "tools/bootstrap-build-windows.ps1",
  "tools/size-report.ps1",
  "tools/test-ffmpeg-source-contract.ps1",
  "tools/test-release-sidecar.ps1",
  "tools/test-tauri-policy.ps1",
  "tools/test-vendor-diff-policy.ps1",
  "src-tauri/tauri.conf.json",
  "src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe",
  "src-tauri/binaries/libwinpthread-1.dll",
  "src-tauri/binaries/LICENSE-ffmpeg-BtbN.txt",
  "src-tauri/binaries/LICENSE-ffmpeg.txt",
  "src-tauri/binaries/LICENSE-libwinpthread.txt",
  "src-tauri/binaries/ffmpeg-provenance.json"
)

$script:VendorArtifactPaths = @(
  "tools/ffmpeg/ffmpeg-version.json",
  "src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe",
  "src-tauri/binaries/libwinpthread-1.dll",
  "src-tauri/binaries/LICENSE-ffmpeg-BtbN.txt",
  "src-tauri/binaries/LICENSE-ffmpeg.txt",
  "src-tauri/binaries/LICENSE-libwinpthread.txt",
  "src-tauri/binaries/ffmpeg-provenance.json"
)

$script:ArtifactToRepositoryPath = [System.Collections.Generic.Dictionary[string, string]]::new(
  [System.StringComparer]::Ordinal
)
$script:ArtifactToRepositoryPath.Add(
  "ffmpeg-x86_64-pc-windows-msvc.exe",
  "src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe"
)
$script:ArtifactToRepositoryPath.Add("ffmpeg-version.json", "tools/ffmpeg/ffmpeg-version.json")
$script:ArtifactToRepositoryPath.Add(
  "ffmpeg-provenance.json",
  "src-tauri/binaries/ffmpeg-provenance.json"
)
$script:ArtifactToRepositoryPath.Add("LICENSE-ffmpeg.txt", "src-tauri/binaries/LICENSE-ffmpeg.txt")
$script:ArtifactToRepositoryPath.Add("tauri.conf.json", "src-tauri/tauri.conf.json")

$script:ArtifactEvidenceNames = @(
  "update-fragment.json",
  "SHA256SUMS.txt"
)

function Test-ProtectedPath {
  param([Parameter(Mandatory = $true)][string]$Path)

  if ($Path.StartsWith(".github/workflows/", [System.StringComparison]::OrdinalIgnoreCase)) {
    return $true
  }

  if ($Path.StartsWith("src-tauri/binaries/", [System.StringComparison]::OrdinalIgnoreCase)) {
    return $true
  }

  foreach ($candidate in $script:ProtectedExactPaths) {
    if ([string]::Equals($Path, $candidate, [System.StringComparison]::OrdinalIgnoreCase)) {
      return $true
    }
  }

  return $false
}

function Test-VendorArtifactPath {
  param([Parameter(Mandatory = $true)][string]$Path)

  if ($Path.StartsWith("src-tauri/binaries/", [System.StringComparison]::OrdinalIgnoreCase)) {
    return $true
  }

  foreach ($candidate in $script:VendorArtifactPaths) {
    if ([string]::Equals($Path, $candidate, [System.StringComparison]::OrdinalIgnoreCase)) {
      return $true
    }
  }

  return $false
}

function Test-AuthorityBlobIdentity {
  param(
    [Parameter(Mandatory = $true)][System.Collections.IDictionary]$BaseTree,
    [Parameter(Mandatory = $true)][System.Collections.IDictionary]$RunTree
  )

  foreach ($path in $script:VendorAuthorityPaths) {
    if (-not $BaseTree.ContainsKey($path) -or -not $RunTree.ContainsKey($path)) {
      return [pscustomobject]@{ Approved = $false; Reason = "missing-authority-blob:$path" }
    }
    if (-not [string]::Equals(
        [string]$BaseTree[$path],
        [string]$RunTree[$path],
        [System.StringComparison]::Ordinal
      )) {
      return [pscustomobject]@{ Approved = $false; Reason = "stale-authority-blob:$path" }
    }
  }

  return [pscustomobject]@{ Approved = $true; Reason = "authority-blobs-identical" }
}

function Test-UpdatePreconditions {
  param(
    [Parameter(Mandatory = $true)]$Preconditions,
    [Parameter(Mandatory = $true)]$BaseManifest,
    [Parameter(Mandatory = $true)]$ReplacementManifest
  )

  if ($Preconditions.currentVersion -isnot [string] -or
      $Preconditions.currentSourceSha256 -isnot [string] -or
      $Preconditions.replacementVersion -isnot [string] -or
      $Preconditions.replacementSourceSha256 -isnot [string]) {
    return [pscustomobject]@{ Approved = $false; Reason = "preconditions-must-be-strings" }
  }
  if (-not [string]::Equals([string]$Preconditions.currentVersion, [string]$BaseManifest.version, [System.StringComparison]::Ordinal) -or
      -not [string]::Equals([string]$Preconditions.currentSourceSha256, [string]$BaseManifest.sourceArchiveSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
    return [pscustomobject]@{ Approved = $false; Reason = "stale-current-vendor-preconditions" }
  }
  if (-not [string]::Equals([string]$Preconditions.replacementVersion, [string]$ReplacementManifest.version, [System.StringComparison]::Ordinal) -or
      -not [string]::Equals([string]$Preconditions.replacementSourceSha256, [string]$ReplacementManifest.sourceArchiveSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
    return [pscustomobject]@{ Approved = $false; Reason = "replacement-source-mismatch" }
  }

  return [pscustomobject]@{ Approved = $true; Reason = "current-and-replacement-preconditions-match" }
}

function Test-VendorPolicyDecision {
  param(
    [Parameter(Mandatory = $true)][bool]$ProtectedChanged,
    [Parameter(Mandatory = $true)][bool]$SameRepository,
    [Parameter(Mandatory = $true)][string]$Association,
    [Parameter(Mandatory = $true)][bool]$VendorArtifactChanged,
    [Parameter(Mandatory = $true)][bool]$RunPresent,
    [Parameter(Mandatory = $true)][bool]$RunSucceeded,
    [Parameter(Mandatory = $true)][bool]$RunTrusted,
    [Parameter(Mandatory = $true)][bool]$ArtifactMatches
  )

  if (-not $ProtectedChanged) {
    return [pscustomobject]@{ Approved = $true; Reason = "protected-files-byte-identical" }
  }
  if (-not $SameRepository) {
    return [pscustomobject]@{ Approved = $false; Reason = "protected-change-from-fork" }
  }
  if (-not [string]::Equals($Association, "OWNER", [System.StringComparison]::Ordinal) -and
      -not [string]::Equals($Association, "MEMBER", [System.StringComparison]::Ordinal)) {
    return [pscustomobject]@{ Approved = $false; Reason = "untrusted-author-association" }
  }
  if (-not $VendorArtifactChanged) {
    return [pscustomobject]@{ Approved = $true; Reason = "authority-change-approved-by-environment" }
  }
  if (-not $RunPresent) {
    return [pscustomobject]@{ Approved = $false; Reason = "missing-vendor-update-run" }
  }
  if (-not $RunSucceeded) {
    return [pscustomobject]@{ Approved = $false; Reason = "vendor-update-run-not-successful" }
  }
  if (-not $RunTrusted) {
    return [pscustomobject]@{ Approved = $false; Reason = "untrusted-vendor-update-run" }
  }
  if (-not $ArtifactMatches) {
    return [pscustomobject]@{ Approved = $false; Reason = "vendor-artifact-mismatch" }
  }

  return [pscustomobject]@{ Approved = $true; Reason = "approved-vendor-artifact-match" }
}

function Invoke-SelfTest {
  $fixtures = @(
    @{ Name = "no-change"; Expected = $true; Args = @($false, $true, "NONE", $false, $false, $false, $false, $false) },
    @{ Name = "fork"; Expected = $false; Args = @($true, $false, "OWNER", $false, $false, $false, $false, $false) },
    @{ Name = "untrusted-association"; Expected = $false; Args = @($true, $true, "COLLABORATOR", $false, $false, $false, $false, $false) },
    @{ Name = "missing-run"; Expected = $false; Args = @($true, $true, "OWNER", $true, $false, $false, $false, $false) },
    @{ Name = "failed-run"; Expected = $false; Args = @($true, $true, "MEMBER", $true, $true, $false, $true, $true) },
    @{ Name = "untrusted-run"; Expected = $false; Args = @($true, $true, "OWNER", $true, $true, $true, $false, $true) },
    @{ Name = "hash-mismatch"; Expected = $false; Args = @($true, $true, "OWNER", $true, $true, $true, $true, $false) },
    @{ Name = "approved-authority"; Expected = $true; Args = @($true, $true, "MEMBER", $false, $false, $false, $false, $false) },
    @{ Name = "approved-match"; Expected = $true; Args = @($true, $true, "OWNER", $true, $true, $true, $true, $true) }
  )

  foreach ($fixture in $fixtures) {
    $args = $fixture.Args
    $actual = Test-VendorPolicyDecision `
      -ProtectedChanged $args[0] `
      -SameRepository $args[1] `
      -Association $args[2] `
      -VendorArtifactChanged $args[3] `
      -RunPresent $args[4] `
      -RunSucceeded $args[5] `
      -RunTrusted $args[6] `
      -ArtifactMatches $args[7]
    if ($actual.Approved -ne $fixture.Expected) {
      throw "Vendor policy fixture '$($fixture.Name)' failed: $($actual.Reason)"
    }
  }

  $expectedAuthorityPaths = @(
    ".gitattributes",
    ".github/workflows/update-ffmpeg-vendor.yml",
    "tools/ffmpeg/build-minimal-ffmpeg.ps1",
    "tools/ffmpeg/FfmpegSourceVerification.psm1",
    "tools/ffmpeg/ffmpeg-release-signing-key.asc",
    "tools/ffmpeg/ffmpeg-version.json",
    "src-tauri/tauri.conf.json"
  )
  if ($script:VendorAuthorityPaths.Count -ne $expectedAuthorityPaths.Count) {
    throw "Canonical vendor authority membership changed."
  }
  if (-not (Test-ProtectedPath -Path "src-tauri/tauri.conf.json")) {
    throw "Tauri configuration must remain a protected authority path."
  }
  $futureBinaryPath = "src-tauri/binaries/future/nested-runtime.dll"
  if (-not (Test-ProtectedPath -Path $futureBinaryPath) -or
      -not (Test-VendorArtifactPath -Path $futureBinaryPath)) {
    throw "Future binaries-subtree paths must remain protected vendor artifacts."
  }
  $authoritySet = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
  for ($index = 0; $index -lt $expectedAuthorityPaths.Count; $index++) {
    $path = $expectedAuthorityPaths[$index]
    if (-not [string]::Equals($script:VendorAuthorityPaths[$index], $path, [System.StringComparison]::Ordinal) -or
        -not $authoritySet.Add($path) -or
        -not (Test-ProtectedPath -Path $path)) {
      throw "Canonical vendor authority definition is incomplete or duplicated."
    }
  }
  if ($script:ApprovedVendorVersion -ne "8.1.2" -or
      $script:ApprovedSourceDateEpoch -ne 1781654400 -or
      -not [string]::Equals(
        $script:ApprovedVendorWorkflowRef,
        "Ayumudayo/StickerFit/.github/workflows/update-ffmpeg-vendor.yml@refs/heads/main",
        [System.StringComparison]::Ordinal
      )) {
    throw "Protected vendor version/epoch/workflow authority contract changed."
  }

  $baseAuthority = [System.Collections.Generic.Dictionary[string, string]]::new(
    [System.StringComparer]::Ordinal
  )
  foreach ($path in $expectedAuthorityPaths) { $baseAuthority[$path] = ("a" * 40) }
  $sameAuthority = [System.Collections.Generic.Dictionary[string, string]]::new(
    $baseAuthority,
    [System.StringComparer]::Ordinal
  )
  if (-not (Test-AuthorityBlobIdentity -BaseTree $baseAuthority -RunTree $sameAuthority).Approved) {
    throw "Identical authority blobs were rejected."
  }
  $staleAuthority = [System.Collections.Generic.Dictionary[string, string]]::new(
    $baseAuthority,
    [System.StringComparer]::Ordinal
  )
  $staleAuthority[$expectedAuthorityPaths[0]] = ("b" * 40)
  if ((Test-AuthorityBlobIdentity -BaseTree $baseAuthority -RunTree $staleAuthority).Approved) {
    throw "Stale authority blobs were accepted."
  }
  $missingAuthority = [System.Collections.Generic.Dictionary[string, string]]::new(
    $baseAuthority,
    [System.StringComparer]::Ordinal
  )
  $missingAuthority.Remove($expectedAuthorityPaths[1])
  if ((Test-AuthorityBlobIdentity -BaseTree $baseAuthority -RunTree $missingAuthority).Approved) {
    throw "Missing authority blobs were accepted."
  }
  $caseVariantAuthority = [System.Collections.Generic.Dictionary[string, string]]::new(
    $baseAuthority,
    [System.StringComparer]::Ordinal
  )
  $caseVariantAuthority.Remove($expectedAuthorityPaths[0])
  $caseVariantAuthority.Add($expectedAuthorityPaths[0].ToUpperInvariant(), ("a" * 40))
  if ((Test-AuthorityBlobIdentity -BaseTree $baseAuthority -RunTree $caseVariantAuthority).Approved) {
    throw "A case-variant authority path was accepted as canonical."
  }

  $treeRecords = [System.Collections.Generic.Dictionary[string, string]]::new(
    [System.StringComparer]::Ordinal
  )
  $caseFoldedTreePaths = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::OrdinalIgnoreCase)
  Add-ProtectedGitTreeEntry `
    -Result $treeRecords `
    -CaseFoldedPaths $caseFoldedTreePaths `
    -Entry ([pscustomobject]@{
      path = ".gitattributes"
      type = "blob"
      mode = "100644"
      sha = ("a" * 40)
    })
  $caseCollisionRejected = $false
  try {
    Add-ProtectedGitTreeEntry `
      -Result $treeRecords `
      -CaseFoldedPaths $caseFoldedTreePaths `
      -Entry ([pscustomobject]@{
        path = ".GITATTRIBUTES"
        type = "blob"
        mode = "100644"
        sha = ("b" * 40)
      })
  }
  catch {
    $caseCollisionRejected = $_.Exception.Message -like "*case-colliding*"
  }
  if (-not $caseCollisionRejected) {
    throw "A canonical plus case-variant protected path collision was accepted."
  }

  $baseManifestFixture = [pscustomobject]@{
    version = "7.1.1"
    sourceArchiveSha256 = ("1" * 64)
  }
  $replacementManifestFixture = [pscustomobject]@{
    version = "8.1.2"
    sourceArchiveSha256 = ("2" * 64)
  }
  $matchingPreconditions = [pscustomobject]@{
    currentVersion = $baseManifestFixture.version
    currentSourceSha256 = $baseManifestFixture.sourceArchiveSha256
    replacementVersion = $replacementManifestFixture.version
    replacementSourceSha256 = $replacementManifestFixture.sourceArchiveSha256
  }
  if (-not (Test-UpdatePreconditions `
      -Preconditions $matchingPreconditions `
      -BaseManifest $baseManifestFixture `
      -ReplacementManifest $replacementManifestFixture).Approved) {
    throw "Matching update preconditions were rejected."
  }
  $staleVersionPreconditions = [pscustomobject]@{
    currentVersion = "7.0"
    currentSourceSha256 = $baseManifestFixture.sourceArchiveSha256
    replacementVersion = $replacementManifestFixture.version
    replacementSourceSha256 = $replacementManifestFixture.sourceArchiveSha256
  }
  if ((Test-UpdatePreconditions `
      -Preconditions $staleVersionPreconditions `
      -BaseManifest $baseManifestFixture `
      -ReplacementManifest $replacementManifestFixture).Approved) {
    throw "Stale current-version preconditions were accepted."
  }
  $staleSourcePreconditions = [pscustomobject]@{
    currentVersion = $baseManifestFixture.version
    currentSourceSha256 = ("3" * 64)
    replacementVersion = $replacementManifestFixture.version
    replacementSourceSha256 = $replacementManifestFixture.sourceArchiveSha256
  }
  if ((Test-UpdatePreconditions `
      -Preconditions $staleSourcePreconditions `
      -BaseManifest $baseManifestFixture `
      -ReplacementManifest $replacementManifestFixture).Approved) {
    throw "Stale current-source preconditions were accepted."
  }
  $replacementMismatchPreconditions = [pscustomobject]@{
    currentVersion = $baseManifestFixture.version
    currentSourceSha256 = $baseManifestFixture.sourceArchiveSha256
    replacementVersion = "8.1.1"
    replacementSourceSha256 = $replacementManifestFixture.sourceArchiveSha256
  }
  if ((Test-UpdatePreconditions `
      -Preconditions $replacementMismatchPreconditions `
      -BaseManifest $baseManifestFixture `
      -ReplacementManifest $replacementManifestFixture).Approved) {
    throw "Mismatched replacement preconditions were accepted."
  }

  Write-Host "Vendor policy source-only fixtures passed: $($fixtures.Count) decisions plus authority/version/precondition contracts"
}

function Assert-OnlineParameters {
  $required = [ordered]@{
    Repository = $Repository
    PullRequestNumber = $PullRequestNumber
    BaseSha = $BaseSha
    HeadSha = $HeadSha
    AuthorLogin = $AuthorLogin
    AuthorAssociation = $AuthorAssociation
    HeadRepository = $HeadRepository
    GitHubToken = $GitHubToken
  }

  foreach ($entry in $required.GetEnumerator()) {
    if ($entry.Key -eq "PullRequestNumber") {
      if ([int]$entry.Value -le 0) {
        throw "PullRequestNumber must be positive."
      }
      continue
    }
    if ([string]::IsNullOrWhiteSpace([string]$entry.Value)) {
      throw "$($entry.Key) is required outside -SelfTest."
    }
  }

  foreach ($sha in @($BaseSha, $HeadSha)) {
    if ($sha -notmatch "\A[0-9a-fA-F]{40}\z") {
      throw "BaseSha and HeadSha must be exact 40-hex commit IDs."
    }
  }
}

function New-GitHubHeaders {
  return @{
    Accept = "application/vnd.github+json"
    Authorization = "Bearer $GitHubToken"
    "X-GitHub-Api-Version" = "2022-11-28"
    "User-Agent" = "StickerFit-vendor-policy"
  }
}

function Invoke-GitHubJson {
  param([Parameter(Mandatory = $true)][string]$Uri)

  return Invoke-RestMethod -Uri $Uri -Headers (New-GitHubHeaders) -Method Get
}

function Get-GitHubRunArtifacts {
  param([Parameter(Mandatory = $true)][string]$RunId)

  $artifacts = New-Object "System.Collections.Generic.List[object]"
  [long]$reportedTotal = -1
  $page = 1
  while ($true) {
    $response = Invoke-GitHubJson -Uri "https://api.github.com/repos/$Repository/actions/runs/$RunId/artifacts?per_page=100&page=$page"
    if ($null -eq $response.total_count -or [long]$response.total_count -lt 0) {
      throw "GitHub artifact pagination did not report a valid total_count."
    }
    if ($reportedTotal -lt 0) {
      $reportedTotal = [long]$response.total_count
    }
    elseif ($reportedTotal -ne [long]$response.total_count) {
      throw "GitHub artifact total_count changed during pagination."
    }

    $pageArtifacts = @($response.artifacts)
    foreach ($artifact in $pageArtifacts) { $artifacts.Add($artifact) }
    if ($pageArtifacts.Count -lt 100) { break }
    $page++
    if ($page -gt 1000) { throw "GitHub artifact pagination exceeded the policy limit." }
  }

  if ([long]$artifacts.Count -ne $reportedTotal) {
    throw "GitHub artifact pagination was incomplete: expected $reportedTotal, received $($artifacts.Count)."
  }
  return @($artifacts)
}

function Add-ProtectedGitTreeEntry {
  param(
    [Parameter(Mandatory = $true)][System.Collections.IDictionary]$Result,
    [Parameter(Mandatory = $true)][System.Collections.Generic.HashSet[string]]$CaseFoldedPaths,
    [Parameter(Mandatory = $true)]$Entry
  )

  $path = [string]$Entry.path
  if (-not (Test-ProtectedPath -Path $path)) {
    return
  }
  if ([string]::Equals([string]$Entry.type, "tree", [System.StringComparison]::Ordinal)) {
    return
  }
  if (-not [string]::Equals([string]$Entry.type, "blob", [System.StringComparison]::Ordinal) -or
      -not [string]::Equals([string]$Entry.mode, "100644", [System.StringComparison]::Ordinal)) {
    throw "Protected path '$path' is not a regular non-executable Git blob."
  }
  if (-not $CaseFoldedPaths.Add($path)) {
    throw "GitHub returned case-colliding protected tree paths at '$path'."
  }
  if ($Result.ContainsKey($path)) {
    throw "GitHub returned a duplicate protected tree path: '$path'."
  }
  $Result.Add($path, [string]$Entry.sha)
}

function Get-GitHubTree {
  param([Parameter(Mandatory = $true)][string]$CommitSha)

  $commit = Invoke-GitHubJson -Uri "https://api.github.com/repos/$Repository/git/commits/$CommitSha"
  $tree = Invoke-GitHubJson -Uri "https://api.github.com/repos/$Repository/git/trees/$($commit.tree.sha)?recursive=1"
  if ($tree.truncated) {
    throw "GitHub returned a truncated repository tree; protected byte comparison is incomplete."
  }

  $result = [System.Collections.Generic.Dictionary[string, string]]::new(
    [System.StringComparer]::Ordinal
  )
  $caseFoldedPaths = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::OrdinalIgnoreCase)
  foreach ($entry in @($tree.tree)) {
    Add-ProtectedGitTreeEntry `
      -Result $result `
      -CaseFoldedPaths $caseFoldedPaths `
      -Entry $entry
  }
  return $result
}

function Get-ChangedFileMetadata {
  $changed = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
  $page = 1
  while ($true) {
    $items = @(Invoke-GitHubJson -Uri "https://api.github.com/repos/$Repository/pulls/$PullRequestNumber/files?per_page=100&page=$page")
    foreach ($item in $items) {
      [void]$changed.Add([string]$item.filename)
      if ($item.PSObject.Properties.Name -contains "previous_filename" -and -not [string]::IsNullOrWhiteSpace([string]$item.previous_filename)) {
        [void]$changed.Add([string]$item.previous_filename)
      }
    }
    if ($items.Count -lt 100) {
      break
    }
    $page++
  }
  return $changed
}

function Get-ProtectedComparison {
  $baseTree = Get-GitHubTree -CommitSha $BaseSha
  $headTree = Get-GitHubTree -CommitSha $HeadSha
  $metadata = Get-ChangedFileMetadata
  $allPaths = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)

  foreach ($path in $baseTree.Keys) { [void]$allPaths.Add([string]$path) }
  foreach ($path in $headTree.Keys) { [void]$allPaths.Add([string]$path) }
  foreach ($path in $metadata) {
    if (Test-ProtectedPath -Path $path) { [void]$allPaths.Add($path) }
  }

  $changed = New-Object "System.Collections.Generic.List[string]"
  foreach ($path in $allPaths) {
    $baseBlob = if ($baseTree.ContainsKey($path)) { [string]$baseTree[$path] } else { "<missing>" }
    $headBlob = if ($headTree.ContainsKey($path)) { [string]$headTree[$path] } else { "<missing>" }
    if (-not [string]::Equals($baseBlob, $headBlob, [System.StringComparison]::Ordinal)) {
      $changed.Add($path)
    }
  }

  return [pscustomobject]@{
    Changed = @($changed | Sort-Object)
    BaseTree = $baseTree
    HeadTree = $headTree
  }
}

function Write-Classification {
  param([Parameter(Mandatory = $true)]$Comparison)

  $protectedChanged = @($Comparison.Changed).Count -gt 0
  $vendorChanged = @($Comparison.Changed | Where-Object { Test-VendorArtifactPath -Path $_ }).Count -gt 0
  $classification = [ordered]@{
    protectedChanged = $protectedChanged
    vendorArtifactChanged = $vendorChanged
    protectedPaths = @($Comparison.Changed)
  }
  $json = $classification | ConvertTo-Json -Depth 5
  if (-not [string]::IsNullOrWhiteSpace($ResultPath)) {
    $parent = Split-Path -Parent $ResultPath
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
      [void](New-Item -ItemType Directory -Force -Path $parent)
    }
    Set-Content -LiteralPath $ResultPath -Value $json -Encoding UTF8
  }
  Write-Host $json
  return [pscustomobject]$classification
}

function ConvertTo-GitHubContentPath {
  param([Parameter(Mandatory = $true)][string]$Path)

  return (($Path -split "/" | ForEach-Object { [System.Uri]::EscapeDataString($_) }) -join "/")
}

function Save-GitHubFile {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Ref,
    [Parameter(Mandatory = $true)][string]$Destination
  )

  $encodedPath = ConvertTo-GitHubContentPath -Path $Path
  $encodedRef = [System.Uri]::EscapeDataString($Ref)
  $uri = "https://api.github.com/repos/{0}/contents/{1}?ref={2}" -f $Repository, $encodedPath, $encodedRef
  $headers = New-GitHubHeaders
  $headers.Accept = "application/vnd.github.raw+json"
  Invoke-WebRequest -Uri $uri -Headers $headers -Method Get -OutFile $Destination -UseBasicParsing
}

function Get-Sha256Hex {
  param([Parameter(Mandatory = $true)][string]$Path)

  return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Save-GitHubArtifactArchive {
  param(
    [Parameter(Mandatory = $true)][string]$Uri,
    [Parameter(Mandatory = $true)][string]$Destination
  )

  $apiUri = $null
  if (-not [System.Uri]::TryCreate($Uri, [System.UriKind]::Absolute, [ref]$apiUri) -or
      -not [string]::Equals($apiUri.Scheme, "https", [System.StringComparison]::OrdinalIgnoreCase) -or
      -not [string]::Equals($apiUri.Host, "api.github.com", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "GitHub artifact API URI must be an absolute api.github.com HTTPS URL."
  }

  Add-Type -AssemblyName System.Net.Http
  $apiHandler = [System.Net.Http.HttpClientHandler]::new()
  $apiHandler.AllowAutoRedirect = $false
  $apiClient = [System.Net.Http.HttpClient]::new($apiHandler)
  try {
    $request = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Get, $apiUri)
    $request.Headers.Authorization = [System.Net.Http.Headers.AuthenticationHeaderValue]::new("Bearer", $GitHubToken)
    $request.Headers.Accept.ParseAdd("application/vnd.github+json")
    $request.Headers.Add("X-GitHub-Api-Version", "2022-11-28")
    $request.Headers.UserAgent.ParseAdd("StickerFit-vendor-policy")
    try {
      $response = $apiClient.SendAsync($request).GetAwaiter().GetResult()
      try {
        if ([int]$response.StatusCode -notin @(301, 302, 303, 307, 308) -or $null -eq $response.Headers.Location) {
          throw "GitHub artifact endpoint did not return a signed download redirect (HTTP $([int]$response.StatusCode))."
        }
        $signedUri = $response.Headers.Location
        if (-not $signedUri.IsAbsoluteUri) {
          $signedUri = [System.Uri]::new($apiUri, $signedUri)
        }
        if (-not [string]::Equals($signedUri.Scheme, "https", [System.StringComparison]::OrdinalIgnoreCase)) {
          throw "GitHub artifact redirect did not use HTTPS."
        }
      }
      finally {
        $response.Dispose()
      }
    }
    finally {
      $request.Dispose()
    }
  }
  finally {
    $apiClient.Dispose()
    $apiHandler.Dispose()
  }

  $downloadHandler = [System.Net.Http.HttpClientHandler]::new()
  $downloadHandler.AllowAutoRedirect = $true
  $downloadClient = [System.Net.Http.HttpClient]::new($downloadHandler)
  try {
    $downloadResponse = $downloadClient.GetAsync($signedUri).GetAwaiter().GetResult()
    try {
      $finalUri = $downloadResponse.RequestMessage.RequestUri
      if ($null -eq $finalUri -or
          -not [string]::Equals($finalUri.Scheme, "https", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Signed vendor artifact download ended on a non-HTTPS URI."
      }
      if (-not $downloadResponse.IsSuccessStatusCode -or
          $null -eq $downloadResponse.Content.Headers.ContentLength -or
          [long]$downloadResponse.Content.Headers.ContentLength -le 0 -or
          [long]$downloadResponse.Content.Headers.ContentLength -gt 32MB) {
        throw "Signed vendor artifact download returned an invalid response."
      }
      $bytes = $downloadResponse.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
      if ($bytes.Length -ne [long]$downloadResponse.Content.Headers.ContentLength) {
        throw "Signed vendor artifact download length changed in transit."
      }
      [System.IO.File]::WriteAllBytes($Destination, $bytes)
    }
    finally {
      $downloadResponse.Dispose()
    }
  }
  finally {
    $downloadClient.Dispose()
    $downloadHandler.Dispose()
  }
}

function Assert-RunTrust {
  param(
    [Parameter(Mandatory = $true)]$Run,
    [Parameter(Mandatory = $true)][string]$RunId
  )

  if (-not [string]::Equals([string]$Run.repository.full_name, "Ayumudayo/StickerFit", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Vendor update run $RunId belongs to an unexpected repository."
  }
  if (-not [string]::Equals([string]$Run.event, "workflow_dispatch", [System.StringComparison]::Ordinal)) {
    throw "Vendor update run $RunId was not workflow_dispatch."
  }
  if (-not [string]::Equals([string]$Run.head_branch, "main", [System.StringComparison]::Ordinal)) {
    throw "Vendor update run $RunId did not run on main."
  }
  if (-not [string]::Equals([string]$Run.conclusion, "success", [System.StringComparison]::Ordinal)) {
    throw "Vendor update run $RunId did not conclude successfully."
  }
  if ([string]$Run.head_sha -notmatch "\A[0-9a-fA-F]{40}\z") {
    throw "Vendor update run $RunId has an invalid head SHA."
  }

  $workflow = Invoke-GitHubJson -Uri "https://api.github.com/repos/$Repository/actions/workflows/$($Run.workflow_id)"
  $expectedWorkflow = ".github/workflows/update-ffmpeg-vendor.yml"
  if (-not [string]::Equals([string]$Run.path, $expectedWorkflow, [System.StringComparison]::Ordinal) -or
      -not [string]::Equals([string]$workflow.path, $expectedWorkflow, [System.StringComparison]::Ordinal)) {
    throw "Vendor update run $RunId did not use the protected update workflow."
  }

  $comparison = Invoke-GitHubJson -Uri "https://api.github.com/repos/$Repository/compare/$($Run.head_sha)...$BaseSha"
  if ((-not [string]::Equals([string]$comparison.status, "ahead", [System.StringComparison]::Ordinal) -and
       -not [string]::Equals([string]$comparison.status, "identical", [System.StringComparison]::Ordinal)) -or
      -not [string]::Equals([string]$comparison.merge_base_commit.sha, [string]$Run.head_sha, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Vendor update run head SHA is not an ancestor of the PR base SHA."
  }

  $runAuthorityTree = Get-GitHubTree -CommitSha ([string]$Run.head_sha)
  $baseAuthorityTree = Get-GitHubTree -CommitSha $BaseSha
  $authorityDecision = Test-AuthorityBlobIdentity -BaseTree $baseAuthorityTree -RunTree $runAuthorityTree
  if (-not $authorityDecision.Approved) {
    throw "Vendor update run used stale or incomplete authority: $($authorityDecision.Reason)."
  }
}

function Expand-ValidatedArtifact {
  param(
    [Parameter(Mandatory = $true)][string]$ArchivePath,
    [Parameter(Mandatory = $true)][string]$Destination
  )

  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
  try {
    $expectedNames = @($script:ArtifactToRepositoryPath.Keys) + $script:ArtifactEvidenceNames
    $expectedNameSet = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
    foreach ($expectedName in $expectedNames) {
      if (-not $expectedNameSet.Add([string]$expectedName)) {
        throw "Vendor artifact policy contains a duplicate expected entry: '$expectedName'."
      }
    }
    $seen = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
    [long]$expandedBytes = 0
    foreach ($entry in $archive.Entries) {
      $name = [string]$entry.FullName
      if ($name.Contains("/") -or $name.Contains("\") -or -not $expectedNameSet.Contains($name) -or
          $entry.Length -le 0 -or $entry.Length -gt 32MB) {
        throw "Unexpected or unsafe vendor artifact entry: '$name'."
      }
      $expandedBytes += [long]$entry.Length
      if ($expandedBytes -gt 64MB) {
        throw "Vendor artifact expands beyond the policy limit."
      }
      if (-not $seen.Add($name)) {
        throw "Duplicate vendor artifact entry: '$name'."
      }
      $destinationPath = Join-Path $Destination $name
      [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $destinationPath, $false)
    }
    foreach ($name in $expectedNames) {
      if (-not $seen.Contains($name)) {
        throw "Required vendor artifact entry is missing: '$name'."
      }
    }
  }
  finally {
    $archive.Dispose()
  }
}

function Assert-ArtifactChecksums {
  param([Parameter(Mandatory = $true)][string]$ExpandedRoot)

  $checksumPath = Join-Path $ExpandedRoot "SHA256SUMS.txt"
  $expectedFiles = @($script:ArtifactToRepositoryPath.Keys) + @("update-fragment.json")
  $actual = [System.Collections.Generic.Dictionary[string, string]]::new(
    [System.StringComparer]::Ordinal
  )
  foreach ($line in @(Get-Content -LiteralPath $checksumPath)) {
    if ($line -notmatch "\A([0-9a-f]{64})  ([A-Za-z0-9._-]+)\z") {
      throw "SHA256SUMS.txt contains an invalid line."
    }
    if ($actual.ContainsKey($Matches[2])) {
      throw "SHA256SUMS.txt contains a duplicate entry for '$($Matches[2])'."
    }
    $actual[$Matches[2]] = $Matches[1]
  }
  if ($actual.Count -ne $expectedFiles.Count) {
    throw "SHA256SUMS.txt does not have the exact expected membership."
  }
  foreach ($name in $expectedFiles) {
    if (-not $actual.ContainsKey($name)) {
      throw "SHA256SUMS.txt is missing '$name'."
    }
    $path = Join-Path $ExpandedRoot $name
    if (-not [string]::Equals([string]$actual[$name], (Get-Sha256Hex -Path $path), [System.StringComparison]::Ordinal)) {
      throw "SHA256SUMS.txt does not match '$name'."
    }
  }
}

function Assert-NonLegacyTauriBundleContract {
  param([Parameter(Mandatory = $true)][string]$ConfigPath)

  $config = Get-Content -Raw -LiteralPath $ConfigPath | ConvertFrom-Json
  $expectedResources = [ordered]@{
    "binaries/LICENSE-ffmpeg.txt" = "LICENSE-ffmpeg.txt"
    "binaries/ffmpeg-provenance.json" = "ffmpeg-provenance.json"
  }
  if ($null -eq $config.bundle.resources -or
      $config.bundle.resources -is [string] -or
      $config.bundle.resources -is [System.Array]) {
    throw "Protected vendor update Tauri resources must be a JSON object."
  }
  $actualResourceProperties = @($config.bundle.resources.PSObject.Properties | Where-Object {
      $_.MemberType -eq [System.Management.Automation.PSMemberTypes]::NoteProperty
    })
  if ($actualResourceProperties.Count -ne $expectedResources.Count) {
    throw "Protected vendor update must use the exact non-legacy Tauri resource map."
  }
  foreach ($entry in $expectedResources.GetEnumerator()) {
    $matchingProperties = @($actualResourceProperties | Where-Object {
        [string]::Equals([string]$_.Name, [string]$entry.Key, [System.StringComparison]::Ordinal)
      })
    if ($matchingProperties.Count -ne 1 -or
        $matchingProperties[0].Value -isnot [string] -or
        -not [string]::Equals([string]$matchingProperties[0].Value, [string]$entry.Value, [System.StringComparison]::Ordinal)) {
      throw "Protected vendor update has an unexpected Tauri resource mapping for '$($entry.Key)'."
    }
  }

  if ($config.bundle.externalBin -isnot [System.Array]) {
    throw "Protected vendor update externalBin must be a JSON array."
  }
  $externalBins = @($config.bundle.externalBin | ForEach-Object { [string]$_ })
  if ($externalBins.Count -ne 1 -or
      $config.bundle.externalBin[0] -isnot [string] -or
      -not [string]::Equals($externalBins[0], "binaries/ffmpeg", [System.StringComparison]::Ordinal)) {
    throw "Protected vendor update must retain the exact FFmpeg externalBin mapping."
  }
}

function Assert-UpdateFragment {
  param(
    [Parameter(Mandatory = $true)][string]$ExpandedRoot,
    [Parameter(Mandatory = $true)][string]$RunId,
    [Parameter(Mandatory = $true)][string]$RunHeadSha,
    [Parameter(Mandatory = $true)]$BaseManifest,
    [Parameter(Mandatory = $true)]$Manifest
  )

  $fragment = Get-Content -Raw -LiteralPath (Join-Path $ExpandedRoot "update-fragment.json") | ConvertFrom-Json
  Assert-NonLegacyTauriBundleContract -ConfigPath (Join-Path $ExpandedRoot "tauri.conf.json")
  if (($fragment.schemaVersion -isnot [int] -and $fragment.schemaVersion -isnot [long]) -or
      [long]$fragment.schemaVersion -ne 1 -or
      $fragment.applyOnlyAfterProtectedReview -isnot [bool] -or
      $fragment.applyOnlyAfterProtectedReview -ne $true) {
    throw "update-fragment.json is not an approval-gated schema v1 fragment."
  }
  if (($fragment.sourceWorkflow.runId -isnot [int] -and $fragment.sourceWorkflow.runId -isnot [long]) -or
      $fragment.sourceWorkflow.sourceCommit -isnot [string] -or
      $fragment.sourceWorkflow.workflowRef -isnot [string] -or
      [string]$fragment.sourceWorkflow.runId -ne $RunId -or
      -not [string]::Equals([string]$fragment.sourceWorkflow.sourceCommit, $RunHeadSha, [System.StringComparison]::OrdinalIgnoreCase) -or
      -not [string]::Equals([string]$fragment.sourceWorkflow.workflowRef, $script:ApprovedVendorWorkflowRef, [System.StringComparison]::Ordinal)) {
    throw "update-fragment.json does not identify the approved workflow run."
  }
  $preconditionDecision = Test-UpdatePreconditions `
    -Preconditions $fragment.preconditions `
    -BaseManifest $BaseManifest `
    -ReplacementManifest $Manifest
  if (-not $preconditionDecision.Approved) {
    throw "update-fragment.json preconditions failed: $($preconditionDecision.Reason)."
  }

  $expectedReplacements = [ordered]@{
    "ffmpeg-x86_64-pc-windows-msvc.exe" = "src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe"
    "LICENSE-ffmpeg.txt" = "src-tauri/binaries/LICENSE-ffmpeg.txt"
    "ffmpeg-provenance.json" = "src-tauri/binaries/ffmpeg-provenance.json"
    "ffmpeg-version.json" = "tools/ffmpeg/ffmpeg-version.json"
    "tauri.conf.json" = "src-tauri/tauri.conf.json"
  }
  if ($fragment.replacements -isnot [System.Array]) {
    throw "update-fragment.json replacements must be a JSON array."
  }
  $replacements = @($fragment.replacements)
  if ($replacements.Count -ne $expectedReplacements.Count) {
    throw "update-fragment.json has an unexpected replacement count."
  }
  $seenReplacements = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
  foreach ($replacement in $replacements) {
    if ($replacement.source -isnot [string] -or $replacement.destination -isnot [string]) {
      throw "update-fragment.json replacement source/destination must be strings."
    }
    $source = [string]$replacement.source
    $matchingSource = @($expectedReplacements.Keys | Where-Object {
        [string]::Equals([string]$_, $source, [System.StringComparison]::Ordinal)
      })
    if (-not $seenReplacements.Add($source) -or
        $matchingSource.Count -ne 1 -or
        -not [string]::Equals([string]$replacement.destination, [string]$expectedReplacements[$matchingSource[0]], [System.StringComparison]::Ordinal)) {
      throw "update-fragment.json contains an unexpected replacement mapping."
    }
    $stagedSourcePath = Join-Path $ExpandedRoot $source
    if (-not (Test-Path -LiteralPath $stagedSourcePath -PathType Leaf) -or
        $replacement.sha256 -isnot [string] -or
        ($replacement.bytes -isnot [int] -and $replacement.bytes -isnot [long]) -or
        -not [string]::Equals([string]$replacement.sha256, (Get-Sha256Hex -Path $stagedSourcePath), [System.StringComparison]::Ordinal) -or
        [long]$replacement.bytes -ne (Get-Item -LiteralPath $stagedSourcePath).Length) {
      throw "update-fragment.json replacement identity does not match staged source '$source'."
    }
    if ($source -eq "ffmpeg-x86_64-pc-windows-msvc.exe" -and
        -not [string]::Equals([string]$replacement.sha256, [string]$Manifest.expectedExeSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "update-fragment.json executable hash does not match the manifest."
    }
  }

  $expectedDeletions = @(
    "src-tauri/binaries/LICENSE-ffmpeg-BtbN.txt",
    "src-tauri/binaries/LICENSE-libwinpthread.txt",
    "src-tauri/binaries/libwinpthread-1.dll"
  ) | Sort-Object
  if ($fragment.deletions -isnot [System.Array] -or
      @($fragment.deletions | Where-Object { $_ -isnot [string] }).Count -ne 0) {
    throw "update-fragment.json deletions must be a JSON string array."
  }
  $actualDeletions = @($fragment.deletions | Sort-Object)
  if ($actualDeletions.Count -ne $expectedDeletions.Count -or
      $null -ne (Compare-Object -CaseSensitive -ReferenceObject $expectedDeletions -DifferenceObject $actualDeletions)) {
    throw "update-fragment.json has an unexpected deletion set."
  }
}

function Assert-VendorArtifact {
  param(
    [Parameter(Mandatory = $true)][string]$RunId,
    [Parameter(Mandatory = $true)]$Run,
    [Parameter(Mandatory = $true)]$HeadTree,
    [Parameter(Mandatory = $true)][string[]]$ChangedPaths
  )

  $requiredChangedPaths = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
  foreach ($repositoryPath in $script:ArtifactToRepositoryPath.Values) {
    if (-not [string]::Equals(
        [string]$repositoryPath,
        "src-tauri/binaries/LICENSE-ffmpeg.txt",
        [System.StringComparison]::Ordinal
      )) {
      [void]$requiredChangedPaths.Add([string]$repositoryPath)
    }
  }
  foreach ($legacyPath in @(
    "src-tauri/binaries/LICENSE-ffmpeg-BtbN.txt",
    "src-tauri/binaries/LICENSE-libwinpthread.txt",
    "src-tauri/binaries/libwinpthread-1.dll"
  )) {
    [void]$requiredChangedPaths.Add($legacyPath)
  }
  $allowedChangedPaths = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
  foreach ($path in $requiredChangedPaths) { [void]$allowedChangedPaths.Add($path) }
  [void]$allowedChangedPaths.Add("src-tauri/binaries/LICENSE-ffmpeg.txt")
  $actualChangedPaths = New-Object "System.Collections.Generic.HashSet[string]" ([System.StringComparer]::Ordinal)
  foreach ($path in $ChangedPaths) {
    if (-not $actualChangedPaths.Add([string]$path)) {
      throw "Protected comparison returned a duplicate changed path: '$path'."
    }
  }
  $missingRequiredPaths = @($requiredChangedPaths | Where-Object { -not $actualChangedPaths.Contains($_) })
  $unexpectedChangedPaths = @($actualChangedPaths | Where-Object { -not $allowedChangedPaths.Contains($_) })
  if ($missingRequiredPaths.Count -ne 0 -or $unexpectedChangedPaths.Count -ne 0) {
    throw "Protected vendor update changed paths outside the exact approved replacement/deletion set."
  }

  $temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("stickerfit-vendor-policy-" + [System.Guid]::NewGuid().ToString("N"))
  [void](New-Item -ItemType Directory -Path $temporaryRoot)
  try {
    $baseManifestPath = Join-Path $temporaryRoot "base-ffmpeg-version.json"
    $headManifestPath = Join-Path $temporaryRoot "head-ffmpeg-version.json"
    $headProvenancePath = Join-Path $temporaryRoot "head-ffmpeg-provenance.json"
    Save-GitHubFile -Path "tools/ffmpeg/ffmpeg-version.json" -Ref $BaseSha -Destination $baseManifestPath
    Save-GitHubFile -Path "tools/ffmpeg/ffmpeg-version.json" -Ref $HeadSha -Destination $headManifestPath
    Save-GitHubFile -Path "src-tauri/binaries/ffmpeg-provenance.json" -Ref $HeadSha -Destination $headProvenancePath
    $baseManifest = Get-Content -Raw -LiteralPath $baseManifestPath | ConvertFrom-Json
    $manifest = Get-Content -Raw -LiteralPath $headManifestPath | ConvertFrom-Json
    $provenance = Get-Content -Raw -LiteralPath $headProvenancePath | ConvertFrom-Json

    if (($manifest.vendorUpdateRunId -isnot [int] -and $manifest.vendorUpdateRunId -isnot [long]) -or
        ($provenance.vendorUpdateRunId -isnot [int] -and $provenance.vendorUpdateRunId -isnot [long]) -or
        [string]$manifest.vendorUpdateRunId -ne $RunId -or
        [string]$provenance.vendorUpdateRunId -ne $RunId) {
      throw "Manifest/provenance vendorUpdateRunId does not match the approved workflow run."
    }
    if ($provenance.legacyBootstrap -isnot [bool] -or $provenance.legacyBootstrap -ne $false) {
      throw "Protected vendor updates must replace legacy bootstrap provenance."
    }
    if (-not [string]::Equals([string]$manifest.version, $script:ApprovedVendorVersion, [System.StringComparison]::Ordinal) -or
        [string]$manifest.sourceArchiveSha256 -notmatch "\A[0-9a-fA-F]{64}\z" -or
        [string]$manifest.expectedExeSha256 -notmatch "\A[0-9a-fA-F]{64}\z") {
      throw "Head FFmpeg manifest is not the approved 8.1.2 update or contains an invalid hash."
    }
    if (-not [string]::Equals([string]$manifest.version, [string]$provenance.version, [System.StringComparison]::Ordinal) -or
        -not [string]::Equals([string]$manifest.sourceArchiveSha256, [string]$provenance.sourceArchiveSha256, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not [string]::Equals([string]$manifest.expectedExeSha256, [string]$provenance.exeSha256, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not [string]::Equals([string]$manifest.releaseKeyFingerprint, [string]$provenance.signingPrimaryFingerprint, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Head manifest and provenance fields do not match."
    }
    if ($null -eq $provenance.toolchainVersions -or
        ($provenance.sourceDateEpoch -isnot [int] -and $provenance.sourceDateEpoch -isnot [long]) -or
        [long]$provenance.sourceDateEpoch -ne $script:ApprovedSourceDateEpoch) {
      throw "Protected vendor provenance must include toolchain versions and SOURCE_DATE_EPOCH=1781654400."
    }

    foreach ($legacyPath in @(
      "src-tauri/binaries/libwinpthread-1.dll",
      "src-tauri/binaries/LICENSE-ffmpeg-BtbN.txt",
      "src-tauri/binaries/LICENSE-libwinpthread.txt"
    )) {
      if ($HeadTree.ContainsKey($legacyPath)) {
        throw "Protected vendor update still contains legacy runtime payload '$legacyPath'."
      }
    }
    foreach ($repositoryPath in $script:ArtifactToRepositoryPath.Values) {
      if (-not $HeadTree.ContainsKey([string]$repositoryPath)) {
        throw "Protected vendor update is missing '$repositoryPath'."
      }
    }
    $expectedVendorTreePaths = @(
      "src-tauri/binaries/README.md",
      "src-tauri/binaries/LICENSE-ffmpeg.txt",
      "src-tauri/binaries/ffmpeg-provenance.json",
      "src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe"
    ) | Sort-Object
    $actualVendorTreePaths = @($HeadTree.Keys | Where-Object {
        $_.StartsWith("src-tauri/binaries/", [System.StringComparison]::OrdinalIgnoreCase)
      } | Sort-Object)
    if ($actualVendorTreePaths.Count -ne $expectedVendorTreePaths.Count -or
        $null -ne (Compare-Object -CaseSensitive -ReferenceObject $expectedVendorTreePaths -DifferenceObject $actualVendorTreePaths)) {
      throw "Protected vendor update must leave the exact approved binaries subtree membership."
    }

    $artifactList = @(Get-GitHubRunArtifacts -RunId $RunId)
    $expectedArtifactName = "ffmpeg-vendor-$($manifest.version)-$RunId"
    $matches = @($artifactList | Where-Object {
      [string]::Equals([string]$_.name, $expectedArtifactName, [System.StringComparison]::Ordinal) -and -not $_.expired
    })
    if ($artifactList.Count -ne 1 -or $matches.Count -ne 1) {
      throw "Expected exactly one run artifact named '$expectedArtifactName'; found $($artifactList.Count) total and $($matches.Count) matching."
    }
    if ([long]$matches[0].size_in_bytes -le 0 -or [long]$matches[0].size_in_bytes -gt 32MB) {
      throw "Approved vendor artifact has an invalid compressed size."
    }
    if ([string]$matches[0].digest -notmatch "\Asha256:([0-9a-f]{64})\z") {
      throw "Approved vendor artifact does not expose a valid immutable SHA-256 digest."
    }
    $archiveDigest = $Matches[1]

    $archivePath = Join-Path $temporaryRoot "vendor.zip"
    Save-GitHubArtifactArchive -Uri ([string]$matches[0].archive_download_url) -Destination $archivePath
    if (-not [string]::Equals($archiveDigest, (Get-Sha256Hex -Path $archivePath), [System.StringComparison]::Ordinal)) {
      throw "Downloaded vendor artifact does not match the GitHub artifact digest."
    }
    $expanded = Join-Path $temporaryRoot "expanded"
    [void](New-Item -ItemType Directory -Path $expanded)
    Expand-ValidatedArtifact -ArchivePath $archivePath -Destination $expanded
    Assert-ArtifactChecksums -ExpandedRoot $expanded
    Assert-UpdateFragment `
      -ExpandedRoot $expanded `
      -RunId $RunId `
      -RunHeadSha ([string]$Run.head_sha) `
      -BaseManifest $baseManifest `
      -Manifest $manifest

    foreach ($entry in $script:ArtifactToRepositoryPath.GetEnumerator()) {
      $headFile = Join-Path $temporaryRoot ("head-" + $entry.Key)
      Save-GitHubFile -Path $entry.Value -Ref $HeadSha -Destination $headFile
      $artifactFile = Join-Path $expanded $entry.Key
      if (-not [string]::Equals((Get-Sha256Hex -Path $headFile), (Get-Sha256Hex -Path $artifactFile), [System.StringComparison]::Ordinal)) {
        throw "Artifact entry '$($entry.Key)' is not byte-identical to the PR head blob."
      }
    }

    $artifactExeHash = Get-Sha256Hex -Path (Join-Path $expanded "ffmpeg-x86_64-pc-windows-msvc.exe")
    if (-not [string]::Equals($artifactExeHash, [string]$manifest.expectedExeSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "Artifact executable SHA-256 does not match ffmpeg-version.json."
    }
  }
  finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
      Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
  }
}

if ($SelfTest) {
  Invoke-SelfTest
  return
}

Assert-OnlineParameters
$comparison = Get-ProtectedComparison
$classification = Write-Classification -Comparison $comparison
if ($DetectOnly) {
  return
}

$sameRepository = [string]::Equals($HeadRepository, $Repository, [System.StringComparison]::OrdinalIgnoreCase)
$initial = Test-VendorPolicyDecision `
  -ProtectedChanged $classification.protectedChanged `
  -SameRepository $sameRepository `
  -Association $AuthorAssociation `
  -VendorArtifactChanged $classification.vendorArtifactChanged `
  -RunPresent $true `
  -RunSucceeded $true `
  -RunTrusted $true `
  -ArtifactMatches $true
if (-not $initial.Approved) {
  throw "Vendor policy rejected PR #$PullRequestNumber by @${AuthorLogin}: $($initial.Reason)."
}

if (-not $classification.protectedChanged) {
  Write-Host "Protected vendor and governance files are byte-identical."
  return
}

if (-not $classification.vendorArtifactChanged) {
  Write-Host "Protected authority-only change accepted after same-repository author and environment approval checks."
  return
}

$manifestTemp = Join-Path ([System.IO.Path]::GetTempPath()) ("stickerfit-manifest-" + [System.Guid]::NewGuid().ToString("N") + ".json")
try {
  Save-GitHubFile -Path "tools/ffmpeg/ffmpeg-version.json" -Ref $HeadSha -Destination $manifestTemp
  $manifest = Get-Content -Raw -LiteralPath $manifestTemp | ConvertFrom-Json
  $runId = [string]$manifest.vendorUpdateRunId
  if ($runId -notmatch "\A[1-9][0-9]*\z") {
    throw "Vendor payload changes require a numeric vendorUpdateRunId in ffmpeg-version.json."
  }
}
finally {
  if (Test-Path -LiteralPath $manifestTemp) {
    Remove-Item -LiteralPath $manifestTemp -Force
  }
}

$run = Invoke-GitHubJson -Uri "https://api.github.com/repos/$Repository/actions/runs/$runId"
Assert-RunTrust -Run $run -RunId $runId
Assert-VendorArtifact `
  -RunId $runId `
  -Run $run `
  -HeadTree $comparison.HeadTree `
  -ChangedPaths @($comparison.Changed)
Write-Host "Protected vendor payload exactly matches approved workflow run $runId."
