param(
  [string]$ToolsRoot = $PSScriptRoot
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($PSVersionTable.PSEdition -ne "Desktop" -or $PSVersionTable.PSVersion.Major -ne 5 -or $PSVersionTable.PSVersion.Minor -lt 1) {
  throw "This gate must run with Windows PowerShell 5.1 (powershell.exe), not PowerShell Core."
}

$resolvedToolsRoot = (Resolve-Path -LiteralPath $ToolsRoot -ErrorAction Stop).Path
$files = @(
  Get-ChildItem -LiteralPath $resolvedToolsRoot -Recurse -File |
    Where-Object { $_.Extension -in @(".ps1", ".psm1") } |
    Sort-Object FullName
)
if ($files.Count -eq 0) {
  throw "No PowerShell files were found under: $resolvedToolsRoot"
}

$allErrors = New-Object System.Collections.Generic.List[object]
foreach ($file in $files) {
  $tokens = $null
  $fileErrors = $null
  [void][System.Management.Automation.Language.Parser]::ParseFile(
    $file.FullName,
    [ref]$tokens,
    [ref]$fileErrors
  )

  foreach ($parseError in @($fileErrors)) {
    $allErrors.Add([pscustomobject]@{
        File = $file.FullName
        Line = $parseError.Extent.StartLineNumber
        Column = $parseError.Extent.StartColumnNumber
        Message = $parseError.Message
      })
  }
}

if ($allErrors.Count -gt 0) {
  foreach ($parseError in $allErrors) {
    Write-Error -ErrorAction Continue ("{0}:{1}:{2}: {3}" -f $parseError.File, $parseError.Line, $parseError.Column, $parseError.Message)
  }
  throw "Windows PowerShell 5.1 parsing failed for $($allErrors.Count) error(s)."
}

Write-Host "Windows PowerShell 5.1 parsed $($files.Count) tool script(s) without errors."
