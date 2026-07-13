[CmdletBinding()]
param(
    [string]$WorkspaceRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-Sha256Hex {
    param(
        [Parameter(Mandatory = $true)]
        [string]$LiteralPath
    )

    $stream = [System.IO.File]::OpenRead($LiteralPath)
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

if (-not $PSBoundParameters.ContainsKey('WorkspaceRoot')) {
    $WorkspaceRoot = Join-Path $PSScriptRoot '..'
}

$WorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot).Path
$fixtureDirectory = Join-Path $WorkspaceRoot 'src-tauri\tests\fixtures'
$manifestPath = Join-Path $fixtureDirectory 'media-fixtures.json'
$cargoManifestPath = Join-Path $WorkspaceRoot 'src-tauri\Cargo.toml'

if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Generated-media manifest is missing: $manifestPath"
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$expectedCommit = 'da26bb3abb2ae5b7d97e91814c28efd20363c060'
$expected = [ordered]@{
    'tiny.mp4' = [ordered]@{
        SourcePath = 'media/test/data/four-colors.mp4'
        SourceRecipe = 'Chromium records this as a 2-second iStopMotion video made from a four-color Paint image.'
        Sha256 = '7ec80cde3a56d6ea81a9c3c617d339e5d0819c6b4914a9b2066cebe515c90089'
        SizeBytes = 12141L
        Codec = 'h264'
    }
    'tiny.webm' = [ordered]@{
        SourcePath = 'media/test/data/four-colors-vp9.webm'
        SourceRecipe = 'Chromium records this only as four-colors.mp4 converted to WebM with FFmpeg; exact arguments and encoder settings are not recorded.'
        Sha256 = '5faabe4799b1e4bdb9e3cd216b6044d7167e737341035d44323acab431613821'
        SizeBytes = 5465L
        Codec = 'vp9'
    }
}

if ([int]$manifest.schemaVersion -ne 1) {
    throw "Unsupported generated-media manifest schema: $($manifest.schemaVersion)"
}
if ([string]$manifest.upstream.repository -ne 'https://chromium.googlesource.com/chromium/src') {
    throw 'Generated-media manifest repository does not match the pinned Chromium source.'
}
if ([string]$manifest.upstream.commit -ne $expectedCommit) {
    throw 'Generated-media manifest commit does not match the pinned Chromium revision.'
}
if ([string]$manifest.upstream.readmeUrl -ne "https://chromium.googlesource.com/chromium/src/+/$expectedCommit/media/test/data/README.md") {
    throw 'Generated-media manifest README URL does not match the pinned Chromium revision.'
}
if ([string]$manifest.upstream.license -ne 'BSD-3-Clause' -or
    [string]$manifest.upstream.licenseFile -ne 'LICENSE-chromium.txt' -or
    [string]$manifest.upstream.licenseSha256 -ne '8c19aaf4ec1d6a59bb2c946461110cc5aac2bdc5b9d2d79336e46354fc9f1d8a' -or
    [long]$manifest.upstream.licenseSizeBytes -ne 1458L) {
    throw 'Generated-media manifest license metadata is not exact.'
}

$licensePath = Join-Path $fixtureDirectory ([string]$manifest.upstream.licenseFile)
if (-not (Test-Path -LiteralPath $licensePath -PathType Leaf)) {
    throw "Generated-media fixture license is missing: $licensePath"
}
$license = Get-Item -LiteralPath $licensePath
if ($license.Length -ne 1458L) {
    throw "Generated-media fixture license size mismatch: expected 1458, got $($license.Length)."
}
$licenseHash = Get-Sha256Hex -LiteralPath $licensePath
if ($licenseHash -ne '8c19aaf4ec1d6a59bb2c946461110cc5aac2bdc5b9d2d79336e46354fc9f1d8a') {
    throw 'Generated-media fixture license SHA-256 mismatch.'
}

$entries = @($manifest.fixtures)
$manifestNames = @($entries | ForEach-Object { [string]$_.file } | Sort-Object)
$expectedNames = @($expected.Keys | Sort-Object)
$duplicateNames = @($manifestNames | Group-Object | Where-Object Count -ne 1)
if ($duplicateNames.Count -ne 0) {
    throw "Generated-media manifest contains duplicate fixture names: $($duplicateNames.Name -join ', ')"
}
$membershipDifference = @(Compare-Object -ReferenceObject $expectedNames -DifferenceObject $manifestNames)
if ($membershipDifference.Count -ne 0) {
    throw 'Generated-media manifest membership must be exactly tiny.mp4 and tiny.webm.'
}

$diskNames = @(
    Get-ChildItem -LiteralPath $fixtureDirectory -File |
        Where-Object { $_.Extension -in @('.mp4', '.webm') } |
        ForEach-Object Name |
        Sort-Object
)
$diskDifference = @(Compare-Object -ReferenceObject $expectedNames -DifferenceObject $diskNames)
if ($diskDifference.Count -ne 0) {
    throw 'Checked-in generated-media binary membership must be exactly tiny.mp4 and tiny.webm.'
}

foreach ($entry in $entries) {
    $name = [string]$entry.file
    $contract = $expected[$name]
    $expectedSourceUrl = "https://chromium.googlesource.com/chromium/src/+/$expectedCommit/$($contract.SourcePath)?format=TEXT"
    if ([string]$entry.sourcePath -ne $contract.SourcePath -or
        [string]$entry.sourceUrl -ne $expectedSourceUrl -or
        [string]$entry.sourceRecipe -ne $contract.SourceRecipe -or
        $null -ne $entry.sourceCommand -or
        [string]$entry.sha256 -ne $contract.Sha256 -or
        [long]$entry.sizeBytes -ne $contract.SizeBytes -or
        [int]$entry.width -ne 960 -or
        [int]$entry.height -ne 540 -or
        [int]$entry.durationSeconds -ne 2 -or
        [string]$entry.codec -ne $contract.Codec) {
        throw "Generated-media manifest metadata is not exact for $name."
    }

    $fixturePath = Join-Path $fixtureDirectory $name
    $item = Get-Item -LiteralPath $fixturePath
    if ($item.Length -ne $contract.SizeBytes) {
        throw "Generated-media fixture size mismatch for ${name}: expected $($contract.SizeBytes), got $($item.Length)."
    }
    $actualHash = Get-Sha256Hex -LiteralPath $fixturePath
    if ($actualHash -ne $contract.Sha256) {
        throw "Generated-media fixture SHA-256 mismatch for $name."
    }
    if ($item.Length -gt 100KB) {
        throw "Generated-media fixture exceeds the 100 KiB source-test cap: $name."
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo is required after fixture pre-verification.'
}

$testNames = @(
    'generated_static_png_jpeg_and_bmp_are_inspectable',
    'generated_gif_and_apng_are_inspectable',
    'generated_frame_count_boundary_accepts_300_and_rejects_301',
    'generated_duration_boundary_accepts_5000000_and_rejects_5000001_us',
    'generated_output_size_boundary_matches_estimate_wire_limit',
    'generated_malformed_png_forms_are_rejected_before_decode',
    'generated_high_entropy_320x320_png_is_deterministic',
    'checked_video_fixtures_match_the_source_contract_without_dynamic_decode',
    'checked_video_fixtures_are_inspectable_through_windows_runtime_paths'
)

Push-Location -LiteralPath $WorkspaceRoot
try {
    foreach ($testName in $testNames) {
        & cargo test --locked --manifest-path $cargoManifestPath --lib "generated_media_tests::$testName" -- --exact
        if ($LASTEXITCODE -ne 0) {
            throw "Generated-media Rust test failed: $testName (exit code $LASTEXITCODE)."
        }
    }
}
finally {
    Pop-Location
}

Write-Host 'Generated-media fixture pre-verification and exact Rust tests passed.'
