<#
.SYNOPSIS
  Watch mode: rebuild the workspace and restart the debug app on every save. Used by `make dev`.

.DESCRIPTION
  1. Refuses to start (exit 1) if a codenotch.exe from the target folder is already running
     (e.g. your own `make run`): the single-instance lock would make the dev instance exit, and
     the running exe locks the file the build has to relink. It is never killed: quit it or run
     `make stop`. With --demo (no single-instance lock) an exe running from another profile
     folder is allowed.
  2. cargo build --workspace (app + hook, the hook has to sit next to the app), then starts
     <target>\debug\codenotch.exe with -AppArgs. The app shares this console (no pipe), so its
     log lines keep their colours.
  3. Watches crates\ (recursive), Cargo.toml, Cargo.lock and .cargo\. A burst of saves is
     collected until 500 ms pass without a new one, then handled once: the app started by this
     script (and only that one, by PID) is stopped, the workspace is rebuilt, and on success the
     app is started again. On a failed build it stays stopped until the next save.
  4. If the app exits by itself (crash, Quit), the script keeps watching and starts it on the
     next change.
  Ctrl+C stops the app it started and the watchers. No dependency besides cargo and PowerShell.

  Status lines are coloured unless NO_COLOR is set. Build jobs come from .cargo\config.toml.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev.ps1 -AppArgs '--demo' -Safe
#>
[CmdletBinding()]
param(
    [string] $Cargo = 'cargo',
    # Cargo target folder (make passes CARGO_TARGET_DIR or `target`); relative = from the repo root.
    [string] $TargetDir = '',
    # Arguments for codenotch.exe, as one string (make passes ARGS).
    [string] $AppArgs = '',
    # Sets CODENOTCH_NO_SPAWN=1 for the app only (make passes it for SAFE=1).
    [switch] $Safe,
    # Quiet time that ends a burst of file events.
    [int] $DebounceMs = 500
)

$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture
if ([Console]::IsOutputRedirected) { [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false }

$Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $Root
if (-not $TargetDir) { $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' } }
if (-not [IO.Path]::IsPathRooted($TargetDir)) { $TargetDir = Join-Path $Root $TargetDir }
$TargetDir = [IO.Path]::GetFullPath($TargetDir).TrimEnd('\')
$Exe = Join-Path $TargetDir 'debug\codenotch.exe'
$IsDemo = $AppArgs -match '(^|\s)--demo(\s|$)'
$UseColor = -not $env:NO_COLOR

function Say([string] $Text, [ConsoleColor] $Color = [ConsoleColor]::Cyan) {
    if ($UseColor) { Write-Host "[dev] $Text" -ForegroundColor $Color } else { Write-Host "[dev] $Text" }
}

# codenotch*.exe processes running from the target folder, minus $Except. Win32_Process because
# a 32-bit PowerShell (started by the 32-bit make) cannot read a 64-bit process path otherwise.
function Get-TargetProcs([int] $Except = 0) {
    $prefix = $TargetDir + '\'
    @(Get-CimInstance Win32_Process -Filter "Name='codenotch.exe' OR Name='codenotch-hook.exe'" |
        Where-Object { $_.ProcessId -ne $Except -and $_.ExecutablePath -and
            $_.ExecutablePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) })
}

# --- Relevant file events ----------------------------------------------------------------------
$Extensions = @('.rs', '.toml', '.lock', '.svg', '.png', '.ico', '.json', '.wgsl', '.hlsl',
    '.glsl', '.ttf', '.otf', '.rc', '.manifest', '.txt', '.md', '.ftl')
function Test-Relevant([string] $Path) {
    if (-not $Path) { return $false }
    $rel = $Path
    if ($rel.StartsWith($Root + '\', [StringComparison]::OrdinalIgnoreCase)) { $rel = $rel.Substring($Root.Length + 1) }
    if ($rel -match '(^|\\)(target|dist|\.git)(\\|$)') { return $false }
    $name = [IO.Path]::GetFileName($rel)
    if ($name -eq '4913' -or $name.EndsWith('~') -or $name.StartsWith('.#') -or $name.StartsWith('~$')) { return $false }
    $ext = [IO.Path]::GetExtension($name).ToLowerInvariant()
    if ($ext -in '.swp', '.swo', '.swx', '.tmp', '.bak', '.orig') { return $false }
    if ($rel -ieq 'Cargo.toml' -or $rel -ieq 'Cargo.lock') { return $true }
    if ($rel -like '.cargo\*') { return $true }
    if ($rel -like 'crates\*') { return $Extensions -contains $ext }
    $false
}
function Get-RelPath([string] $Path) {
    if ($Path.StartsWith($Root + '\', [StringComparison]::OrdinalIgnoreCase)) { $Path.Substring($Root.Length + 1) } else { $Path }
}

# --- App process (only the one this script started) --------------------------------------------
$script:Child = $null
function Start-App {
    if (-not (Test-Path -LiteralPath $Exe)) { Say "$Exe not found" Red; return }
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = $AppArgs
    $psi.WorkingDirectory = $Root
    $psi.UseShellExecute = $false   # inherit this console: no pipe, the app keeps its colours
    if ($Safe) { $psi.EnvironmentVariables['CODENOTCH_NO_SPAWN'] = '1' }
    $script:Child = [Diagnostics.Process]::Start($psi)
    $what = (@('codenotch.exe', $AppArgs) | Where-Object { $_ }) -join ' '
    $extra = if ($Safe) { ', CODENOTCH_NO_SPAWN=1' } else { '' }
    Say ("started $what (PID " + $script:Child.Id + "$extra)") Green
}
function Stop-App {
    $c = $script:Child
    $script:Child = $null
    if (-not $c) { return }
    try {
        if (-not $c.HasExited) {
            Say ('stopping codenotch.exe (PID ' + $c.Id + ')') DarkGray
            $c.Kill()
            if (-not $c.WaitForExit(10000)) { Say ('PID ' + $c.Id + ' did not exit within 10 s') Yellow }
        }
    } catch { Say ('could not stop PID ' + $c.Id + ': ' + $_.Exception.Message) Yellow }
    finally { $c.Dispose() }
}

# cargo is started as a plain child sharing this console (not `& cargo`): when this script's
# own output is redirected, PowerShell 5.1 would turn cargo's stderr progress lines into errors.
$script:Build = $null
function Invoke-Build {
    Say "$Cargo build --workspace" DarkGray
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $Cargo
    $psi.Arguments = 'build --workspace'
    $psi.WorkingDirectory = $Root
    $psi.UseShellExecute = $false
    try { $script:Build = [Diagnostics.Process]::Start($psi) }
    catch { Say ("cannot start ${Cargo}: " + $_.Exception.Message) Red; return $false }
    # Short waits keep Ctrl+C responsive (a single WaitForExit() cannot be interrupted).
    while (-not $script:Build.WaitForExit(200)) { }
    $code = $script:Build.ExitCode
    $script:Build.Dispose(); $script:Build = $null
    if ($code -eq 0) { return $true }
    foreach ($p in Get-TargetProcs) {
        Say ('note: ' + $p.ExecutablePath + ' is running (PID ' + $p.ProcessId + ') and locks its file. Close it or run: make stop') Yellow
    }
    Say 'build failed, waiting for the next save' Red
    $false
}

# --- Refuse to fight a running instance --------------------------------------------------------
$running = @(Get-TargetProcs | Where-Object { $_.Name -eq 'codenotch.exe' })
$blocking = @($running | Where-Object { -not $IsDemo -or $_.ExecutablePath -ieq $Exe })
if ($blocking.Count -gt 0) {
    $p = $blocking[0]
    Say ('codenotch.exe is already running from ' + $p.ExecutablePath + ' (PID ' + $p.ProcessId + ').') Red
    if ($IsDemo) { Say 'It locks the exe that every rebuild has to relink.' Red }
    else { Say 'Its single-instance lock would make the dev instance exit, and it locks the exe a rebuild relinks.' Red }
    Say 'Quit it (tray > Quit CodeNotch) or run: make stop. It was not touched.' Red
    exit 1
}
if (-not $IsDemo) {
    $other = @(Get-CimInstance Win32_Process -Filter "Name='codenotch.exe'")
    if ($other.Count -gt 0) {
        Say ('note: CodeNotch is running (PID ' + $other[0].ProcessId + '); without --demo the dev instance hands over to it and exits. Use ARGS=--demo or quit it.') Yellow
    }
}

# --- Watchers ----------------------------------------------------------------------------------
$watchers = New-Object Collections.Generic.List[IO.FileSystemWatcher]
$sources = New-Object Collections.Generic.List[string]
$tag = 'cndev-' + $PID + '-'
function Add-Watcher([string] $Dir, [bool] $Recurse) {
    if (-not (Test-Path -LiteralPath $Dir -PathType Container)) { return }
    $w = New-Object IO.FileSystemWatcher
    $w.Path = $Dir
    $w.IncludeSubdirectories = $Recurse
    $w.NotifyFilter = [IO.NotifyFilters]'FileName, DirectoryName, LastWrite, Size'
    $w.InternalBufferSize = 65536
    foreach ($ev in 'Changed', 'Created', 'Deleted', 'Renamed') {
        $id = $tag + $watchers.Count + '-' + $ev
        Register-ObjectEvent -InputObject $w -EventName $ev -SourceIdentifier $id | Out-Null
        $sources.Add($id)
    }
    $w.EnableRaisingEvents = $true
    $watchers.Add($w)
}

# Relevant paths among the queued events (consumed).
function Read-Events {
    $paths = @()
    foreach ($e in @(Get-Event | Where-Object { $_.SourceIdentifier -like "$tag*" })) {
        Remove-Event -EventIdentifier $e.EventIdentifier
        $a = $e.SourceEventArgs
        foreach ($p in @($a.FullPath, $(if ($a -is [IO.RenamedEventArgs]) { $a.OldFullPath }))) {
            if (Test-Relevant $p) { $paths += $p }
        }
    }
    $paths
}

$exitCode = 0
try {
    Add-Watcher (Join-Path $Root 'crates') $true
    Add-Watcher $Root $false          # Cargo.toml, Cargo.lock (other root files are filtered out)
    Add-Watcher (Join-Path $Root '.cargo') $true
    Say ("watching crates\, Cargo.toml, Cargo.lock, .cargo\ (debounce $DebounceMs ms). Ctrl+C to stop.") Cyan

    if (Invoke-Build) { Start-App }

    while ($true) {
        # Wait for a relevant change, noticing an app that exits by itself.
        $changed = @()
        while ($changed.Count -eq 0) {
            Wait-Event -Timeout 1 | Out-Null
            $changed = @(Read-Events)
            if ($script:Child -and $script:Child.HasExited) {
                $code = $script:Child.ExitCode
                $script:Child.Dispose(); $script:Child = $null
                Say "app exited (code $code), waiting for a change" Yellow
            }
            if ($changed.Count -eq 0) { Start-Sleep -Milliseconds 100 }
        }
        # Debounce: keep collecting until $DebounceMs pass with no new relevant event.
        $quietSince = [DateTime]::UtcNow
        while (([DateTime]::UtcNow - $quietSince).TotalMilliseconds -lt $DebounceMs) {
            Start-Sleep -Milliseconds 100
            $more = @(Read-Events)
            if ($more.Count -gt 0) { $changed += $more; $quietSince = [DateTime]::UtcNow }
        }
        $names = @($changed | ForEach-Object { Get-RelPath $_ } | Select-Object -Unique)
        $shown = ($names | Select-Object -First 3) -join ', '
        if ($names.Count -gt 3) { $shown += (' (+' + ($names.Count - 3) + ' more)') }
        Say ("change: $shown " + [char]0x2192 + ' rebuilding') Magenta

        Stop-App
        if (Invoke-Build) {
            # Events raised while building (another save) trigger the next round.
            Start-App
        }
    }
}
finally {
    if ($script:Build) { try { if (-not $script:Build.HasExited) { $script:Build.Kill() } } catch { } }
    Stop-App
    foreach ($id in $sources) { Unregister-Event -SourceIdentifier $id -ErrorAction SilentlyContinue }
    Get-Event | Where-Object { $_.SourceIdentifier -like "$tag*" } | Remove-Event -ErrorAction SilentlyContinue
    foreach ($w in $watchers) { $w.EnableRaisingEvents = $false; $w.Dispose() }
    Say 'stopped' DarkGray
}
exit $exitCode
