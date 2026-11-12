# CodeNotch for Windows - one-line installer.
#
#   irm https://raw.githubusercontent.com/MakFly/codenotch-windows/main/install.ps1 | iex
#
# Per-user, no admin. Installs codenotch.exe and codenotch-hook.exe to
# %LOCALAPPDATA%\Programs\CodeNotch, adds a Start Menu shortcut and starts the app. The app then
# updates itself; running this again also updates (or repairs) the install.
#
# Options (environment variables, read when piped into iex; or parameters when run as a file):
#   CODENOTCH_INSTALL_DIR   -InstallDir    install folder
#   CODENOTCH_VERSION       -Version       install this release (e.g. 0.1.0) instead of the latest
#   CODENOTCH_NO_LAUNCH=1   -NoLaunch      do not start the app afterwards
#   CODENOTCH_NO_SHORTCUT=1 -NoShortcut    no Start Menu shortcut
#   CODENOTCH_UNINSTALL=1   -Uninstall     remove the app (your settings and data are kept)
#   CODENOTCH_SHORTCUT_DIR  -ShortcutDir   folder for the shortcut (default: your Start Menu)
#   CODENOTCH_MANIFEST_URL  -ManifestUrl   update manifest (URL or local path; for testing)
#
# Uninstall:
#   $env:CODENOTCH_UNINSTALL=1; irm https://raw.githubusercontent.com/MakFly/codenotch-windows/main/install.ps1 | iex
#
# As a file (if the execution policy blocks scripts):
#   powershell -ExecutionPolicy Bypass -File install.ps1 [-Uninstall] [-Version 0.1.0] ...

function Install-CodeNotch {
    [CmdletBinding()]
    param(
        [string]$InstallDir,
        [string]$Version,
        [string]$ShortcutDir,
        [string]$ManifestUrl,
        [switch]$NoLaunch,
        [switch]$NoShortcut,
        [switch]$Uninstall
    )

    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is many times faster without it
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }

    $repo = 'MakFly/codenotch-windows'
    $oneLiner = "irm https://raw.githubusercontent.com/$repo/main/install.ps1 | iex"
    if (-not $ManifestUrl) { $ManifestUrl = "https://raw.githubusercontent.com/$repo/main/update.json" }
    if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\CodeNotch' }
    if (-not $ShortcutDir) { $ShortcutDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs' }
    $InstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')
    $shortcut = Join-Path $ShortcutDir 'CodeNotch.lnk'
    $exeNames = @('codenotch.exe', 'codenotch-hook.exe')
    $extraNames = @('LICENSE', 'NOTICE.md')
    $dataDir = Join-Path $env:APPDATA 'dev.kev.smashtoken'

    function Say([string]$msg, [string]$color = 'Gray') { Write-Host $msg -ForegroundColor $color }
    function Fail([string]$msg) { Write-Host ''; Write-Host "error: $msg" -ForegroundColor Red; throw "CodeNotch install failed: $msg" }

    # Explains a failed web request in plain words.
    function Describe-WebError($err, [string]$what) {
        $resp = $null
        try { $resp = $err.Exception.Response } catch { }
        $code = 0
        if ($resp) { try { $code = [int]$resp.StatusCode } catch { } }
        if ($code -eq 403 -or $code -eq 429) {
            return "$what was refused by GitHub (HTTP $code). This is usually the API rate limit for anonymous requests (60 per hour): wait a while and try again."
        }
        if ($code -eq 404) { return "$what failed: not found (HTTP 404). Check that the version exists on the releases page." }
        if ($code -gt 0) { return "$what failed (HTTP $code)." }
        return "$what failed: $($err.Exception.Message) Check your internet connection (and proxy settings)."
    }

    function Get-LocalPath([string]$src) {
        if ($src -match '^file:') { return ([Uri]$src).LocalPath }
        if ($src -notmatch '^[a-z][a-z0-9+.-]*://') { return $src }
        return $null
    }

    function Get-Json([string]$src, [string]$what) {
        $local = Get-LocalPath $src
        if ($local) { return (Get-Content -LiteralPath $local -Raw | ConvertFrom-Json) }
        try { return Invoke-RestMethod -Uri $src -UseBasicParsing -Headers @{ 'User-Agent' = 'codenotch-installer' } }
        catch { Fail (Describe-WebError $_ $what) }
    }

    function Get-Text([string]$src, [string]$what) {
        $local = Get-LocalPath $src
        if ($local) { return (Get-Content -LiteralPath $local -Raw) }
        try { $c = (Invoke-WebRequest -Uri $src -UseBasicParsing -Headers @{ 'User-Agent' = 'codenotch-installer' }).Content }
        catch { Fail (Describe-WebError $_ $what) }
        # Windows PowerShell returns binary content types (octet-stream) as bytes.
        if ($c -is [byte[]]) { return [Text.Encoding]::UTF8.GetString($c) }
        return [string]$c
    }

    function Save-File([string]$src, [string]$dest, [string]$what) {
        $local = Get-LocalPath $src
        if ($local) { Copy-Item -LiteralPath $local -Destination $dest -Force; return }
        try { Invoke-WebRequest -Uri $src -OutFile $dest -UseBasicParsing -Headers @{ 'User-Agent' = 'codenotch-installer' } }
        catch { Fail (Describe-WebError $_ $what) }
    }

    function Get-ShaFromFile([string]$text) {
        if ($text -match '\b([0-9a-fA-F]{64})\b') { return $Matches[1].ToLowerInvariant() }
        Fail 'the published .sha256 file has no SHA-256 in it.'
    }

    # CodeNotch processes started from the install folder (never touches other copies).
    function Get-InstalledProcesses {
        @(Get-Process -Name 'codenotch', 'codenotch-hook' -ErrorAction SilentlyContinue | Where-Object {
            $p = $null
            try { $p = $_.Path } catch { }
            $p -and ([IO.Path]::GetDirectoryName($p).TrimEnd('\') -ieq $InstallDir)
        })
    }

    function Stop-Installed([string]$why) {
        $procs = Get-InstalledProcesses
        if ($procs.Count -eq 0) { return $false }
        Say "CodeNotch is running from $InstallDir; closing it $why." 'Yellow'
        foreach ($p in $procs) {
            try { Stop-Process -Id $p.Id -Force -ErrorAction Stop } catch { }
            try { [void]$p.WaitForExit(10000) } catch { }
        }
        Start-Sleep -Milliseconds 300   # let Windows release the file locks
        if ((Get-InstalledProcesses).Count -gt 0) {
            Fail "CodeNotch is still running from $InstallDir. Quit it from its tray icon and run the command again."
        }
        return $true
    }

    # ------------------------------------------------------------------------------------------
    # Uninstall
    # ------------------------------------------------------------------------------------------
    if ($Uninstall) {
        Say 'Uninstalling CodeNotch...' 'Cyan'
        [void](Stop-Installed 'before removing it')
        $removed = 0
        if (Test-Path -LiteralPath $InstallDir -PathType Container) {
            $names = $exeNames + $extraNames
            foreach ($n in $names) {
                $f = Join-Path $InstallDir $n
                if (Test-Path -LiteralPath $f -PathType Leaf) { Remove-Item -LiteralPath $f -Force; $removed++ }
            }
            # Leftovers of the app's own self-updater (codenotch*.old / .new).
            Get-ChildItem -LiteralPath $InstallDir -File -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -like 'codenotch*.old' -or $_.Name -like 'codenotch*.new' } |
                ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force; $removed++ }
            if (-not (Get-ChildItem -LiteralPath $InstallDir -Force -ErrorAction SilentlyContinue)) {
                Remove-Item -LiteralPath $InstallDir -Force
                Say "Removed $InstallDir" 'Green'
            } else {
                Say "Removed $removed file(s); $InstallDir still holds other files, so it was left in place." 'Yellow'
            }
        } else {
            Say "Nothing installed in $InstallDir." 'Yellow'
        }

        if (Test-Path -LiteralPath $shortcut -PathType Leaf) {
            Remove-Item -LiteralPath $shortcut -Force
            Say "Removed the Start Menu shortcut." 'Green'
        }

        # "Open CodeNotch at login": the app writes HKCU\...\Run, value "CodeNotch". Remove it only
        # when it points at the codenotch.exe being uninstalled.
        $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
        $cmd = $null
        try { $cmd = (Get-ItemProperty -LiteralPath $runKey -Name 'CodeNotch' -ErrorAction Stop).CodeNotch } catch { }
        if ($cmd) {
            $target = (Join-Path $InstallDir 'codenotch.exe')
            if ($cmd -like "*$target*") {
                Remove-ItemProperty -LiteralPath $runKey -Name 'CodeNotch'
                Say 'Removed the "Open at login" entry.' 'Green'
            }
        }

        Write-Host ''
        Say 'CodeNotch is uninstalled.' 'Green'
        Say "Your settings and data are kept in $dataDir. To delete them too:"
        Say "  Remove-Item -Recurse -Force `"$dataDir`"" 'White'
        Say 'If you installed the Claude Code hooks from the app, remove them from ~\.claude\settings.json.'
        return
    }

    # ------------------------------------------------------------------------------------------
    # Install
    # ------------------------------------------------------------------------------------------
    Say 'Installing CodeNotch for Windows...' 'Cyan'

    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    if ($arch -eq 'ARM64') {
        Say 'This is an ARM64 PC. Only an x64 build exists; Windows runs it through emulation.' 'Yellow'
    } elseif ($arch -ne 'AMD64') {
        Fail "CodeNotch needs 64-bit Windows (this PC reports $arch)."
    }

    $url = $null; $sha = $null; $size = $null; $ver = $null
    if ($Version) {
        $ver = $Version.TrimStart('v', 'V')
        $url = "https://github.com/$repo/releases/download/v$ver/CodeNotch-$ver.zip"
        Say "Version $ver requested."
        $sha = Get-ShaFromFile (Get-Text "$url.sha256" "Downloading the checksum of version $ver")
    } else {
        $manifest = $null
        try {
            $local = Get-LocalPath $ManifestUrl
            if ($local) { $manifest = Get-Content -LiteralPath $local -Raw | ConvertFrom-Json }
            else { $manifest = Invoke-RestMethod -Uri $ManifestUrl -UseBasicParsing -Headers @{ 'User-Agent' = 'codenotch-installer' } }
        } catch {
            Say "Could not read the update manifest ($($_.Exception.Message)); asking the GitHub API instead." 'Yellow'
        }
        if ($manifest -and $manifest.windows_x64 -and $manifest.windows_x64.url) {
            $ver = [string]$manifest.version
            $url = [string]$manifest.windows_x64.url
            $sha = ([string]$manifest.windows_x64.sha256).ToLowerInvariant()
            if ($manifest.windows_x64.size) { $size = [long]$manifest.windows_x64.size }
        } else {
            $rel = Get-Json "https://api.github.com/repos/$repo/releases/latest" 'Looking up the latest release'
            $ver = ([string]$rel.tag_name).TrimStart('v', 'V')
            $asset = @($rel.assets | Where-Object { $_.name -like 'CodeNotch-*.zip' }) | Select-Object -First 1
            $shaAsset = @($rel.assets | Where-Object { $_.name -like 'CodeNotch-*.zip.sha256' }) | Select-Object -First 1
            if (-not $asset -or -not $shaAsset) { Fail "release $($rel.tag_name) has no CodeNotch zip and .sha256 asset." }
            $url = [string]$asset.browser_download_url
            $size = [long]$asset.size
            $sha = Get-ShaFromFile (Get-Text $shaAsset.browser_download_url 'Downloading the checksum')
        }
    }
    if ($sha -notmatch '^[0-9a-f]{64}$') { Fail 'the release has no valid SHA-256 checksum.' }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('codenotch-install-' + [Guid]::NewGuid().ToString('N').Substring(0, 12))
    try {
        New-Item -ItemType Directory -Path $tmp -Force | Out-Null
        $zip = Join-Path $tmp 'CodeNotch.zip'
        Say "Downloading CodeNotch $ver..."
        Save-File $url $zip "Downloading $url"

        $actualSize = (Get-Item -LiteralPath $zip).Length
        if ($size -and $actualSize -ne $size) {
            Fail "the download has the wrong size ($actualSize bytes, expected $size). Nothing was installed; try again."
        }
        $actualSha = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualSha -ne $sha) {
            Fail "SHA-256 mismatch: the download is $actualSha, the release says $sha. Nothing was installed."
        }
        Say 'Checksum verified.' 'Green'

        $x = Join-Path $tmp 'x'
        Expand-Archive -LiteralPath $zip -DestinationPath $x -Force
        $mainExe = Get-ChildItem -LiteralPath $x -Recurse -File -Filter 'codenotch.exe' | Select-Object -First 1
        if (-not $mainExe) { Fail 'codenotch.exe was not found in the archive.' }
        $src = $mainExe.DirectoryName
        if (-not (Test-Path -LiteralPath (Join-Path $src 'codenotch-hook.exe'))) { Fail 'codenotch-hook.exe was not found in the archive.' }

        $why = if ($NoLaunch) { 'to update it' } else { 'to update it; it will be started again' }
        $wasRunning = Stop-Installed $why
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        foreach ($n in ($exeNames + $extraNames)) {
            $f = Join-Path $src $n
            if (Test-Path -LiteralPath $f -PathType Leaf) {
                Copy-Item -LiteralPath $f -Destination $InstallDir -Force
                # Drop the "downloaded from the internet" mark so SmartScreen asks less.
                Unblock-File -LiteralPath (Join-Path $InstallDir $n) -ErrorAction SilentlyContinue
            }
        }
        foreach ($n in $exeNames) {
            $a = (Get-FileHash -LiteralPath (Join-Path $src $n) -Algorithm SHA256).Hash
            $b = (Get-FileHash -LiteralPath (Join-Path $InstallDir $n) -Algorithm SHA256).Hash
            if ($a -ne $b) { Fail "$n was not copied correctly to $InstallDir." }
        }
    } finally {
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
    }

    $exe = Join-Path $InstallDir 'codenotch.exe'
    if (-not $NoShortcut) {
        try {
            New-Item -ItemType Directory -Path $ShortcutDir -Force | Out-Null
            $shell = New-Object -ComObject WScript.Shell
            $lnk = $shell.CreateShortcut($shortcut)
            $lnk.TargetPath = $exe
            $lnk.WorkingDirectory = $InstallDir
            $lnk.IconLocation = "$exe,0"
            $lnk.Description = 'CodeNotch - AI quota rings'
            $lnk.Save()
            [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
        } catch {
            Say "Could not create the Start Menu shortcut: $($_.Exception.Message)" 'Yellow'
        }
    }

    $launched = $false
    if (-not $NoLaunch) {
        Start-Process -FilePath $exe -WorkingDirectory $InstallDir
        $launched = $true
    }

    Write-Host ''
    Say "CodeNotch $ver is installed." 'Green'
    Say "  Folder:    $InstallDir"
    if (-not $NoShortcut) { Say "  Shortcut:  $shortcut" }
    if ($launched) {
        if ($wasRunning) { Say '  Restarted: the app is running again (tray icon).' }
        else { Say '  Started:   look for the tray icon; the onboarding opens on first start.' }
    }
    Say '  Updates:   the app updates itself; or run the install command again.'
    Say '  Uninstall:'
    $dirVar = ''
    if ($InstallDir -ine [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs\CodeNotch')).TrimEnd('\')) { $dirVar = "`$env:CODENOTCH_INSTALL_DIR='$InstallDir'; " }
    Say "    $dirVar`$env:CODENOTCH_UNINSTALL=1; $oneLiner" 'White'
    Say 'The executables are not code-signed: SmartScreen may ask for confirmation on first start.' 'DarkGray'
}

function Test-CodeNotchFlag([string]$name) {
    $v = [Environment]::GetEnvironmentVariable($name)
    return [bool]($v -and $v -notin @('0', 'false', 'no', 'off'))
}

# Parameters when run as a file; environment variables when piped into iex.
$codeNotchArgs = @{
    InstallDir  = $env:CODENOTCH_INSTALL_DIR
    Version     = $env:CODENOTCH_VERSION
    ShortcutDir = $env:CODENOTCH_SHORTCUT_DIR
    ManifestUrl = $env:CODENOTCH_MANIFEST_URL
    NoLaunch    = (Test-CodeNotchFlag 'CODENOTCH_NO_LAUNCH')
    NoShortcut  = (Test-CodeNotchFlag 'CODENOTCH_NO_SHORTCUT')
    Uninstall   = (Test-CodeNotchFlag 'CODENOTCH_UNINSTALL')
}
# One-shot: so that running the install command again in the same window installs, not uninstalls.
Remove-Item -Path Env:CODENOTCH_UNINSTALL -ErrorAction SilentlyContinue
if ($MyInvocation.InvocationName -ne '' -and $MyInvocation.MyCommand.Path -and $args.Count -gt 0) {
    # Run as a file with arguments: they override the environment.
    for ($cnI = 0; $cnI -lt $args.Count; $cnI++) {
        $cnA = [string]$args[$cnI]
        switch -Regex ($cnA) {
            '^-(InstallDir|Version|ShortcutDir|ManifestUrl)$' { $codeNotchArgs[$Matches[1]] = [string]$args[++$cnI]; continue }
            '^-(NoLaunch|NoShortcut|Uninstall)$' { $codeNotchArgs[$Matches[1]] = $true; continue }
            '^-(NoLaunch|NoShortcut|Uninstall):\$?(true|false)$' { $codeNotchArgs[$Matches[1]] = ($Matches[2] -eq 'true'); continue }
            default { Write-Host "error: unknown argument '$cnA'. Options: -InstallDir, -Version, -ShortcutDir, -ManifestUrl, -NoLaunch, -NoShortcut, -Uninstall" -ForegroundColor Red; exit 2 }
        }
    }
}
foreach ($cnK in @($codeNotchArgs.Keys)) { if ($codeNotchArgs[$cnK] -is [string] -and -not $codeNotchArgs[$cnK]) { $codeNotchArgs.Remove($cnK) } }
foreach ($cnK in @('NoLaunch', 'NoShortcut', 'Uninstall')) { if ($codeNotchArgs.ContainsKey($cnK) -and -not $codeNotchArgs[$cnK]) { $codeNotchArgs.Remove($cnK) } }

$codeNotchFailed = $false
try {
    Install-CodeNotch @codeNotchArgs
} catch {
    if ($_.Exception.Message -notlike 'CodeNotch install failed:*') {
        Write-Host "error: $($_.Exception.Message)" -ForegroundColor Red
    }
    $codeNotchFailed = $true
} finally {
    Remove-Variable -Name codeNotchArgs, cnI, cnA, cnK -ErrorAction SilentlyContinue
}
# A failed run as a file exits with 1; under iex, never close the user's shell.
if ($codeNotchFailed -and $MyInvocation.MyCommand.Path) { exit 1 }
Remove-Variable -Name codeNotchFailed -ErrorAction SilentlyContinue
