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

$nsisBundle = Get-ChildItem (Join-Path $WorkspaceRoot "src-tauri\target\release\bundle\nsis") -Filter "StickerFit_*_x64-setup.exe" -File -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1
$nsisBundlePath = if ($nsisBundle) {
  $nsisBundle.FullName
} else {
  Join-Path $WorkspaceRoot "src-tauri\target\release\bundle\nsis"
}

$entries = @(
  (Get-CombinedSizeEntry "Estimated installed footprint" @(
    (Join-Path $WorkspaceRoot "src-tauri\target\release\desktop.exe"),
    (Join-Path $WorkspaceRoot "src-tauri\target\release\ffmpeg.exe"),
    (Join-Path $WorkspaceRoot "src-tauri\target\release\libwinpthread-1.dll")
  )),
  (Get-SizeEntry "desktop.exe" (Join-Path $WorkspaceRoot "src-tauri\target\release\desktop.exe")),
  (Get-SizeEntry "packaged ffmpeg sidecar" (Join-Path $WorkspaceRoot "src-tauri\target\release\ffmpeg.exe")),
  (Get-SizeEntry "runtime DLL" (Join-Path $WorkspaceRoot "src-tauri\target\release\libwinpthread-1.dll")),
  (Get-SizeEntry "dist" (Join-Path $WorkspaceRoot "dist")),
  (Get-SizeEntry "NSIS bundle" $nsisBundlePath)
)

$entries |
  Select-Object Label, SizeMB, Path |
  Format-Table -AutoSize
