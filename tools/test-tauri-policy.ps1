param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-Policy {
  param(
    [Parameter(Mandatory = $true)][bool]$Condition,
    [Parameter(Mandatory = $true)][string]$Message
  )

  if (-not $Condition) {
    throw "Tauri policy failure: $Message"
  }
}

function ConvertTo-CanonicalCsp {
  param(
    [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Policy,
    [Parameter(Mandatory = $true)][string]$Label
  )

  Assert-Policy -Condition (-not [string]::IsNullOrWhiteSpace($Policy)) -Message "$Label must not be null or empty"
  $directives = @(
    $Policy -split ';' |
      ForEach-Object { ($_ -replace '\s+', ' ').Trim() } |
      Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
  )
  Assert-Policy -Condition ($directives.Count -eq 6) -Message "$Label must contain exactly six directives"
  return ($directives -join ';')
}

function Assert-ExactStringArray {
  param(
    [Parameter(Mandatory = $true)][object]$Actual,
    [Parameter(Mandatory = $true)][string[]]$Expected,
    [Parameter(Mandatory = $true)][string]$Label
  )

  Assert-Policy -Condition ($Actual -is [System.Array]) -Message "$Label must be a JSON array"
  Assert-Policy -Condition ($Actual.Count -eq $Expected.Count) -Message "$Label has an unexpected item count"
  for ($index = 0; $index -lt $Expected.Count; $index += 1) {
    Assert-Policy `
      -Condition ($Actual[$index] -is [string] -and [string]$Actual[$index] -ceq $Expected[$index]) `
      -Message "$Label item $index must be the string '$($Expected[$index])'"
  }
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
  Assert-Policy `
    -Condition ($properties.Count -eq 1) `
    -Message "$Label must contain exactly one case-sensitive '$Name' property"
  Write-Output -NoEnumerate $properties[0].Value
}

$resolvedWorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot -ErrorAction Stop).Path
$tauriConfigPath = Join-Path $resolvedWorkspaceRoot "src-tauri\tauri.conf.json"
$ffmpegManifestPath = Join-Path $resolvedWorkspaceRoot "tools\ffmpeg\ffmpeg-version.json"
$capabilityPath = Join-Path $resolvedWorkspaceRoot "src-tauri\capabilities\default.json"
$viteConfigPath = Join-Path $resolvedWorkspaceRoot "vite.config.ts"
$diagnosticsPath = Join-Path $resolvedWorkspaceRoot "src\platform\cspDiagnostics.ts"
$mainPath = Join-Path $resolvedWorkspaceRoot "src\main.tsx"
$advancedDetailsPath = Join-Path $resolvedWorkspaceRoot "src\components\AdvancedDetailsPanel.tsx"
$messagesPath = Join-Path $resolvedWorkspaceRoot "src\locales\messages.ts"

foreach ($requiredPath in @(
    $tauriConfigPath,
    $ffmpegManifestPath,
    $capabilityPath,
    $viteConfigPath,
    $diagnosticsPath,
    $mainPath,
    $advancedDetailsPath,
    $messagesPath
  )) {
  Assert-Policy -Condition (Test-Path -LiteralPath $requiredPath -PathType Leaf) -Message "required source is missing: $requiredPath"
}

$tauriConfig = Get-Content -Raw -LiteralPath $tauriConfigPath | ConvertFrom-Json
$ffmpegManifest = Get-Content -Raw -LiteralPath $ffmpegManifestPath | ConvertFrom-Json
$tauriApp = Get-ExactJsonProperty -Object $tauriConfig -Name "app" -Label "Tauri config"
$tauriBundle = Get-ExactJsonProperty -Object $tauriConfig -Name "bundle" -Label "Tauri config"
$security = Get-ExactJsonProperty -Object $tauriApp -Name "security" -Label "Tauri app"
$manifestVendor = Get-ExactJsonProperty -Object $ffmpegManifest -Name "vendor" -Label "FFmpeg manifest"

$legacyResourcePolicy = Get-ExactJsonProperty `
  -Object $manifestVendor `
  -Name "legacyBootstrapAllowedForRoutineVerification" `
  -Label "FFmpeg manifest vendor"
Assert-Policy `
  -Condition ($legacyResourcePolicy -is [bool]) `
  -Message "FFmpeg manifest legacy resource policy must be a JSON boolean"
$expectedResourceMap = if ([bool]$legacyResourcePolicy) {
  [ordered]@{
    "binaries/libwinpthread-1.dll" = "libwinpthread-1.dll"
    "binaries/LICENSE-ffmpeg.txt" = "LICENSE-ffmpeg.txt"
    "binaries/LICENSE-libwinpthread.txt" = "LICENSE-libwinpthread.txt"
    "binaries/ffmpeg-provenance.json" = "ffmpeg-provenance.json"
  }
}
else {
  [ordered]@{
    "binaries/LICENSE-ffmpeg.txt" = "LICENSE-ffmpeg.txt"
    "binaries/ffmpeg-provenance.json" = "ffmpeg-provenance.json"
  }
}
$bundleResources = Get-ExactJsonProperty -Object $tauriBundle -Name "resources" -Label "Tauri bundle"
$actualResourceProperties = @($bundleResources.PSObject.Properties | Where-Object {
    $_.MemberType -eq [System.Management.Automation.PSMemberTypes]::NoteProperty
  })
Assert-Policy `
  -Condition ($actualResourceProperties.Count -eq $expectedResourceMap.Count) `
  -Message "bundle.resources must contain the exact FFmpeg map for the manifest legacy state"
foreach ($entry in $expectedResourceMap.GetEnumerator()) {
  $matchingProperties = @($actualResourceProperties | Where-Object {
      [string]::Equals([string]$_.Name, [string]$entry.Key, [System.StringComparison]::Ordinal)
    })
  Assert-Policy `
    -Condition (
      $matchingProperties.Count -eq 1 -and
      $matchingProperties[0].Value -is [string] -and
      [string]$matchingProperties[0].Value -ceq [string]$entry.Value
    ) `
    -Message "bundle.resources has an unexpected mapping for '$($entry.Key)'"
}
$externalBins = Get-ExactJsonProperty -Object $tauriBundle -Name "externalBin" -Label "Tauri bundle"
Assert-ExactStringArray `
  -Actual $externalBins `
  -Expected @("binaries/ffmpeg") `
  -Label "bundle.externalBin"

$expectedProductionCsp = @(
  "default-src 'self'",
  "connect-src ipc: http://ipc.localhost",
  "img-src 'self' asset: http://asset.localhost blob: data:",
  "media-src 'self' asset: http://asset.localhost blob:",
  "style-src 'self' 'unsafe-inline'",
  "script-src 'self'"
) -join ';'
$expectedDevelopmentCsp = @(
  "default-src 'self'",
  "connect-src ipc: http://ipc.localhost http://localhost:1420 http://127.0.0.1:1420 ws://localhost:1420 ws://127.0.0.1:1420",
  "img-src 'self' asset: http://asset.localhost blob: data:",
  "media-src 'self' asset: http://asset.localhost blob:",
  "style-src 'self' 'unsafe-inline'",
  "script-src 'self'"
) -join ';'

$productionCspValue = Get-ExactJsonProperty -Object $security -Name "csp" -Label "Tauri security"
$developmentCspValue = Get-ExactJsonProperty -Object $security -Name "devCsp" -Label "Tauri security"
Assert-Policy -Condition ($productionCspValue -is [string]) -Message "production CSP must be a JSON string"
Assert-Policy -Condition ($developmentCspValue -is [string]) -Message "development CSP must be a JSON string"
$productionCsp = ConvertTo-CanonicalCsp -Policy $productionCspValue -Label "production CSP"
$developmentCsp = ConvertTo-CanonicalCsp -Policy $developmentCspValue -Label "development CSP"
Assert-Policy -Condition ($productionCsp -ceq $expectedProductionCsp) -Message "production CSP does not exactly match the six-directive local policy"
Assert-Policy -Condition ($developmentCsp -ceq $expectedDevelopmentCsp) -Message "development CSP does not exactly match production plus the four loopback port-1420 endpoints"
Assert-Policy -Condition ($productionCsp -notmatch '(?i)(?:http|ws)://localhost(?::|/|$)') -Message "production CSP contains localhost"
Assert-Policy -Condition ($productionCsp -notmatch '(?i)(?:http|ws)://127\.0\.0\.1(?::|/|$)') -Message "production CSP contains 127.0.0.1"
Assert-Policy -Condition ($productionCsp -notmatch '(?i)\b(?:ws|wss):') -Message "production CSP contains a WebSocket endpoint"

$tauriWindows = Get-ExactJsonProperty -Object $tauriApp -Name "windows" -Label "Tauri app"
Assert-Policy -Condition ($tauriWindows -is [System.Array]) -Message "app.windows must be a JSON array"
foreach ($window in $tauriWindows) {
  if ($window.PSObject.Properties.Name -contains "devtools") {
    Assert-Policy -Condition ($window.devtools -ne $true) -Message "release window enables devtools"
  }
}

$capability = Get-Content -Raw -LiteralPath $capabilityPath | ConvertFrom-Json
$capabilityIdentifier = Get-ExactJsonProperty -Object $capability -Name "identifier" -Label "capability"
$capabilityWindows = Get-ExactJsonProperty -Object $capability -Name "windows" -Label "capability"
$capabilityPermissions = Get-ExactJsonProperty -Object $capability -Name "permissions" -Label "capability"
Assert-Policy -Condition ($capabilityIdentifier -is [string] -and $capabilityIdentifier -ceq "default") -Message "unexpected capability identifier"
Assert-ExactStringArray -Actual $capabilityWindows -Expected @("main") -Label "capability windows"
Assert-ExactStringArray -Actual $capabilityPermissions -Expected @("core:default", "dialog:allow-open") -Label "capability permissions"
Assert-Policy -Condition ($capabilityPermissions -notcontains "dialog:default") -Message "broad dialog:default permission remains enabled"

$viteSource = Get-Content -Raw -LiteralPath $viteConfigPath
Assert-Policy -Condition ($viteSource -match '(?s)server\s*:\s*\{\s*port\s*:\s*1420\s*,') -Message "Vite server port is not the literal 1420"
Assert-Policy -Condition ($viteSource -match '(?s)hmr\s*:[\s\S]{0,240}?\{[\s\S]{0,240}?port\s*:\s*1420\s*,?') -Message "Vite HMR port is not the literal 1420"
Assert-Policy -Condition ($viteSource -notmatch '\b1421\b') -Message "the stale HMR port 1421 remains"

$diagnosticsSource = Get-Content -Raw -LiteralPath $diagnosticsPath
Assert-Policy -Condition ($diagnosticsSource -match 'export const CSP_DIAGNOSTIC_RECORD_LIMIT\s*=\s*50\s*;') -Message "CSP diagnostics record bound is not exactly 50"
Assert-Policy -Condition ($diagnosticsSource -match 'addEventListener\(\s*["'']securitypolicyviolation["'']') -Message "securitypolicyviolation listener is missing"
Assert-Policy -Condition ($diagnosticsSource -match 'new URL\(value\)\.origin') -Message "HTTP/WebSocket blocked targets are not reduced to origin"
Assert-Policy -Condition ($diagnosticsSource -match 'records\.shift\(\)') -Message "bounded records do not evict the oldest entry"
Assert-Policy -Condition ($diagnosticsSource -notmatch '(?i)\b(?:fetch|XMLHttpRequest|localStorage|sessionStorage|writeFile)\b') -Message "CSP diagnostics performs network or persistent storage I/O"

$recordTypeMatch = [regex]::Match(
  $diagnosticsSource,
  '(?s)export type CspViolationRecord\s*=\s*Readonly<\{(?<body>.*?)\}>;'
)
Assert-Policy -Condition $recordTypeMatch.Success -Message "CSP diagnostic record type is missing"
$recordFieldNames = @(
  [regex]::Matches($recordTypeMatch.Groups['body'].Value, '(?m)^\s*(?<name>[A-Za-z][A-Za-z0-9]*)\s*:') |
    ForEach-Object { $_.Groups['name'].Value }
)
Assert-ExactStringArray -Actual $recordFieldNames -Expected @("directive", "blockedOrigin", "count") -Label "CSP diagnostic record fields"
Assert-Policy -Condition ($diagnosticsSource -match 'records\.push\(Object\.freeze\(\{\s*directive\s*,\s*blockedOrigin\s*,\s*count\s*:\s*1\s*\}\)\)') -Message "new CSP records are not constructed from the three-field allowlist"
Assert-Policy -Condition (@([regex]::Matches($diagnosticsSource, 'records\.push\(')).Count -eq 1) -Message "CSP diagnostics has an unreviewed additional record insertion path"
Assert-Policy -Condition ($diagnosticsSource -notmatch '(?i)\b(?:sample|originalPolicy|documentURI|sourceFile|lineNumber|columnNumber|statusCode)\b') -Message "CSP diagnostics references a forbidden full-event field"
Assert-Policy -Condition ($diagnosticsSource -notmatch '\.blockedURI\b') -Message "blockedURI is accessed directly instead of through the sanitizer"
$blockedUriMentions = @([regex]::Matches($diagnosticsSource, '\bblockedURI\b'))
Assert-Policy -Condition ($blockedUriMentions.Count -eq 2) -Message "blockedURI must appear only in the input type and sanitizer read"
Assert-Policy -Condition ($diagnosticsSource -notmatch '(?s)\{[^{}]*\bblockedURI\b[^{}]*\}\s*=\s*event\b') -Message "blockedURI must not be destructured or aliased from the raw event"
Assert-Policy -Condition ($diagnosticsSource -match 'const\s+blockedOrigin\s*=\s*sanitizeBlockedOrigin\(event\)\s*;') -Message "stored blockedOrigin is not assigned directly from the sanitizer"
$listenerRegistrations = @([regex]::Matches($diagnosticsSource, 'target\.addEventListener\(\s*["'']securitypolicyviolation["'']'))
Assert-Policy -Condition ($listenerRegistrations.Count -eq 1) -Message "CSP diagnostics must have exactly one listener-registration site"
Assert-Policy -Condition ($diagnosticsSource -match 'if\s*\(\s*!target\s*\|\|\s*installedTargets\.has\(target\)\s*\)\s*\{\s*return\s*;\s*\}') -Message "CSP listener registration is not dominated by the null/idempotence guard"
$listenerIndex = $listenerRegistrations[0].Index
$installedIndex = $diagnosticsSource.IndexOf('installedTargets.add(target)', [StringComparison]::Ordinal)
Assert-Policy -Condition ($listenerIndex -ge 0 -and $installedIndex -gt $listenerIndex) -Message "CSP target is not recorded after the single listener registration"

$mainSource = Get-Content -Raw -LiteralPath $mainPath
$installIndex = $mainSource.IndexOf("installCspDiagnostics();", [StringComparison]::Ordinal)
$createRootIndex = $mainSource.IndexOf("ReactDOM.createRoot", [StringComparison]::Ordinal)
Assert-Policy -Condition ($installIndex -ge 0) -Message "main bootstrap does not install CSP diagnostics"
Assert-Policy -Condition ($createRootIndex -ge 0) -Message "React root bootstrap is missing"
Assert-Policy -Condition ($installIndex -lt $createRootIndex) -Message "CSP diagnostics is installed after React bootstrap"

$advancedDetailsSource = Get-Content -Raw -LiteralPath $advancedDetailsPath
$conditionalIndex = $advancedDetailsSource.IndexOf('!hasAdvancedContent ?', [StringComparison]::Ordinal)
$counterIndex = $advancedDetailsSource.IndexOf('copy.cspViolationCount(violationCount)', [StringComparison]::Ordinal)
Assert-Policy -Condition ($advancedDetailsSource -match 'useCspDiagnostics\(\)') -Message "Advanced Details does not subscribe to CSP diagnostics"
Assert-Policy -Condition ($counterIndex -ge 0) -Message "Advanced Details does not render the localized CSP violation count"
Assert-Policy -Condition ($conditionalIndex -ge 0) -Message "Advanced Details empty-plan branch is missing or has an unreviewed shape"
Assert-Policy -Condition ($counterIndex -lt $conditionalIndex) -Message "CSP count is hidden until optimizer-plan content exists"
$beforeCounter = $advancedDetailsSource.Substring(0, $counterIndex)
Assert-Policy -Condition ($beforeCounter -notmatch '(?s)if\s*\([^)]*hasAdvancedContent[^)]*\)\s*\{?\s*return\b') -Message "Advanced Details returns before rendering the CSP count for an empty plan"

$messagesSource = Get-Content -Raw -LiteralPath $messagesPath
Assert-Policy -Condition ($messagesSource -match 'cspViolationCount') -Message "localized CSP violation-count copy is missing"

Write-Host "Tauri CSP, capability, diagnostics, and HMR policies passed."
