<#
.SYNOPSIS
  Write the self-update manifest (update.json) for dist\CodeNotch-<version>.zip.

.DESCRIPTION
  Used by `make update-manifest` and by the release workflow. Fills in the version, the zip's
  size and SHA-256, the GitHub release URLs and today's date. min_version is kept from the
  current manifest (-Base, default update.json; the version itself when there is none), the
  notes only when the version did not change. -MinVersion and -Notes override both.

  Publish the zip BEFORE pushing the manifest: a manifest pointing at a missing file makes
  every update fail.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\update-manifest.ps1
#>
[CmdletBinding()]
param(
    [string] $Version = '',
    # Default: dist\CodeNotch-<version>.zip
    [string] $Zip = '',
    [string] $Out = 'update.json',
    # Existing manifest whose min_version and notes are kept (default: update.json).
    [string] $Base = 'update.json',
    [string] $MinVersion = '',
    [string] $Notes = '',
    [string] $Repo = 'MakFly/codenotch-windows'
)

$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

if (-not $Version) { $Version = & (Join-Path $PSScriptRoot 'version.ps1') }
if (-not $Zip) { $Zip = "dist\CodeNotch-$Version.zip" }
if (-not (Test-Path -LiteralPath $Zip)) {
    Write-Host "error: $Zip not found. Run: make dist" -ForegroundColor Red
    exit 1
}

$old = $null
if ($Base -and (Test-Path -LiteralPath $Base)) {
    $old = Get-Content -LiteralPath $Base -Raw -Encoding UTF8 | ConvertFrom-Json
}
$min = $Version
if ($old -and $old.min_version) { $min = [string]$old.min_version }
if ($MinVersion) { $min = $MinVersion }
$text = "CodeNotch for Windows $Version."
if ($old -and $old.version -eq $Version -and $old.notes) { $text = [string]$old.notes }
if ($Notes) { $text = $Notes }

$tag = "v$Version"
$base = "https://github.com/$Repo"
$asset = [ordered]@{
    url    = "$base/releases/download/$tag/CodeNotch-$Version.zip"
    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Zip).Hash.ToLower()
    size   = (Get-Item -LiteralPath $Zip).Length
}
# Written by hand rather than with ConvertTo-Json, whose PowerShell 5.1 layout is irregular:
# 2-space indent, LF line ends, no BOM. Each string value is escaped by ConvertTo-Json.
function Q([string] $s) { ConvertTo-Json -InputObject $s -Compress }
$json = @(
    '{'
    '  "version": ' + (Q $Version) + ','
    '  "min_version": ' + (Q $min) + ','
    '  "published_at": ' + (Q (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')) + ','
    '  "notes": ' + (Q $text) + ','
    '  "notes_url": ' + (Q "$base/releases/tag/$tag") + ','
    '  "windows_x64": {'
    '    "url": ' + (Q $asset.url) + ','
    '    "sha256": ' + (Q $asset.sha256) + ','
    '    "size": ' + $asset.size
    '  }'
    '}'
) -join "`n"
$null = $json | ConvertFrom-Json  # sanity check
$path = [IO.Path]::Combine((Get-Location).Path, $Out)
[IO.File]::WriteAllText($path, $json + "`n", (New-Object Text.UTF8Encoding $false))
Write-Host "Wrote $Out for $Version`: $($asset.size) bytes, sha256 $($asset.sha256)"
