param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-Contract {
  param(
    [Parameter(Mandatory = $true)][bool]$Condition,
    [Parameter(Mandatory = $true)][string]$Message
  )

  if (-not $Condition) {
    throw "Bootstrap contract failure: $Message"
  }
}

function Get-OperationText {
  param([Parameter(Mandatory = $true)]$Operation)

  return ((@($Operation.executable) + @($Operation.arguments)) -join ' ')
}

function Invoke-BootstrapDryRun {
  param([string[]]$AdditionalArguments = @())

  $bootstrapPath = Join-Path $WorkspaceRoot "tools\bootstrap-build-windows.ps1"
  $powershellPath = Join-Path $PSHOME "powershell.exe"
  if (-not (Test-Path -LiteralPath $powershellPath -PathType Leaf)) {
    $powershellCommand = Get-Command powershell.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1
    $powershellPath = $powershellCommand.Source
  }

  $arguments = @(
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-File", $bootstrapPath,
    "-WorkspaceRoot", $WorkspaceRoot,
    "-DryRun"
  ) + $AdditionalArguments
  $output = & $powershellPath @arguments 2>&1
  $exitCode = $LASTEXITCODE
  if ($exitCode -ne 0) {
    throw "Bootstrap DryRun failed with exit code $exitCode.`n$($output | Out-String)"
  }

  try {
    $transcript = ($output | Out-String) | ConvertFrom-Json -ErrorAction Stop
  }
  catch {
    throw "Bootstrap DryRun did not return standalone JSON: $($_.Exception.Message)`n$($output | Out-String)"
  }

  Assert-Contract -Condition ($transcript.schemaVersion -eq 1) -Message "unexpected transcript schema"
  Assert-Contract -Condition ($transcript.dryRun -eq $true) -Message "transcript is not marked dryRun"
  return $transcript
}

$resolvedWorkspaceRoot = (Resolve-Path -LiteralPath $WorkspaceRoot -ErrorAction Stop).Path
$bootstrapSource = Get-Content -Raw -LiteralPath (Join-Path $resolvedWorkspaceRoot "tools\bootstrap-build-windows.ps1")
$package = Get-Content -Raw -LiteralPath (Join-Path $resolvedWorkspaceRoot "package.json") | ConvertFrom-Json

$skipTranscript = Invoke-BootstrapDryRun -AdditionalArguments @("-SkipDependencyInstall")
Assert-Contract -Condition ($skipTranscript.mode -eq "VerifyOnly") -Message "SkipDependencyInstall must select VerifyOnly mode"
$skipOperations = @($skipTranscript.operations)
$skipText = @($skipOperations | ForEach-Object { Get-OperationText -Operation $_ }) -join "`n"
foreach ($operation in $skipOperations) {
  Assert-Contract -Condition ($operation.PSObject.Properties.Name -contains "mutatesMachine") -Message "operation omitted mutatesMachine"
  Assert-Contract -Condition ($operation.PSObject.Properties.Name -contains "mutatesRepository") -Message "operation omitted mutatesRepository"
  Assert-Contract -Condition ($operation.PSObject.Properties.Name -contains "targetPath") -Message "operation omitted targetPath"
}
Assert-Contract -Condition (@($skipOperations | Where-Object { $_.kind -eq "elevate" }).Count -eq 0) -Message "verify-only mode planned elevation"
Assert-Contract -Condition (@($skipOperations | Where-Object { $_.kind -in @("install", "toolchain-install", "toolchain-select", "system-update") }).Count -eq 0) -Message "verify-only mode planned dependency mutation"
Assert-Contract -Condition ($skipText -notmatch '(?im)^.*winget(?:\.exe)?.*\binstall\b') -Message "verify-only mode contains winget install"
Assert-Contract -Condition ($skipText -notmatch '(?im)^.*rustup(?:\.exe)?.*\b(?:toolchain\s+install|default)\b') -Message "verify-only mode contains Rust toolchain mutation"
Assert-Contract -Condition ($skipText -notmatch '(?im)^.*cargo(?:\.exe)?.*\binstall\b') -Message "verify-only mode contains cargo install"
Assert-Contract -Condition ($skipText -notmatch '(?im)pacman\s+-S') -Message "verify-only mode contains pacman mutation"
Assert-Contract -Condition (@($skipOperations | Where-Object { $_.mutatesMachine -and $_.mutatesRepository }).Count -eq 0) -Message "operation ambiguously mutates both machine and repository"
$npmCiOperation = @($skipOperations | Where-Object { (Get-OperationText -Operation $_) -eq "npm.cmd ci" })
Assert-Contract -Condition ($npmCiOperation.Count -eq 1 -and $npmCiOperation[0].mutatesRepository -and -not $npmCiOperation[0].mutatesMachine) -Message "npm ci mutation scope is not repository-only"
$sourceVerificationOperation = @($skipOperations | Where-Object { (Get-OperationText -Operation $_) -match '-VerifySourceOnly' })
Assert-Contract -Condition ($sourceVerificationOperation.Count -eq 1 -and $sourceVerificationOperation[0].mutatesMachine -and -not $sourceVerificationOperation[0].mutatesRepository) -Message "source cache mutation scope is not machine/temp-only"
Assert-Contract -Condition (-not [string]::IsNullOrWhiteSpace([string]$sourceVerificationOperation[0].targetPath)) -Message "source cache target path is missing"

$webViewRegistryReads = @($skipOperations | Where-Object { $_.kind -eq "registry-read" })
Assert-Contract -Condition ($webViewRegistryReads.Count -eq 2) -Message "WebView2 DryRun must contain exactly the official HKLM and HKCU reads"
Assert-Contract -Condition (@($webViewRegistryReads | Where-Object { $_.mutatesMachine -or $_.mutatesRepository -or $_.requiresElevation }).Count -eq 0) -Message "WebView2 registry detection is not read-only"
$webViewMachineReads = @($webViewRegistryReads | Where-Object { $_.arguments[0] -eq "HKLM" })
$webViewUserReads = @($webViewRegistryReads | Where-Object { $_.arguments[0] -eq "HKCU" })
$expectedUserRegistryView = if ([Environment]::Is64BitOperatingSystem) { "Registry64" } else { "Registry32" }
Assert-Contract -Condition ($webViewMachineReads.Count -eq 1) -Message "WebView2 DryRun must contain exactly one HKLM read"
Assert-Contract -Condition ($webViewMachineReads[0].arguments[1] -eq "Registry32") -Message "WebView2 HKLM must use only Registry32 (the official WOW6432Node mapping on 64-bit Windows)"
Assert-Contract -Condition ($webViewUserReads.Count -eq 1) -Message "WebView2 DryRun must contain exactly one HKCU read"
Assert-Contract -Condition ($webViewUserReads[0].arguments[1] -eq $expectedUserRegistryView) -Message "WebView2 HKCU does not use the official Software path's native OS view"
Assert-Contract -Condition (@($webViewRegistryReads | Where-Object { $_.arguments[0] -eq "HKLM" -and $_.arguments[1] -eq "Registry64" }).Count -eq 0) -Message "WebView2 DryRun planned the unofficial HKLM Registry64 key"
$webViewRegistryText = @($webViewRegistryReads | ForEach-Object { Get-OperationText -Operation $_ }) -join "`n"
foreach ($requiredRegistryToken in @(
    "HKLM",
    "HKCU",
    "Registry32",
    "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    "pv"
  )) {
  Assert-Contract -Condition ($webViewRegistryText.Contains($requiredRegistryToken)) -Message "WebView2 DryRun omitted registry token: $requiredRegistryToken"
}
if ([Environment]::Is64BitOperatingSystem) {
  Assert-Contract -Condition ($webViewUserReads[0].arguments[1] -eq "Registry64") -Message "64-bit WebView2 HKCU DryRun omitted the native Registry64 view"
} else {
  Assert-Contract -Condition (-not $webViewRegistryText.Contains("Registry64")) -Message "32-bit WebView2 DryRun planned an unavailable Registry64 read"
}
$webViewFileReads = @($skipOperations | Where-Object { $_.kind -eq "filesystem-read" -and (Get-OperationText -Operation $_) -match 'EdgeWebView' })
$expectedFileReadCount = @(${env:ProgramFiles(x86)}, $env:ProgramFiles) |
  Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
  Select-Object -Unique |
  Measure-Object |
  Select-Object -ExpandProperty Count
Assert-Contract -Condition ($webViewFileReads.Count -eq $expectedFileReadCount) -Message "WebView2 DryRun does not match the available Program Files fallback roots"
Assert-Contract -Condition (@($webViewFileReads | Where-Object { $_.mutatesMachine -or $_.mutatesRepository -or $_.requiresElevation }).Count -eq 0) -Message "WebView2 filesystem fallback is not read-only"
$webViewFileText = @($webViewFileReads | ForEach-Object { Get-OperationText -Operation $_ }) -join "`n"
Assert-Contract -Condition ($webViewFileText -match 'Microsoft\\EdgeWebView\\Application') -Message "WebView2 filesystem fallback root is missing"
Assert-Contract -Condition ($webViewFileText -match 'msedgewebview2\.exe') -Message "WebView2 filesystem fallback executable probe is missing"
Assert-Contract -Condition ($webViewFileText -match 'VersionInfo\.ProductVersion') -Message "WebView2 filesystem fallback version read is missing"
Assert-Contract -Condition ($skipText -notmatch 'msedgewebview2\.exe\s+--exists-only') -Message "DryRun contains the obsolete virtual WebView2 executable invocation"

foreach ($requiredCommand in @(
    'npm.cmd ci',
    'tools/ffmpeg/build-minimal-ffmpeg.ps1',
    '-VerifySourceOnly',
    '-VerifyVendorArtifacts',
    'npm.cmd run build:desktop',
    'npm.cmd run report:size'
  )) {
  Assert-Contract -Condition ($skipText.Contains($requiredCommand)) -Message "verify-only transcript omitted repository command: $requiredCommand"
}
Assert-Contract -Condition ($skipText -notmatch '(?:^|\s)-UpdateVendorArtifacts(?:\s|$)') -Message "routine bootstrap planned a protected vendor update"

$checkTranscript = Invoke-BootstrapDryRun -AdditionalArguments @("-SkipDependencyInstall", "-RunChecks")
$checkText = @($checkTranscript.operations | ForEach-Object { Get-OperationText -Operation $_ }) -join "`n"
Assert-Contract -Condition ($checkText.Contains('npm.cmd run check:quality')) -Message "RunChecks omitted check:quality"
Assert-Contract -Condition ($checkText.Contains('npm.cmd run test:release-sidecar')) -Message "RunChecks omitted packaged-sidecar verification"

$normalTranscript = Invoke-BootstrapDryRun
$elevationOperations = @($normalTranscript.operations | Where-Object { $_.kind -eq "elevate" })
Assert-Contract -Condition ($elevationOperations.Count -eq 1) -Message "normal mode must plan exactly one elevation"
$elevationText = Get-OperationText -Operation $elevationOperations[0]
Assert-Contract -Condition ($elevationText -match '(?:^|\s)-InstallDependenciesOnly(?:\s|$)') -Message "elevated child is not install-only"
Assert-Contract -Condition ($elevationText -notmatch '(?:^|\s)-(?:RunChecks|Clean|SkipDependencyInstall)(?:\s|$)') -Message "repository switches leaked into elevated child"

$overrideValues = [ordered]@{
  "-BashPath" = "C:\Tool Overrides\bash.exe"
  "-GpgPath" = "C:\Tool Overrides\gpg.exe"
  "-GpgvPath" = "C:\Tool Overrides\gpgv.exe"
  "-VsWherePath" = "C:\Tool Overrides\vswhere.exe"
}
$overrideArguments = New-Object System.Collections.Generic.List[string]
foreach ($entry in $overrideValues.GetEnumerator()) {
  $overrideArguments.Add([string]$entry.Key)
  $overrideArguments.Add([string]$entry.Value)
}
$overrideTranscript = Invoke-BootstrapDryRun -AdditionalArguments @($overrideArguments)
$overrideElevation = @($overrideTranscript.operations | Where-Object { $_.kind -eq "elevate" })
Assert-Contract -Condition ($overrideElevation.Count -eq 1) -Message "override DryRun did not plan exactly one elevation"
$forwardedArguments = @($overrideElevation[0].arguments)
foreach ($entry in $overrideValues.GetEnumerator()) {
  $flagIndex = [Array]::IndexOf([object[]]$forwardedArguments, [string]$entry.Key)
  Assert-Contract -Condition ($flagIndex -ge 0) -Message "DryRun elevation omitted override $($entry.Key)"
  Assert-Contract -Condition ([string]$forwardedArguments[$flagIndex + 1] -ceq [string]$entry.Value) -Message "DryRun elevation changed the spaced value for $($entry.Key)"
}

$installTranscript = Invoke-BootstrapDryRun -AdditionalArguments @("-InstallDependenciesOnly")
Assert-Contract -Condition ($installTranscript.mode -eq "InstallDependenciesOnly") -Message "install-only mode was not selected"
$installOperations = @($installTranscript.operations)
Assert-Contract -Condition (@($installOperations | Where-Object { $_.phase -eq "repository" }).Count -eq 0) -Message "install-only child planned repository work"
Assert-Contract -Condition (@($installOperations | Where-Object { -not $_.requiresElevation }).Count -eq 0) -Message "install-only operation was not marked elevated"
Assert-Contract -Condition (@($installOperations | Where-Object { $_.mutatesRepository }).Count -eq 0) -Message "install-only child planned a repository mutation"
$wingetOperations = @($installOperations | Where-Object { $_.executable -eq "winget.exe" })
Assert-Contract -Condition ($wingetOperations.Count -eq 6) -Message "unexpected winget package count"
foreach ($wingetOperation in $wingetOperations) {
  $wingetText = Get-OperationText -Operation $wingetOperation
  foreach ($requiredFlag in @("--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements")) {
    Assert-Contract -Condition ($wingetText -match ('(?:^|\s)' + [regex]::Escape($requiredFlag) + '(?:\s|$)')) -Message "winget DryRun omitted $requiredFlag"
  }
}
$vsWingetOperation = @($wingetOperations | Where-Object { (Get-OperationText -Operation $_) -match 'Microsoft\.VisualStudio\.2022\.BuildTools' })
Assert-Contract -Condition ($vsWingetOperation.Count -eq 1) -Message "Visual Studio winget operation is missing"
$sevenZipWingetOperation = @($wingetOperations | Where-Object { (Get-OperationText -Operation $_) -match '(?:^|\s)7zip\.7zip(?:\s|$)' })
Assert-Contract -Condition ($sevenZipWingetOperation.Count -eq 1) -Message "canonical 7-Zip winget operation is missing"
$vsWingetText = Get-OperationText -Operation $vsWingetOperation[0]
Assert-Contract -Condition ($vsWingetText -match '--override\s+--wait --quiet --norestart --nocache --add Microsoft\.VisualStudio\.Workload\.VCTools --includeRecommended') -Message "Visual Studio winget override does not match the actual installer contract"

$pacmanCommands = @(
  $installOperations |
    Where-Object { $_.executable -eq "bash.exe" } |
    ForEach-Object { @($_.arguments)[-1] }
)
Assert-Contract -Condition ($pacmanCommands.Count -eq 3) -Message "unexpected MSYS2 update/install operation count"
Assert-Contract -Condition ($pacmanCommands[0] -eq "pacman -Syu --noconfirm") -Message "MSYS2 update pass 1 is not a full -Syu"
Assert-Contract -Condition ($pacmanCommands[1] -eq "pacman -Syu --noconfirm") -Message "MSYS2 update pass 2 is not a full -Syu"
Assert-Contract -Condition ($pacmanCommands[2] -match '^pacman -S --needed --noconfirm\b') -Message "MSYS2 packages are not installed after the full updates with --needed"
Assert-Contract -Condition (($pacmanCommands -join "`n") -notmatch '(?m)pacman\s+-Sy(?!u)\b') -Message "standalone pacman -Sy is forbidden"

$cleanTranscript = Invoke-BootstrapDryRun -AdditionalArguments @("-SkipDependencyInstall", "-Clean")
$rootWithSeparator = $resolvedWorkspaceRoot.TrimEnd('\') + '\'
$cleanOperations = @($cleanTranscript.operations | Where-Object { $_.kind -eq "delete" })
Assert-Contract -Condition ($cleanOperations.Count -eq 2) -Message "Clean must plan exactly the dist and src-tauri/target/release deletions"
$expectedCleanPaths = @(
  [IO.Path]::GetFullPath((Join-Path $resolvedWorkspaceRoot "dist")),
  [IO.Path]::GetFullPath((Join-Path $resolvedWorkspaceRoot "src-tauri\target\release"))
) | Sort-Object
$actualCleanPaths = New-Object System.Collections.Generic.List[string]
foreach ($cleanOperation in $cleanOperations) {
  $literalPathIndex = [Array]::IndexOf([object[]]@($cleanOperation.arguments), "-LiteralPath")
  Assert-Contract -Condition ($literalPathIndex -ge 0) -Message "clean operation does not use -LiteralPath"
  $cleanPath = [IO.Path]::GetFullPath([string]$cleanOperation.arguments[$literalPathIndex + 1])
  Assert-Contract -Condition ($cleanOperation.mutatesRepository -and -not $cleanOperation.mutatesMachine) -Message "clean mutation scope is not repository-only"
  Assert-Contract -Condition ([string]::Equals([IO.Path]::GetFullPath([string]$cleanOperation.targetPath), $cleanPath, [StringComparison]::OrdinalIgnoreCase)) -Message "clean targetPath does not match its literal deletion target"
  Assert-Contract -Condition ($cleanPath.StartsWith($rootWithSeparator, [StringComparison]::OrdinalIgnoreCase)) -Message "clean operation escaped the workspace"
  Assert-Contract -Condition (-not [string]::Equals($cleanPath.TrimEnd('\'), $resolvedWorkspaceRoot.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)) -Message "clean operation targeted the workspace root"
  $actualCleanPaths.Add($cleanPath)
}
Assert-Contract -Condition ((@($actualCleanPaths | Sort-Object) -join "`n") -ceq ($expectedCleanPaths -join "`n")) -Message "Clean targets do not exactly match dist and src-tauri/target/release"

Assert-Contract -Condition ($bootstrapSource -match 'if \(\$isAdministrator\)[\s\S]*Repository work must not run elevated') -Message "normal administrator invocation is not refused"
Assert-Contract -Condition ($bootstrapSource -match 'Remove-Item -LiteralPath \$safePath -Recurse -Force') -Message "clean does not use the validated literal path"
Assert-Contract -Condition ($bootstrapSource -match 'ReparsePoint') -Message "clean does not reject reparse points"
Assert-Contract -Condition ($bootstrapSource -match '>=22\.12\.0 <23 or >=24\.0\.0 <25') -Message "supported Node range is not exact"
Assert-Contract -Condition ($bootstrapSource -match '1\.96\.1-x86_64-pc-windows-msvc') -Message "release Rust toolchain is not exact"
Assert-Contract -Condition ($bootstrapSource -match 'cargo-audit must be exactly') -Message "cargo-audit exact-version gate is missing"
Assert-Contract -Condition ($bootstrapSource -match 'sevenZip\s*=\s*\[Version\]"26\.2\.0"') -Message "7-Zip minimum version must remain 26.2.0"
Assert-Contract -Condition ($bootstrapSource -match '7-Zip\\7z\.exe') -Message "canonical Program Files 7-Zip path is missing"
Assert-Contract -Condition ($bootstrapSource -match 'webView2\s*=\s*\[Version\]"109\.0\.0"') -Message "WebView2 minimum version must remain 109.0.0"
Assert-Contract -Condition ($bootstrapSource -match '\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5\}') -Message "official WebView2 Runtime client ID is missing"
Assert-Contract -Condition ($bootstrapSource -match 'SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients') -Message "official 64-bit HKLM WOW6432Node registration path is missing"
Assert-Contract -Condition ($bootstrapSource -match '\[Microsoft\.Win32\.RegistryHive\]::LocalMachine') -Message "WebView2 detection omits HKLM per-machine registration"
Assert-Contract -Condition ($bootstrapSource -match '\[Microsoft\.Win32\.RegistryHive\]::CurrentUser') -Message "WebView2 detection omits HKCU per-user registration"
Assert-Contract -Condition ($bootstrapSource -match '\[Microsoft\.Win32\.RegistryView\]::Registry32') -Message "WebView2 detection omits the 32-bit registry view"
Assert-Contract -Condition ($bootstrapSource -match '\[Microsoft\.Win32\.RegistryView\]::Registry64') -Message "WebView2 detection omits the 64-bit registry view"
Assert-Contract -Condition ($bootstrapSource -match 'Hive\s*=\s*\[Microsoft\.Win32\.RegistryHive\]::LocalMachine\s+View\s*=\s*\[Microsoft\.Win32\.RegistryView\]::Registry32') -Message "WebView2 HKLM candidate is not fixed to Registry32"
Assert-Contract -Condition ($bootstrapSource -notmatch 'Hive\s*=\s*\[Microsoft\.Win32\.RegistryHive\]::LocalMachine\s+View\s*=\s*\[Microsoft\.Win32\.RegistryView\]::Registry64') -Message "WebView2 accepts the unofficial HKLM Registry64 candidate"
Assert-Contract -Condition ($bootstrapSource -match 'Hive\s*=\s*\[Microsoft\.Win32\.RegistryHive\]::CurrentUser\s+View\s*=\s*\$userView') -Message "WebView2 HKCU candidate does not use the OS-appropriate view"
Assert-Contract -Condition ($bootstrapSource -match 'OpenBaseKey\(\$hive, \$view\)') -Message "WebView2 registry detection does not open explicit hive/view pairs"
Assert-Contract -Condition ($bootstrapSource -match 'OpenSubKey\(\$subKeyPath, \$false\)') -Message "WebView2 registry detection does not open the client key read-only"
Assert-Contract -Condition ($bootstrapSource -match 'GetValueKind\("pv"\).*RegistryValueKind\]::String') -Message "WebView2 pv is not constrained to REG_SZ"
Assert-Contract -Condition ($bootstrapSource -match 'GetValue\([\s\S]*?"pv"[\s\S]*?DoNotExpandEnvironmentNames') -Message "WebView2 pv is not read fail-safely"
Assert-Contract -Condition ($bootstrapSource -match '\$parsedVersion -le \[Version\]"0\.0\.0\.0"') -Message "WebView2 zero version is not rejected"
Assert-Contract -Condition ([regex]::Matches($bootstrapSource, '\$version -ge \$minimumToolVersions\.webView2').Count -ge 2) -Message "WebView2 registry and file candidates are not both gated by the minimum version"
Assert-Contract -Condition ($bootstrapSource -match 'Where-Object \{ \$_\.MeetsMinimum \}') -Message "WebView2 registry selection can accept a below-minimum candidate"
Assert-Contract -Condition ($bootstrapSource -match 'Runtime version is too old\. Required >= .*Observed candidates:') -Message "WebView2 explicit version-too-old failure or observed-candidate summary is missing"
Assert-Contract -Condition ($bootstrapSource -match 'Runtime was not found in the official registry locations or Program Files fallback') -Message "WebView2 missing-runtime failure is not distinct from version-too-old"
Assert-Contract -Condition ($bootstrapSource -notmatch '(?i)\b(?:CreateSubKey|SetValue|DeleteSubKey|DeleteValue)\s*\(') -Message "WebView2 detection contains a registry mutation API"

Assert-Contract -Condition ($package.scripts.'build:desktop' -eq 'tauri build -- --locked') -Message "build:desktop must pass --locked to Tauri/Cargo"
Assert-Contract -Condition ($package.scripts.'dev:desktop' -eq 'tauri dev -- --locked') -Message "dev:desktop must pass --locked to Tauri/Cargo"
foreach ($property in $package.scripts.PSObject.Properties) {
  $scriptText = [string]$property.Value
  $commandSegments = [regex]::Split($scriptText, '\s*(?:&&|\|\||;)\s*')
  foreach ($commandSegment in $commandSegments) {
    if ($commandSegment -match '(?i)(?:^|[^A-Za-z0-9_-])cargo\s+(?:\+[^\s]+\s+)?(?:test|check|clippy|build)\b') {
      Assert-Contract -Condition ($commandSegment -match '(?i)(?:^|\s)--locked(?:\s|$)') -Message "script $($property.Name) has an unlocked dependency-resolving Cargo command segment: $commandSegment"
    }
  }
}

Write-Host "Bootstrap contract checks passed."
