param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
  [string]$BashPath,
  [string]$GpgPath,
  [string]$GpgvPath,
  [string]$VsWherePath,
  [switch]$SkipDependencyInstall,
  [switch]$InstallDependenciesOnly,
  [switch]$RunChecks,
  [switch]$Clean,
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$requiredRustToolchain = "1.96.1-x86_64-pc-windows-msvc"
$requiredCargoAuditVersion = "0.22.2"
$minimumToolVersions = @{
  npm = [Version]"10.0.0"
  bash = [Version]"5.2.0"
  pacman = [Version]"6.0.0"
  gcc = [Version]"13.0.0"
  binutils = [Version]"2.40.0"
  make = [Version]"4.3.0"
  nasm = [Version]"2.16.0"
  pkgconf = [Version]"1.8.0"
  gnupg = [Version]"2.4.0"
  visualStudio = [Version]"17.0.0"
  msvcTools = [Version]"14.0.0"
  webView2 = [Version]"109.0.0"
  sevenZip = [Version]"26.2.0"
}
$script:dryRunOperations = New-Object System.Collections.Generic.List[object]

function Write-Step {
  param([Parameter(Mandatory = $true)][string]$Message)

  Write-Host ""
  Write-Host "==> $Message" -ForegroundColor Cyan
}

function Add-DryRunOperation {
  param(
    [Parameter(Mandatory = $true)][string]$Phase,
    [Parameter(Mandatory = $true)][string]$Kind,
    [Parameter(Mandatory = $true)][string]$Executable,
    [string[]]$ArgumentList = @(),
    [bool]$MutatesMachine = $false,
    [bool]$MutatesRepository = $false,
    [AllowNull()][string]$TargetPath = $null,
    [bool]$RequiresElevation = $false
  )

  $script:dryRunOperations.Add([pscustomobject][ordered]@{
      phase = $Phase
      kind = $Kind
      executable = $Executable
      arguments = @($ArgumentList)
      mutatesMachine = $MutatesMachine
      mutatesRepository = $MutatesRepository
      targetPath = $TargetPath
      requiresElevation = $RequiresElevation
    })
}

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

function Resolve-NativeCommand {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [string]$ExplicitPath
  )

  if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
    $resolvedExplicitPath = (Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop).Path
    if (-not (Test-Path -LiteralPath $resolvedExplicitPath -PathType Leaf)) {
      throw "$Name was not found at the explicit path: $ExplicitPath"
    }

    return $resolvedExplicitPath
  }

  $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -eq $command) {
    throw "$Name was not found."
  }

  return $command.Source
}

function Get-CanonicalSevenZipCandidate {
  $programFiles = [Environment]::GetFolderPath(
    [Environment+SpecialFolder]::ProgramFiles
  )
  if ([string]::IsNullOrWhiteSpace($programFiles)) {
    throw "Could not resolve the canonical 64-bit Program Files directory."
  }

  return [System.IO.Path]::GetFullPath(
    (Join-Path $programFiles "7-Zip\7z.exe")
  )
}

function Assert-SevenZipVersion {
  param([Parameter(Mandatory = $true)][string]$Output)

  $match = [regex]::Match($Output, '(?m)^7-Zip\s+(\d+)\.(\d+)(?:\.(\d+))?(?:\s|$)')
  if (-not $match.Success) {
    throw "Could not parse the canonical 7-Zip version."
  }
  $patch = if ($match.Groups[3].Success) { [int]$match.Groups[3].Value } else { 0 }
  $version = New-Object Version -ArgumentList @(
    [int]$match.Groups[1].Value,
    [int]$match.Groups[2].Value,
    $patch
  )
  if ($version -lt $minimumToolVersions.sevenZip) {
    throw "Canonical 7-Zip $version is too old. Required >= $($minimumToolVersions.sevenZip)."
  }

  return $version
}

function Test-IsAdministrator {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-ValidatedWorkspaceRoot {
  param([Parameter(Mandatory = $true)][string]$Path)

  $resolvedRoot = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
  if (-not (Test-Path -LiteralPath (Join-Path $resolvedRoot "package.json") -PathType Leaf)) {
    throw "Workspace marker package.json was not found under: $resolvedRoot"
  }
  if (-not (Test-Path -LiteralPath (Join-Path $resolvedRoot "src-tauri\Cargo.toml") -PathType Leaf)) {
    throw "Workspace marker src-tauri/Cargo.toml was not found under: $resolvedRoot"
  }

  return [IO.Path]::GetFullPath($resolvedRoot).TrimEnd('\')
}

function Assert-SafeCleanTarget {
  param(
    [Parameter(Mandatory = $true)][string]$ResolvedWorkspaceRoot,
    [Parameter(Mandatory = $true)][string]$Candidate
  )

  $rootWithSeparator = $ResolvedWorkspaceRoot.TrimEnd('\') + '\'
  $candidateFullPath = [IO.Path]::GetFullPath($Candidate).TrimEnd('\')
  if (-not $candidateFullPath.StartsWith($rootWithSeparator, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to clean a path outside the validated workspace: $candidateFullPath"
  }

  $relativePath = $candidateFullPath.Substring($rootWithSeparator.Length)
  if ([string]::IsNullOrWhiteSpace($relativePath)) {
    throw "Refusing to clean the workspace root."
  }

  $cursor = $ResolvedWorkspaceRoot
  foreach ($segment in ($relativePath -split '[\\/]')) {
    if ([string]::IsNullOrWhiteSpace($segment)) {
      continue
    }

    $cursor = Join-Path $cursor $segment
    if (Test-Path -LiteralPath $cursor) {
      $item = Get-Item -LiteralPath $cursor -Force
      if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to clean through a reparse point: $cursor"
      }
    }
  }

  return $candidateFullPath
}

function Add-PathIfPresent {
  param(
    [Parameter(Mandatory = $true)][string]$Candidate,
    [switch]$Prepend
  )

  if (-not (Test-Path -LiteralPath $Candidate -PathType Container)) {
    return
  }

  $currentPaths = @($env:Path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  if ($currentPaths -contains $Candidate) {
    if ($Prepend) {
      $remainingPaths = @($currentPaths | Where-Object { $_ -ine $Candidate })
      $env:Path = (@($Candidate) + $remainingPaths) -join ';'
    }
    return
  }

  if ([string]::IsNullOrWhiteSpace($env:Path)) {
    $env:Path = $Candidate
  } elseif ($Prepend) {
    $env:Path = "$Candidate;$env:Path"
  } else {
    $env:Path = "$env:Path;$Candidate"
  }
}

function Refresh-ProcessPath {
  param([switch]$PreferInstalledDefaults)

  $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  $mergedPaths = New-Object System.Collections.Generic.List[string]
  $combinedPathText = @($env:Path, $machinePath, $userPath) -join ';'
  foreach ($candidate in ($combinedPathText -split ';')) {
    if ([string]::IsNullOrWhiteSpace($candidate) -or $mergedPaths.Contains($candidate)) {
      continue
    }
    $mergedPaths.Add($candidate)
  }
  $env:Path = $mergedPaths -join ';'

  Add-PathIfPresent -Candidate "$env:ProgramFiles\nodejs" -Prepend:$PreferInstalledDefaults
  Add-PathIfPresent -Candidate "$env:USERPROFILE\.cargo\bin" -Prepend:$PreferInstalledDefaults
  Add-PathIfPresent -Candidate "C:\msys64\usr\bin" -Prepend:$PreferInstalledDefaults
  Add-PathIfPresent -Candidate "C:\msys64\mingw64\bin" -Prepend:$PreferInstalledDefaults
}

function Get-PowerShellExecutable {
  $windowsPowerShell = Join-Path $PSHOME "powershell.exe"
  if (Test-Path -LiteralPath $windowsPowerShell -PathType Leaf) {
    return $windowsPowerShell
  }

  return Resolve-NativeCommand -Name "powershell.exe"
}

function Quote-StartProcessArgument {
  param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Value)

  if ($Value -notmatch '[\s"]') {
    return $Value
  }

  return '"' + ($Value -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}

function Get-DependencyInstallerChildArguments {
  param([Parameter(Mandatory = $true)][string]$ResolvedWorkspaceRoot)

  $childArguments = @(
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-File", $PSCommandPath,
    "-WorkspaceRoot", $ResolvedWorkspaceRoot,
    "-InstallDependenciesOnly"
  )

  if (-not [string]::IsNullOrWhiteSpace($BashPath)) {
    $childArguments += @("-BashPath", $BashPath)
  }
  if (-not [string]::IsNullOrWhiteSpace($GpgPath)) {
    $childArguments += @("-GpgPath", $GpgPath)
  }
  if (-not [string]::IsNullOrWhiteSpace($GpgvPath)) {
    $childArguments += @("-GpgvPath", $GpgvPath)
  }
  if (-not [string]::IsNullOrWhiteSpace($VsWherePath)) {
    $childArguments += @("-VsWherePath", $VsWherePath)
  }

  return $childArguments
}

function Invoke-DependencyInstallerChild {
  param([Parameter(Mandatory = $true)][string]$ResolvedWorkspaceRoot)

  $powershellPath = Get-PowerShellExecutable
  $childArguments = @(Get-DependencyInstallerChildArguments -ResolvedWorkspaceRoot $ResolvedWorkspaceRoot)

  $quotedArguments = @($childArguments | ForEach-Object { Quote-StartProcessArgument -Value ([string]$_) })
  Write-Step "Installing machine dependencies in an elevated child process"
  $process = Start-Process -FilePath $powershellPath -ArgumentList $quotedArguments -Verb RunAs -Wait -PassThru -WindowStyle Hidden
  if ($process.ExitCode -ne 0) {
    throw "The elevated dependency installer failed with exit code $($process.ExitCode)."
  }
}

function Ensure-WingetPackage {
  param(
    [Parameter(Mandatory = $true)][string]$WingetPath,
    [Parameter(Mandatory = $true)][string]$Id,
    [Parameter(Mandatory = $true)][string]$Label,
    [string[]]$ExtraArguments = @()
  )

  Write-Step "Installing or repairing $Label"
  $arguments = @(
    "install",
    "--id", $Id,
    "--exact",
    "--silent",
    "--accept-package-agreements",
    "--accept-source-agreements"
  ) + $ExtraArguments
  Invoke-CheckedNative -FilePath $WingetPath -ArgumentList $arguments -Description "winget install for $Label"
}

function Resolve-MsysTool {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [string]$ExplicitPath,
    [Parameter(Mandatory = $true)][string[]]$Candidates
  )

  if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
    return Resolve-NativeCommand -Name $Name -ExplicitPath $ExplicitPath
  }

  foreach ($candidate in $Candidates) {
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
  }

  return Resolve-NativeCommand -Name $Name
}

function Ensure-RustToolchain {
  param([Parameter(Mandatory = $true)][string]$RustupPath)

  Write-Step "Installing exact Rust release toolchain $requiredRustToolchain"
  Invoke-CheckedNative -FilePath $RustupPath -ArgumentList @(
    "toolchain", "install", $requiredRustToolchain,
    "--profile", "minimal",
    "--component", "rustfmt",
    "--component", "clippy"
  ) -Description "rustup toolchain install"
  Invoke-CheckedNative -FilePath $RustupPath -ArgumentList @(
    "default", $requiredRustToolchain
  ) -Description "rustup default"
}

function Ensure-MsysPackages {
  param([Parameter(Mandatory = $true)][string]$ResolvedBashPath)

  Write-Step "Updating the complete MSYS2 installation (pass 1)"
  Invoke-CheckedNative -FilePath $ResolvedBashPath -ArgumentList @(
    "-lc", "pacman -Syu --noconfirm"
  ) -Description "MSYS2 full update pass 1"

  # A new bash process is intentional. MSYS2 core runtime updates can require
  # the first process to terminate before the full update is completed.
  Write-Step "Updating the complete MSYS2 installation (pass 2)"
  Invoke-CheckedNative -FilePath $ResolvedBashPath -ArgumentList @(
    "-lc", "pacman -Syu --noconfirm"
  ) -Description "MSYS2 full update pass 2"

  Write-Step "Installing MSYS2 build and signature-verification packages"
  Invoke-CheckedNative -FilePath $ResolvedBashPath -ArgumentList @(
    "-lc",
    "pacman -S --needed --noconfirm diffutils make pkgconf nasm gnupg mingw-w64-x86_64-binutils mingw-w64-x86_64-gcc"
  ) -Description "MSYS2 package installation"
}

function Ensure-CargoAudit {
  param([Parameter(Mandatory = $true)][string]$CargoPath)

  Write-Step "Installing cargo-audit $requiredCargoAuditVersion"
  Invoke-CheckedNative -FilePath $CargoPath -ArgumentList @(
    "+$requiredRustToolchain",
    "install", "cargo-audit",
    "--version", $requiredCargoAuditVersion,
    "--locked"
  ) -Description "cargo install cargo-audit"
}

function Install-Dependencies {
  $wingetPath = Resolve-NativeCommand -Name "winget.exe"
  Ensure-WingetPackage -WingetPath $wingetPath -Id "OpenJS.NodeJS.LTS" -Label "Node.js LTS"
  Ensure-WingetPackage -WingetPath $wingetPath -Id "Rustlang.Rustup" -Label "Rustup"
  Ensure-WingetPackage -WingetPath $wingetPath -Id "MSYS2.MSYS2" -Label "MSYS2"
  Ensure-WingetPackage -WingetPath $wingetPath -Id "7zip.7zip" -Label "7-Zip"
  Ensure-WingetPackage -WingetPath $wingetPath -Id "Microsoft.EdgeWebView2Runtime" -Label "Microsoft Edge WebView2 Runtime"
  Ensure-WingetPackage -WingetPath $wingetPath -Id "Microsoft.VisualStudio.2022.BuildTools" -Label "Visual Studio Build Tools 2022" -ExtraArguments @(
    "--override",
    "--wait --quiet --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  )

  Refresh-ProcessPath -PreferInstalledDefaults
  $rustupPath = Resolve-NativeCommand -Name "rustup.exe"
  Ensure-RustToolchain -RustupPath $rustupPath

  $resolvedBashPath = Resolve-MsysTool -Name "bash.exe" -ExplicitPath $BashPath -Candidates @(
    "C:\msys64\usr\bin\bash.exe"
  )
  Ensure-MsysPackages -ResolvedBashPath $resolvedBashPath

  Refresh-ProcessPath -PreferInstalledDefaults
  $cargoPath = Resolve-NativeCommand -Name "cargo.exe"
  Ensure-CargoAudit -CargoPath $cargoPath
}

function ConvertTo-Version {
  param(
    [Parameter(Mandatory = $true)][string]$Value,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $match = [regex]::Match($Value, '(?<!\d)(\d+)\.(\d+)\.(\d+)(?!\d)')
  if (-not $match.Success) {
    throw "Could not parse the $Label version from: $Value"
  }

  return New-Object Version -ArgumentList @(
    [int]$match.Groups[1].Value,
    [int]$match.Groups[2].Value,
    [int]$match.Groups[3].Value
  )
}

function Assert-SupportedNodeVersion {
  param([Parameter(Mandatory = $true)][string]$NodeVersionOutput)

  $version = ConvertTo-Version -Value $NodeVersionOutput -Label "Node.js"
  $supported = (($version -ge [Version]"22.12.0") -and ($version -lt [Version]"23.0.0")) -or
    (($version -ge [Version]"24.0.0") -and ($version -lt [Version]"25.0.0"))
  if (-not $supported) {
    throw "Unsupported Node.js $version. Expected >=22.12.0 <23 or >=24.0.0 <25."
  }
}

function Assert-VersionPattern {
  param(
    [Parameter(Mandatory = $true)][string]$Output,
    [Parameter(Mandatory = $true)][string]$Pattern,
    [Parameter(Mandatory = $true)][string]$Label
  )

  if ($Output -notmatch $Pattern) {
    throw "$Label returned an unexpected version string: $Output"
  }
}

function Assert-MinimumVersion {
  param(
    [Parameter(Mandatory = $true)][string]$Output,
    [Parameter(Mandatory = $true)][Version]$MinimumVersion,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $version = ConvertTo-Version -Value $Output -Label $Label
  if ($version -lt $MinimumVersion) {
    throw "$Label $version is older than the required minimum $MinimumVersion."
  }
  return $version
}

function Get-VsWherePath {
  if (-not [string]::IsNullOrWhiteSpace($VsWherePath)) {
    return Resolve-NativeCommand -Name "vswhere.exe" -ExplicitPath $VsWherePath
  }

  $candidate = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  return Resolve-NativeCommand -Name "vswhere.exe" -ExplicitPath $candidate
}

function Get-VsDevCmdPath {
  param([Parameter(Mandatory = $true)][string]$ResolvedVsWherePath)

  $output = Invoke-CheckedNative -FilePath $ResolvedVsWherePath -ArgumentList @(
    "-latest",
    "-products", "*",
    "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
    "-find", "Common7\Tools\VsDevCmd.bat"
  ) -Description "vswhere VC tools lookup" -CaptureOutput
  $path = @($output -split "`r?`n" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -First 1)
  if ($path.Count -ne 1) {
    throw "Could not locate VsDevCmd.bat for Visual Studio Build Tools."
  }

  return (Resolve-Path -LiteralPath $path[0].Trim() -ErrorAction Stop).Path
}

function Import-VsDevEnvironment {
  param([Parameter(Mandatory = $true)][string]$VsDevCmdPath)

  Write-Step "Importing the Visual Studio x64 build environment"
  $commandLine = 'call "{0}" -arch=x64 -host_arch=x64 >nul && set' -f $VsDevCmdPath
  $output = Invoke-CheckedNative -FilePath "$env:SystemRoot\System32\cmd.exe" -ArgumentList @(
    "/d", "/s", "/c", $commandLine
  ) -Description "Visual Studio environment import" -CaptureOutput

  foreach ($line in ($output -split "`r?`n")) {
    if ($line -match '^([^=]+)=(.*)$') {
      [Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
    }
  }
}

function ConvertTo-NonZeroWebView2Version {
  param([AllowNull()][AllowEmptyString()][string]$Value)

  if ([string]::IsNullOrWhiteSpace($Value)) {
    return $null
  }

  [Version]$parsedVersion = $null
  if (-not [Version]::TryParse($Value.Trim(), [ref]$parsedVersion)) {
    return $null
  }
  if ($parsedVersion -le [Version]"0.0.0.0") {
    return $null
  }

  return $parsedVersion
}

function Get-WebView2RegistryInstallations {
  $clientId = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
  $subKeyPath = "SOFTWARE\Microsoft\EdgeUpdate\Clients\$clientId"
  $officialMachineWow64Path = "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$clientId"
  $machineObservedPath = if ([Environment]::Is64BitOperatingSystem) {
    $officialMachineWow64Path
  } else {
    $subKeyPath
  }
  $userView = if ([Environment]::Is64BitOperatingSystem) {
    [Microsoft.Win32.RegistryView]::Registry64
  } else {
    [Microsoft.Win32.RegistryView]::Registry32
  }
  $userViewLabel = if ([Environment]::Is64BitOperatingSystem) { "Registry64" } else { "Registry32" }
  $registryCandidates = @(
    [pscustomobject]@{
      Hive = [Microsoft.Win32.RegistryHive]::LocalMachine
      View = [Microsoft.Win32.RegistryView]::Registry32
      Source = "HKLM/Registry32/$machineObservedPath"
    },
    [pscustomobject]@{
      Hive = [Microsoft.Win32.RegistryHive]::CurrentUser
      View = $userView
      Source = "HKCU/$userViewLabel/$subKeyPath"
    }
  )

  $installations = New-Object System.Collections.Generic.List[object]
  foreach ($candidate in $registryCandidates) {
    $hive = $candidate.Hive
    $view = $candidate.View
    $baseKey = $null
    $clientKey = $null
    try {
      $baseKey = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
      # Registry32 maps the logical HKLM path to the documented WOW6432Node
      # location on 64-bit Windows. HKCU uses the native view and official path.
      $clientKey = $baseKey.OpenSubKey($subKeyPath, $false)
      if ($null -eq $clientKey -or $clientKey.GetValueNames() -notcontains "pv") {
        continue
      }
      if ($clientKey.GetValueKind("pv") -ne [Microsoft.Win32.RegistryValueKind]::String) {
        continue
      }

      $rawVersion = [string]$clientKey.GetValue(
        "pv",
        $null,
        [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
      )
      $version = ConvertTo-NonZeroWebView2Version -Value $rawVersion
      if ($null -eq $version) {
        continue
      }

      $installations.Add([pscustomobject]@{
          Version = $version
          Source = $candidate.Source
          MeetsMinimum = ($version -ge $minimumToolVersions.webView2)
        })
    }
    catch {
      Write-Verbose "Could not read WebView2 Runtime registration from $($candidate.Source): $($_.Exception.Message)"
    }
    finally {
      if ($null -ne $clientKey) {
        $clientKey.Dispose()
      }
      if ($null -ne $baseKey) {
        $baseKey.Dispose()
      }
    }
  }

  return @($installations)
}

function Get-WebView2Runtime {
  $registryInstallations = @(Get-WebView2RegistryInstallations)
  $selectedRegistration = $registryInstallations |
    Where-Object { $_.MeetsMinimum } |
    Sort-Object Version -Descending |
    Select-Object -First 1
  if ($null -ne $selectedRegistration) {
    return "registry:$($selectedRegistration.Source)@$($selectedRegistration.Version)"
  }

  $tooOldCandidates = New-Object System.Collections.Generic.List[object]
  foreach ($registration in $registryInstallations) {
    if (-not $registration.MeetsMinimum) {
      $tooOldCandidates.Add($registration)
    }
  }

  $roots = @(
    @(${env:ProgramFiles(x86)}, $env:ProgramFiles) |
      Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
      Select-Object -Unique |
      ForEach-Object { Join-Path $_ "Microsoft\EdgeWebView\Application" }
  )

  foreach ($root in $roots) {
    if (-not (Test-Path -LiteralPath $root -PathType Container)) {
      continue
    }

    $runtimes = @(Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
      ForEach-Object { Join-Path $_.FullName "msedgewebview2.exe" } |
      Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
    foreach ($runtime in $runtimes) {
      $runtimeItem = Get-Item -LiteralPath $runtime -ErrorAction Stop
      $versionText = [string]$runtimeItem.VersionInfo.ProductVersion
      $version = ConvertTo-NonZeroWebView2Version -Value $versionText
      if ($null -eq $version) {
        $versionText = $runtimeItem.Directory.Name
        $version = ConvertTo-NonZeroWebView2Version -Value $versionText
      }
      if ($null -eq $version) {
        continue
      }

      if ($version -ge $minimumToolVersions.webView2) {
        return $runtimeItem.FullName
      }
      $tooOldCandidates.Add([pscustomobject]@{
          Version = $version
          Source = "file/$($runtimeItem.FullName)"
          MeetsMinimum = $false
        })
    }
  }

  if ($tooOldCandidates.Count -gt 0) {
    $observedCandidates = @($tooOldCandidates |
        Sort-Object Source, Version |
        ForEach-Object { "$($_.Source)@$($_.Version)" }) -join "; "
    throw "Microsoft Edge WebView2 Runtime version is too old. Required >= $($minimumToolVersions.webView2). Observed candidates: $observedCandidates"
  }
  throw "Microsoft Edge WebView2 Runtime was not found in the official registry locations or Program Files fallback."
}

function Test-Dependencies {
  param([Parameter(Mandatory = $true)][string]$ResolvedWorkspaceRoot)

  Write-Step "Verifying exact toolchain and machine dependencies"
  Push-Location $ResolvedWorkspaceRoot
  try {
  $nodePath = Resolve-NativeCommand -Name "node.exe"
  $nodeVersion = Invoke-CheckedNative -FilePath $nodePath -ArgumentList @("--version") -Description "node --version" -CaptureOutput
  Assert-SupportedNodeVersion -NodeVersionOutput $nodeVersion
  $npmPath = Resolve-NativeCommand -Name "npm.cmd"
  $npmVersion = Invoke-CheckedNative -FilePath $npmPath -ArgumentList @("--version") -Description "npm --version" -CaptureOutput
  $null = Assert-MinimumVersion -Output $npmVersion -MinimumVersion $minimumToolVersions.npm -Label "npm"

  $rustupPath = Resolve-NativeCommand -Name "rustup.exe"
  $rustcPath = Resolve-NativeCommand -Name "rustc.exe"
  $cargoPath = Resolve-NativeCommand -Name "cargo.exe"
  $activeToolchain = Invoke-CheckedNative -FilePath $rustupPath -ArgumentList @("show", "active-toolchain") -Description "rustup active toolchain lookup" -CaptureOutput
  if ($activeToolchain -notmatch ('^' + [regex]::Escape($requiredRustToolchain) + '(?:\s|$)')) {
    throw "The active Rust toolchain is not exactly ${requiredRustToolchain}: $activeToolchain"
  }

  $rustcVersion = Invoke-CheckedNative -FilePath $rustcPath -ArgumentList @("--version") -Description "rustc --version" -CaptureOutput
  Assert-VersionPattern -Output $rustcVersion -Pattern '^rustc 1\.96\.1(?:\s|$)' -Label "rustc"
  $pinnedRustcVersion = Invoke-CheckedNative -FilePath $rustupPath -ArgumentList @("run", $requiredRustToolchain, "rustc", "--version") -Description "pinned rustc version lookup" -CaptureOutput
  if ($rustcVersion -ne $pinnedRustcVersion) {
    throw "rustc does not resolve from the exact pinned Rust toolchain."
  }
  $cargoVersion = Invoke-CheckedNative -FilePath $cargoPath -ArgumentList @("--version") -Description "cargo --version" -CaptureOutput
  $pinnedCargoVersion = Invoke-CheckedNative -FilePath $rustupPath -ArgumentList @("run", $requiredRustToolchain, "cargo", "--version") -Description "pinned cargo version lookup" -CaptureOutput
  if ($cargoVersion -ne $pinnedCargoVersion) {
    throw "cargo does not resolve from the exact pinned Rust toolchain."
  }
  $rustfmtVersion = Invoke-CheckedNative -FilePath $rustupPath -ArgumentList @("run", $requiredRustToolchain, "rustfmt", "--version") -Description "pinned rustfmt version lookup" -CaptureOutput
  Assert-VersionPattern -Output $rustfmtVersion -Pattern '^rustfmt 1\.\d+\.\d+(?:-|\s|$)' -Label "rustfmt"
  $clippyVersion = Invoke-CheckedNative -FilePath $rustupPath -ArgumentList @("run", $requiredRustToolchain, "clippy-driver", "--version") -Description "pinned clippy version lookup" -CaptureOutput
  Assert-VersionPattern -Output $clippyVersion -Pattern '^clippy \d+\.\d+\.\d+(?:\s|$)' -Label "clippy"

  $cargoAuditPath = Resolve-NativeCommand -Name "cargo-audit.exe"
  $cargoAuditVersion = Invoke-CheckedNative -FilePath $cargoAuditPath -ArgumentList @("--version") -Description "cargo-audit --version" -CaptureOutput
  if ($cargoAuditVersion -ne "cargo-audit $requiredCargoAuditVersion") {
    throw "cargo-audit must be exactly $requiredCargoAuditVersion; got: $cargoAuditVersion"
  }

  $sevenZipPath = Resolve-NativeCommand `
    -Name "7z.exe" `
    -ExplicitPath (Get-CanonicalSevenZipCandidate)
  $sevenZipVersionOutput = Invoke-CheckedNative `
    -FilePath $sevenZipPath `
    -ArgumentList @("i") `
    -Description "canonical 7-Zip version lookup" `
    -CaptureOutput
  $null = Assert-SevenZipVersion -Output $sevenZipVersionOutput

  $resolvedBashPath = Resolve-MsysTool -Name "bash.exe" -ExplicitPath $BashPath -Candidates @("C:\msys64\usr\bin\bash.exe")
  $resolvedGpgPath = Resolve-MsysTool -Name "gpg.exe" -ExplicitPath $GpgPath -Candidates @("C:\msys64\usr\bin\gpg.exe")
  $resolvedGpgvPath = Resolve-MsysTool -Name "gpgv.exe" -ExplicitPath $GpgvPath -Candidates @("C:\msys64\usr\bin\gpgv.exe")
  $gccPath = Resolve-MsysTool -Name "gcc.exe" -Candidates @("C:\msys64\mingw64\bin\gcc.exe")
  $pacmanPath = Resolve-MsysTool -Name "pacman.exe" -Candidates @("C:\msys64\usr\bin\pacman.exe")
  $ldPath = Resolve-MsysTool -Name "ld.exe" -Candidates @("C:\msys64\mingw64\bin\ld.exe")
  $stripPath = Resolve-MsysTool -Name "strip.exe" -Candidates @("C:\msys64\mingw64\bin\strip.exe")
  $cygpathPath = Resolve-MsysTool -Name "cygpath.exe" -Candidates @("C:\msys64\usr\bin\cygpath.exe")
  $makePath = Resolve-MsysTool -Name "make.exe" -Candidates @("C:\msys64\usr\bin\make.exe")
  $nasmPath = Resolve-MsysTool -Name "nasm.exe" -Candidates @("C:\msys64\usr\bin\nasm.exe")
  $pkgconfPath = Resolve-MsysTool -Name "pkgconf.exe" -Candidates @(
    "C:\msys64\mingw64\bin\pkgconf.exe",
    "C:\msys64\usr\bin\pkgconf.exe"
  )

  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $resolvedBashPath -ArgumentList @("--version") -Description "bash --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.bash -Label "bash"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $pacmanPath -ArgumentList @("--version") -Description "pacman --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.pacman -Label "pacman"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $gccPath -ArgumentList @("--version") -Description "gcc --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.gcc -Label "gcc"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $ldPath -ArgumentList @("--version") -Description "ld --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.binutils -Label "GNU ld"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $stripPath -ArgumentList @("--version") -Description "strip --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.binutils -Label "GNU strip"
  Assert-VersionPattern -Output (Invoke-CheckedNative -FilePath $cygpathPath -ArgumentList @("--version") -Description "cygpath --version" -CaptureOutput) -Pattern '(?i)cygpath.*\d+\.' -Label "cygpath"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $makePath -ArgumentList @("--version") -Description "make --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.make -Label "make"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $nasmPath -ArgumentList @("-v") -Description "nasm -v" -CaptureOutput) -MinimumVersion $minimumToolVersions.nasm -Label "nasm"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $pkgconfPath -ArgumentList @("--version") -Description "pkgconf --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.pkgconf -Label "pkgconf"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $resolvedGpgPath -ArgumentList @("--version") -Description "gpg --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.gnupg -Label "gpg"
  $null = Assert-MinimumVersion -Output (Invoke-CheckedNative -FilePath $resolvedGpgvPath -ArgumentList @("--version") -Description "gpgv --version" -CaptureOutput) -MinimumVersion $minimumToolVersions.gnupg -Label "gpgv"

  $resolvedVsWherePath = Get-VsWherePath
  $vsDevCmdPath = Get-VsDevCmdPath -ResolvedVsWherePath $resolvedVsWherePath
  $visualStudioVersion = Invoke-CheckedNative -FilePath $resolvedVsWherePath -ArgumentList @(
    "-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationVersion"
  ) -Description "Visual Studio Build Tools version lookup" -CaptureOutput
  $null = Assert-MinimumVersion -Output $visualStudioVersion -MinimumVersion $minimumToolVersions.visualStudio -Label "Visual Studio Build Tools"
  Import-VsDevEnvironment -VsDevCmdPath $vsDevCmdPath
  $dumpbinPath = Resolve-NativeCommand -Name "dumpbin.exe"
  $dumpbinVersion = Invoke-CheckedNative -FilePath $dumpbinPath -ArgumentList @("/?") -Description "dumpbin version lookup" -CaptureOutput
  $null = Assert-MinimumVersion -Output $dumpbinVersion -MinimumVersion $minimumToolVersions.msvcTools -Label "dumpbin"
  $null = Get-WebView2Runtime

  return [pscustomobject]@{
    BashPath = $resolvedBashPath
    GpgPath = $resolvedGpgPath
    GpgvPath = $resolvedGpgvPath
    SevenZipPath = $sevenZipPath
    PowerShellPath = Get-PowerShellExecutable
    NpmPath = $npmPath
  }
  }
  finally {
    Pop-Location
  }
}

function Invoke-RepositoryCommand {
  param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [Parameter(Mandatory = $true)][string[]]$ArgumentList,
    [Parameter(Mandatory = $true)][string]$Label
  )

  Write-Step $Label
  Invoke-CheckedNative -FilePath $Executable -ArgumentList $ArgumentList -Description $Label
}

function Add-DependencyInstallDryRunOperations {
  foreach ($package in @(
      [pscustomobject]@{ Id = "OpenJS.NodeJS.LTS"; ExtraArguments = @() },
      [pscustomobject]@{ Id = "Rustlang.Rustup"; ExtraArguments = @() },
      [pscustomobject]@{ Id = "MSYS2.MSYS2"; ExtraArguments = @() },
      [pscustomobject]@{ Id = "7zip.7zip"; ExtraArguments = @() },
      [pscustomobject]@{ Id = "Microsoft.EdgeWebView2Runtime"; ExtraArguments = @() },
      [pscustomobject]@{
        Id = "Microsoft.VisualStudio.2022.BuildTools"
        ExtraArguments = @(
          "--override",
          "--wait --quiet --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
        )
      }
    )) {
    $wingetArguments = @(
      "install", "--id", $package.Id, "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements"
    ) + @($package.ExtraArguments)
    Add-DryRunOperation -Phase "dependencies" -Kind "install" -Executable "winget.exe" -ArgumentList $wingetArguments -MutatesMachine $true -RequiresElevation $true
  }
  Add-DryRunOperation -Phase "dependencies" -Kind "toolchain-install" -Executable "rustup.exe" -ArgumentList @(
    "toolchain", "install", $requiredRustToolchain, "--profile", "minimal", "--component", "rustfmt", "--component", "clippy"
  ) -MutatesMachine $true -RequiresElevation $true
  Add-DryRunOperation -Phase "dependencies" -Kind "toolchain-select" -Executable "rustup.exe" -ArgumentList @(
    "default", $requiredRustToolchain
  ) -MutatesMachine $true -RequiresElevation $true
  Add-DryRunOperation -Phase "dependencies" -Kind "system-update" -Executable "bash.exe" -ArgumentList @(
    "-lc", "pacman -Syu --noconfirm"
  ) -MutatesMachine $true -RequiresElevation $true
  Add-DryRunOperation -Phase "dependencies" -Kind "system-update" -Executable "bash.exe" -ArgumentList @(
    "-lc", "pacman -Syu --noconfirm"
  ) -MutatesMachine $true -RequiresElevation $true
  Add-DryRunOperation -Phase "dependencies" -Kind "install" -Executable "bash.exe" -ArgumentList @(
    "-lc", "pacman -S --needed --noconfirm diffutils make pkgconf nasm gnupg mingw-w64-x86_64-binutils mingw-w64-x86_64-gcc"
  ) -MutatesMachine $true -RequiresElevation $true
  Add-DryRunOperation -Phase "dependencies" -Kind "install" -Executable "cargo.exe" -ArgumentList @(
    "+$requiredRustToolchain", "install", "cargo-audit", "--version", $requiredCargoAuditVersion, "--locked"
  ) -MutatesMachine $true -RequiresElevation $true
}

function Add-DependencyVerificationDryRunOperations {
  $resolvedBashLabel = if ($BashPath) { $BashPath } else { "bash.exe" }
  $resolvedGpgLabel = if ($GpgPath) { $GpgPath } else { "gpg.exe" }
  $resolvedGpgvLabel = if ($GpgvPath) { $GpgvPath } else { "gpgv.exe" }
  $resolvedVsWhereLabel = if ($VsWherePath) { $VsWherePath } else { "vswhere.exe" }
  $canonicalSevenZipLabel = Get-CanonicalSevenZipCandidate
  $webView2ClientId = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
  $userRegistryViewLabel = if ([Environment]::Is64BitOperatingSystem) { "Registry64" } else { "Registry32" }
  $registryReadCandidates = @(
    [pscustomobject]@{ Hive = "HKLM"; View = "Registry32" },
    [pscustomobject]@{ Hive = "HKCU"; View = $userRegistryViewLabel }
  )
  foreach ($registryCandidate in $registryReadCandidates) {
    Add-DryRunOperation -Phase "verify" -Kind "registry-read" -Executable "Microsoft.Win32.RegistryKey" -ArgumentList @(
      $registryCandidate.Hive,
      $registryCandidate.View,
      "SOFTWARE\Microsoft\EdgeUpdate\Clients\$webView2ClientId",
      "pv"
    )
  }

  foreach ($operation in @(
      [pscustomobject]@{ Executable = "node.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "npm.cmd"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "rustup.exe"; Arguments = @("show", "active-toolchain") },
      [pscustomobject]@{ Executable = "rustc.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "rustup.exe"; Arguments = @("run", $requiredRustToolchain, "rustc", "--version") },
      [pscustomobject]@{ Executable = "cargo.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "rustup.exe"; Arguments = @("run", $requiredRustToolchain, "cargo", "--version") },
      [pscustomobject]@{ Executable = "rustup.exe"; Arguments = @("run", $requiredRustToolchain, "rustfmt", "--version") },
      [pscustomobject]@{ Executable = "rustup.exe"; Arguments = @("run", $requiredRustToolchain, "clippy-driver", "--version") },
      [pscustomobject]@{ Executable = "cargo-audit.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = $canonicalSevenZipLabel; Arguments = @("i") },
      [pscustomobject]@{ Executable = $resolvedBashLabel; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "pacman.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "gcc.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "ld.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "strip.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "cygpath.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "make.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = "nasm.exe"; Arguments = @("-v") },
      [pscustomobject]@{ Executable = "pkgconf.exe"; Arguments = @("--version") },
      [pscustomobject]@{ Executable = $resolvedGpgLabel; Arguments = @("--version") },
      [pscustomobject]@{ Executable = $resolvedGpgvLabel; Arguments = @("--version") },
      [pscustomobject]@{ Executable = $resolvedVsWhereLabel; Arguments = @("-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-find", "Common7\Tools\VsDevCmd.bat") },
      [pscustomobject]@{ Executable = $resolvedVsWhereLabel; Arguments = @("-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationVersion") },
      [pscustomobject]@{ Executable = "cmd.exe"; Arguments = @("/d", "/s", "/c", "call <resolved VsDevCmd.bat> -arch=x64 -host_arch=x64 >nul && set") },
      [pscustomobject]@{ Executable = "dumpbin.exe"; Arguments = @("/?") }
    )) {
    Add-DryRunOperation -Phase "verify" -Kind "verify" -Executable $operation.Executable -ArgumentList @($operation.Arguments)
  }

  $webViewRoots = @(
    @(${env:ProgramFiles(x86)}, $env:ProgramFiles) |
      Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
      Select-Object -Unique |
      ForEach-Object { Join-Path $_ "Microsoft\EdgeWebView\Application" }
  )
  foreach ($webViewRoot in $webViewRoots) {
    Add-DryRunOperation -Phase "verify" -Kind "filesystem-read" -Executable "Get-ChildItem/Get-Item" -ArgumentList @(
      $webViewRoot,
      "msedgewebview2.exe",
      "VersionInfo.ProductVersion"
    )
  }
}

function Add-RepositoryDryRunOperations {
  param([Parameter(Mandatory = $true)][string]$ResolvedWorkspaceRoot)

  if ($Clean) {
    foreach ($candidate in @(
        (Join-Path $ResolvedWorkspaceRoot "dist"),
        (Join-Path $ResolvedWorkspaceRoot "src-tauri\target\release")
      )) {
      $safePath = Assert-SafeCleanTarget -ResolvedWorkspaceRoot $ResolvedWorkspaceRoot -Candidate $candidate
      Add-DryRunOperation -Phase "clean" -Kind "delete" -Executable "Remove-Item" -ArgumentList @(
        "-LiteralPath", $safePath, "-Recurse", "-Force"
      ) -MutatesRepository $true -TargetPath $safePath
    }
  }

  Add-DryRunOperation -Phase "repository" -Kind "repo-command" -Executable "npm.cmd" -ArgumentList @("ci") -MutatesRepository $true -TargetPath $ResolvedWorkspaceRoot
  $verifySourceArguments = @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", "tools/ffmpeg/build-minimal-ffmpeg.ps1",
    "-WorkspaceRoot", $ResolvedWorkspaceRoot,
    "-VerifySourceOnly"
  )
  if (-not [string]::IsNullOrWhiteSpace($GpgPath)) {
    $verifySourceArguments += @("-GpgPath", $GpgPath)
  }
  if (-not [string]::IsNullOrWhiteSpace($GpgvPath)) {
    $verifySourceArguments += @("-GpgvPath", $GpgvPath)
  }
  Add-DryRunOperation -Phase "repository" -Kind "repo-command" -Executable "powershell.exe" -ArgumentList $verifySourceArguments -MutatesMachine $true -TargetPath (Join-Path $env:TEMP "stickerfit-ffmpeg-cache")
  Add-DryRunOperation -Phase "repository" -Kind "repo-command" -Executable "powershell.exe" -ArgumentList @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", "tools/ffmpeg/build-minimal-ffmpeg.ps1",
    "-WorkspaceRoot", $ResolvedWorkspaceRoot,
    "-VerifyVendorArtifacts"
  )

  Add-DryRunOperation -Phase "repository" -Kind "repo-command" -Executable "npm.cmd" -ArgumentList @("run", "build:desktop") -MutatesRepository $true -TargetPath (Join-Path $ResolvedWorkspaceRoot "src-tauri\target")
  if ($RunChecks) {
    Add-DryRunOperation -Phase "repository" -Kind "repo-command" -Executable "npm.cmd" -ArgumentList @("run", "check:quality") -MutatesRepository $true -TargetPath $ResolvedWorkspaceRoot
    Add-DryRunOperation -Phase "repository" -Kind "repo-command" -Executable "npm.cmd" -ArgumentList @("run", "test:release-sidecar")
  }
  Add-DryRunOperation -Phase "repository" -Kind "repo-command" -Executable "npm.cmd" -ArgumentList @("run", "report:size") -MutatesRepository $true -TargetPath (Join-Path $ResolvedWorkspaceRoot "src-tauri\target\release\size-report.json")
}

function Write-DryRunTranscript {
  param(
    [Parameter(Mandatory = $true)][string]$Mode,
    [Parameter(Mandatory = $true)][string]$ResolvedWorkspaceRoot
  )

  [pscustomobject][ordered]@{
    schemaVersion = 1
    dryRun = $true
    mode = $Mode
    workspaceRoot = $ResolvedWorkspaceRoot
    operations = $script:dryRunOperations.ToArray()
  } | ConvertTo-Json -Depth 8
}

if ($InstallDependenciesOnly -and ($SkipDependencyInstall -or $RunChecks -or $Clean)) {
  throw "-InstallDependenciesOnly cannot be combined with -SkipDependencyInstall, -RunChecks, or -Clean."
}

$WorkspaceRoot = Get-ValidatedWorkspaceRoot -Path $WorkspaceRoot

if ($DryRun) {
  if ($InstallDependenciesOnly) {
    Add-DependencyInstallDryRunOperations
    Write-DryRunTranscript -Mode "InstallDependenciesOnly" -ResolvedWorkspaceRoot $WorkspaceRoot
    exit 0
  }

  if (-not $SkipDependencyInstall) {
    $installerArguments = @(Get-DependencyInstallerChildArguments -ResolvedWorkspaceRoot $WorkspaceRoot)
    Add-DryRunOperation -Phase "elevation" -Kind "elevate" -Executable "powershell.exe" -ArgumentList $installerArguments -MutatesMachine $true -RequiresElevation $true
  }
  Add-DependencyVerificationDryRunOperations
  Add-RepositoryDryRunOperations -ResolvedWorkspaceRoot $WorkspaceRoot
  $mode = if ($SkipDependencyInstall) { "VerifyOnly" } else { "Normal" }
  Write-DryRunTranscript -Mode $mode -ResolvedWorkspaceRoot $WorkspaceRoot
  exit 0
}

$isAdministrator = Test-IsAdministrator
if ($InstallDependenciesOnly) {
  if (-not $isAdministrator) {
    throw "-InstallDependenciesOnly is internal and must be launched by the non-admin parent with elevation."
  }

  Install-Dependencies
  exit 0
}

if ($isAdministrator) {
  throw "Repository work must not run elevated. Re-open a non-admin PowerShell shell and run the bootstrap again."
}

if (-not $SkipDependencyInstall) {
  Invoke-DependencyInstallerChild -ResolvedWorkspaceRoot $WorkspaceRoot
}

Refresh-ProcessPath -PreferInstalledDefaults:(-not $SkipDependencyInstall)
$tools = Test-Dependencies -ResolvedWorkspaceRoot $WorkspaceRoot

if ($Clean) {
  Write-Step "Cleaning validated release output descendants"
  foreach ($candidate in @(
      (Join-Path $WorkspaceRoot "dist"),
      (Join-Path $WorkspaceRoot "src-tauri\target\release")
    )) {
    $safePath = Assert-SafeCleanTarget -ResolvedWorkspaceRoot $WorkspaceRoot -Candidate $candidate
    if (Test-Path -LiteralPath $safePath) {
      Remove-Item -LiteralPath $safePath -Recurse -Force
    }
  }
}

Push-Location $WorkspaceRoot
try {
  Invoke-RepositoryCommand -Executable $tools.NpmPath -ArgumentList @("ci") -Label "Installing locked npm dependencies"

  $verifySourceArguments = @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", (Join-Path $WorkspaceRoot "tools\ffmpeg\build-minimal-ffmpeg.ps1"),
    "-WorkspaceRoot", $WorkspaceRoot,
    "-VerifySourceOnly",
    "-GpgPath", $tools.GpgPath,
    "-GpgvPath", $tools.GpgvPath
  )
  Invoke-RepositoryCommand -Executable $tools.PowerShellPath -ArgumentList $verifySourceArguments -Label "Verifying the signed FFmpeg source archive"
  Invoke-RepositoryCommand -Executable $tools.PowerShellPath -ArgumentList @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", (Join-Path $WorkspaceRoot "tools\ffmpeg\build-minimal-ffmpeg.ps1"),
    "-WorkspaceRoot", $WorkspaceRoot,
    "-VerifyVendorArtifacts"
  ) -Label "Verifying the tracked FFmpeg vendor payload"

  Invoke-RepositoryCommand -Executable $tools.NpmPath -ArgumentList @("run", "build:desktop") -Label "Building the desktop bundle with locked Cargo dependencies"
  if ($RunChecks) {
    Invoke-RepositoryCommand -Executable $tools.NpmPath -ArgumentList @("run", "check:quality") -Label "Running quality gates"
    Invoke-RepositoryCommand -Executable $tools.NpmPath -ArgumentList @("run", "test:release-sidecar") -Label "Verifying the packaged FFmpeg sidecar"
  }
  Invoke-RepositoryCommand -Executable $tools.NpmPath -ArgumentList @("run", "report:size") -Label "Reporting release artifact sizes"

  $nsisPath = Get-ChildItem -LiteralPath (Join-Path $WorkspaceRoot "src-tauri\target\release\bundle\nsis") -Filter "StickerFit_*_x64-setup.exe" -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTimeUtc -Descending |
    Select-Object -First 1 -ExpandProperty FullName
  $desktopPath = Join-Path $WorkspaceRoot "src-tauri\target\release\desktop.exe"

  Write-Step "Build completed"
  Write-Host "desktop.exe : $desktopPath"
  Write-Host "NSIS bundle : $nsisPath"
}
finally {
  Pop-Location
}
