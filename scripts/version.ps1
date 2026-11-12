<#
.SYNOPSIS
  Print the workspace version: the [workspace.package] version of the root Cargo.toml.

.DESCRIPTION
  The first line starting with `version =` in the root Cargo.toml is the one in
  [workspace.package] (dependency versions are never at the start of a line). Used by
  dist.ps1, update-manifest.ps1 and the release workflow.
#>
$ErrorActionPreference = 'Stop'
$toml = Join-Path (Split-Path -Parent $PSScriptRoot) 'Cargo.toml'
foreach ($line in Get-Content -LiteralPath $toml) {
    if ($line -match '^version\s*=\s*"([^"]+)"') { return $Matches[1] }
}
Write-Error "no version line found in $toml"
exit 1
