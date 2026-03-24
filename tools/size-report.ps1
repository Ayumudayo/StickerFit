param(
  [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"

function Get-SizeEntry {
  param(
    [string]$Label,
    [string]$Path
  )

  if (-not (Test-Path $Path)) {
    return [pscustomobject]@{
      Label = $Label
      Path = $Path
      SizeMB = $null
    }
  }

  $item = Get-Item $Path
  $size = if ($item.PSIsContainer) {
    (Get-ChildItem $Path -Recurse -File | Measure-Object -Property Length -Sum).Sum
  } else {
    $item.Length
  }

  [pscustomobject]@{
    Label = $Label
    Path = $item.FullName
    SizeMB = [math]::Round(($size / 1MB), 2)
  }
}

function Get-CombinedSizeEntry {
  param(
    [string]$Label,
    [string[]]$Paths
  )

  $resolved = @()
  $total = 0
  foreach ($path in $Paths) {
    if (-not (Test-Path $path)) {
      continue
    }

    $item = Get-Item $path
    $resolved += $item.FullName
    $total += if ($item.PSIsContainer) {
      (Get-ChildItem $path -Recurse -File | Measure-Object -Property Length -Sum).Sum
    } else {
      $item.Length
    }
  }

  [pscustomobject]@{
    Label = $Label
    Path = ($resolved -join "; ")
    SizeMB = if ($resolved.Count -gt 0) { [math]::Round(($total / 1MB), 2) } else { $null }
  }
}

$entries = @(
  (Get-CombinedSizeEntry "Estimated installed footprint" @(
    (Join-Path $WorkspaceRoot "src-tauri\target\release\desktop.exe"),
    (Join-Path $WorkspaceRoot "src-tauri\binaries")
  )),
  (Get-SizeEntry "desktop.exe" (Join-Path $WorkspaceRoot "src-tauri\target\release\desktop.exe")),
  (Get-SizeEntry "ffmpeg sidecar" (Join-Path $WorkspaceRoot "src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe")),
  (Get-SizeEntry "dist" (Join-Path $WorkspaceRoot "dist")),
  (Get-SizeEntry "NSIS bundle" (Join-Path $WorkspaceRoot "src-tauri\target\release\bundle\nsis\StickerFit_0.1.0_x64-setup.exe")),
  (Get-SizeEntry "MSI bundle" (Join-Path $WorkspaceRoot "src-tauri\target\release\bundle\msi\StickerFit_0.1.0_x64_en-US.msi"))
)

$entries |
  Select-Object Label, SizeMB, Path |
  Format-Table -AutoSize
