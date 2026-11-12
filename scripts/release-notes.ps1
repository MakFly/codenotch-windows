<#
.SYNOPSIS
  Extract the CHANGELOG.md section of one version, for the GitHub release notes.

.DESCRIPTION
  Prints (or writes to -Out, UTF-8 without BOM) the lines between `## <version>` and the next
  `## ` heading, without the heading itself. Fails when the version has no section. Used by the
  release workflow.
#>
[CmdletBinding()]
param(
    [string] $Version = '',
    [string] $Out = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Version) { $Version = & (Join-Path $PSScriptRoot 'version.ps1') }

$lines = Get-Content -LiteralPath (Join-Path $root 'CHANGELOG.md') -Encoding UTF8
$start = -1
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match ('^##\s+v?' + [regex]::Escape($Version) + '(\s|$)')) { $start = $i + 1; break }
}
if ($start -lt 0) {
    Write-Host "error: CHANGELOG.md has no '## $Version' section" -ForegroundColor Red
    exit 1
}
$end = $lines.Count
for ($i = $start; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match '^##\s') { $end = $i; break }
}
$body = ($lines[$start..($end - 1)] -join "`n").Trim()
if ($Out) {
    [IO.File]::WriteAllText([IO.Path]::GetFullPath((Join-Path (Get-Location).Path $Out)), $body + "`n", (New-Object Text.UTF8Encoding $false))
} else {
    $body
}
