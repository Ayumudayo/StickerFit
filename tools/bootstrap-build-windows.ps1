param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
  [string]$FfmpegVersion = "7.1.3",
  [switch]$SkipDependencyInstall,
  [switch]$RunChecks,
  [switch]$Clean
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Write-Step {
  param([string]$Message)
  Write-Host ""
  Write-Host "==> $Message" -ForegroundColor Cyan
}

function Test-IsAdministrator {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = [Security.Principal.WindowsPrincipal]::new($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Ensure-Administrator {
  if ($SkipDependencyInstall) {
    return
  }

  if (Test-IsAdministrator) {
    return
  }

  $argList = @(
    "-ExecutionPolicy", "Bypass",
    "-File", "`"$PSCommandPath`"",
    "-WorkspaceRoot", "`"$WorkspaceRoot`"",
    "-FfmpegVersion", "`"$FfmpegVersion`""
  )

  if ($RunChecks) {
    $argList += "-RunChecks"
  }

  if ($Clean) {
    $argList += "-Clean"
  }

  Write-Host "Re-launching with administrator privileges to install build dependencies..."
  $process = Start-Process -FilePath "powershell.exe" -ArgumentList $argList -Verb RunAs -Wait -PassThru
  exit $process.ExitCode
}

function Test-WingetPackageInstalled {
  param([string]$Id)

  $output = winget list --id $Id --exact --accept-source-agreements 2>$null | Out-String
  return $output -match [regex]::Escape($Id)
}

function Ensure-Winget {
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw "winget is required for the clean-machine bootstrap script."
  }
}

function Ensure-WingetPackage {
  param(
    [string]$Id,
    [string]$Label,
    [string[]]$ExtraArgs = @()
  )

  if (Test-WingetPackageInstalled -Id $Id) {
    Write-Host "$Label is already installed."
    return
  }

  Write-Step "Installing $Label"
  $arguments = @(
    "install",
    "--id", $Id,
    "--exact",
    "--silent",
    "--accept-package-agreements",
    "--accept-source-agreements"
  ) + $ExtraArgs

  & winget @arguments
  if ($LASTEXITCODE -ne 0) {
    throw "winget failed while installing $Label ($Id)."
  }
}

function Add-PathIfPresent {
  param([string]$Candidate)

  if (-not (Test-Path $Candidate)) {
    return
  }

  $current = $env:Path -split ';'
  if ($current -contains $Candidate) {
    return
  }

  $env:Path = "$Candidate;$env:Path"
}

function Refresh-ProcessPath {
  Add-PathIfPresent -Candidate "$env:ProgramFiles\nodejs"
  Add-PathIfPresent -Candidate "$env:USERPROFILE\.cargo\bin"
  Add-PathIfPresent -Candidate "C:\msys64\usr\bin"
  Add-PathIfPresent -Candidate "C:\msys64\mingw64\bin"
  Add-PathIfPresent -Candidate "$env:LOCALAPPDATA\bin\NASM"
}

function Ensure-RustToolchain {
  Write-Step "Ensuring Rust stable MSVC toolchain"
  & rustup toolchain install stable-x86_64-pc-windows-msvc
  if ($LASTEXITCODE -ne 0) {
    throw "rustup failed to install the stable MSVC toolchain."
  }

  & rustup default stable-x86_64-pc-windows-msvc
  if ($LASTEXITCODE -ne 0) {
    throw "rustup failed to select the stable MSVC toolchain."
  }
}

function Ensure-MsysPackages {
  param([string]$BashPath)

  Write-Step "Ensuring MSYS2 build packages"
  $packageScript = @"
set -euo pipefail
pacman -Sy --noconfirm
pacman -S --needed --noconfirm diffutils make pkgconf nasm mingw-w64-x86_64-gcc
"@

  & $BashPath -lc $packageScript
  if ($LASTEXITCODE -ne 0) {
    throw "MSYS2 package installation failed."
  }
}

function Get-VsDevCmdPath {
  $vswherePath = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  if (-not (Test-Path $vswherePath)) {
    throw "vswhere.exe was not found. Visual Studio Build Tools installation may be incomplete."
  }

  $path = & $vswherePath -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find Common7\Tools\VsDevCmd.bat | Select-Object -First 1
  if (-not $path) {
    throw "Could not locate VsDevCmd.bat for Visual Studio Build Tools."
  }

  return $path.Trim()
}

function Import-VsDevEnvironment {
  param([string]$VsDevCmdPath)

  Write-Step "Importing Visual Studio build environment"
  $cmd = "`"$VsDevCmdPath`" -arch=x64 -host_arch=x64 >nul && set"
  $output = & cmd.exe /d /s /c $cmd
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to import the Visual Studio build environment."
  }

  foreach ($line in $output) {
    if ($line -match '^(.*?)=(.*)$') {
      [Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
    }
  }
}

function Invoke-RepoCommand {
  param([string]$Command)

  Write-Step "Running: $Command"
  & cmd.exe /d /s /c $Command
  if ($LASTEXITCODE -ne 0) {
    throw "Command failed: $Command"
  }
}

Ensure-Administrator
Ensure-Winget

if (-not $SkipDependencyInstall) {
  Ensure-WingetPackage -Id "OpenJS.NodeJS.LTS" -Label "Node.js LTS"
  Ensure-WingetPackage -Id "Rustlang.Rustup" -Label "Rustup"
  Ensure-WingetPackage -Id "MSYS2.MSYS2" -Label "MSYS2"
  Ensure-WingetPackage -Id "Microsoft.EdgeWebView2Runtime" -Label "Microsoft Edge WebView2 Runtime"
  Ensure-WingetPackage -Id "Microsoft.VisualStudio.2022.BuildTools" -Label "Visual Studio Build Tools 2022" -ExtraArgs @(
    "--override",
    "--wait --quiet --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  )
}

Refresh-ProcessPath

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  throw "node was not found after dependency setup."
}

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
  throw "rustup was not found after dependency setup."
}

$bashPath = "C:\msys64\usr\bin\bash.exe"
if (-not (Test-Path $bashPath)) {
  throw "MSYS2 bash was not found at $bashPath."
}

Ensure-RustToolchain
Ensure-MsysPackages -BashPath $bashPath

$vsDevCmdPath = Get-VsDevCmdPath
Import-VsDevEnvironment -VsDevCmdPath $vsDevCmdPath

if ($Clean) {
  Write-Step "Cleaning previous build artifacts"
  $pathsToRemove = @(
    (Join-Path $WorkspaceRoot "dist"),
    (Join-Path $WorkspaceRoot "src-tauri\target\release")
  )

  foreach ($path in $pathsToRemove) {
    if (Test-Path $path) {
      Remove-Item -Path $path -Recurse -Force
    }
  }
}

Push-Location $WorkspaceRoot
try {
  Invoke-RepoCommand -Command "npm ci"
  Invoke-RepoCommand -Command "powershell -ExecutionPolicy Bypass -File tools/ffmpeg/build-minimal-ffmpeg.ps1 -WorkspaceRoot ""$WorkspaceRoot"" -FfmpegVersion ""$FfmpegVersion"""
  Invoke-RepoCommand -Command "npm run build:desktop"

  if ($RunChecks) {
    Invoke-RepoCommand -Command "npm run test:release-sidecar"
    Invoke-RepoCommand -Command "npm run test:unit"
    Invoke-RepoCommand -Command "npm run test:rust"
    Invoke-RepoCommand -Command "npm run test:web-smoke"
  }

  Invoke-RepoCommand -Command "npm run report:size"

  $nsisPath = Get-ChildItem (Join-Path $WorkspaceRoot "src-tauri\target\release\bundle\nsis") -Filter "StickerFit_*_x64-setup.exe" -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1 -ExpandProperty FullName
  $desktopPath = Join-Path $WorkspaceRoot "src-tauri\target\release\desktop.exe"

  Write-Step "Build completed"
  Write-Host "desktop.exe : $desktopPath"
  Write-Host "NSIS bundle : $nsisPath"
}
finally {
  Pop-Location
}
