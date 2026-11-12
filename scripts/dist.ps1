<#
.SYNOPSIS
  Package the release build into dist\CodeNotch-<version>\ and dist\CodeNotch-<version>.zip.

.DESCRIPTION
  Used by `make dist` and by the release workflow (.github/workflows/release.yml), so the
  package is built the same way everywhere. It does not build: run
  `cargo build --release -p codenotch -p codenotch-hook` first.

  The folder holds codenotch.exe, codenotch-hook.exe, LICENSE, README.md and the provider-mark
  NOTICE.md. The zip contains the CodeNotch-<version> folder itself. Entries are added one by
  one with a `/` separator: in PowerShell 5.1 both Compress-Archive and
  ZipFile.CreateFromDirectory write `\`, which the zip format does not allow and some unzip
  tools turn into odd file names.

  Prints the zip path, size and SHA-256 at the end.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dist.ps1
#>
[CmdletBinding()]
param(
    # Default: the [workspace.package] version of the root Cargo.toml.
    [string] $Version = '',
    # Cargo target folder holding release\codenotch.exe (default: CARGO_TARGET_DIR or target).
    [string] $TargetDir = '',
    [string] $OutDir = 'dist'
)

$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

if (-not $Version) { $Version = & (Join-Path $PSScriptRoot 'version.ps1') }
if (-not $TargetDir) { $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' } }

$release = Join-Path $TargetDir 'release'
$exes = @((Join-Path $release 'codenotch.exe'), (Join-Path $release 'codenotch-hook.exe'))
foreach ($f in $exes) {
    if (-not (Test-Path -LiteralPath $f)) {
        Write-Host "error: $f not found. Build it first: cargo build --release -p codenotch -p codenotch-hook" -ForegroundColor Red
        exit 1
    }
}

$name = "CodeNotch-$Version"
$d = Join-Path $OutDir $name
$z = "$d.zip"
if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force }
if (Test-Path -LiteralPath $z) { Remove-Item -LiteralPath $z -Force }
New-Item -ItemType Directory -Path $d | Out-Null

Copy-Item -LiteralPath ($exes + @('LICENSE', 'README.md')) -Destination $d
$notice = 'crates\ui-notch\assets\glyphs\NOTICE.md'
if (Test-Path -LiteralPath $notice) {
    Copy-Item -LiteralPath $notice -Destination (Join-Path $d 'NOTICE.md')
} else {
    Write-Host "warning: $notice not found, not included" -ForegroundColor Yellow
}

Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::Open([IO.Path]::GetFullPath($z), 'Create')
try {
    foreach ($f in Get-ChildItem -LiteralPath $d -File | Sort-Object Name) {
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $f.FullName, "$name/$($f.Name)", 'Optimal')
    }
} finally {
    $zip.Dispose()
}

Write-Host "Created $d\"
Get-ChildItem -LiteralPath $d | ForEach-Object { '  {0,-24} {1,12:N0} bytes' -f $_.Name, $_.Length }
$item = Get-Item -LiteralPath $z
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $z).Hash.ToLower()
Write-Host ('Created {0} ({1:N0} bytes)' -f $z, $item.Length)
Write-Host "SHA-256 $hash"
