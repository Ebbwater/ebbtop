# Install ebbtop.exe for the current user and create a one-click desktop shortcut.
# No admin needed. Re-run to update; the previous exe is kept as ebbtop.exe.bak.
# The in-app updater runs this detached: it stops the running app, WAITS for the
# exe lock to release, swaps the bytes, logs to update.log, and relaunches.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Exe .\ebbtop.exe [-Autostart]
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [switch]$Autostart,
    [switch]$Rollback
)
$ErrorActionPreference = 'Stop'
$dir = Join-Path $env:LOCALAPPDATA 'ebbtop'
$dst = Join-Path $dir 'ebbtop.exe'
$log = Join-Path $dir 'update.log'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

function Log([string]$msg) {
    Add-Content -Path $log -Value ('{0} {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $msg)
}

# Copy with retry: even after the app exits, Windows can hold the exe lock for a
# moment. A single immediate Copy-Item is what made updates silently fail.
function Copy-WithRetry([string]$src, [string]$dest) {
    $last = $null
    for ($i = 0; $i -lt 20; $i++) {
        try { Copy-Item -Force $src $dest -ErrorAction Stop; return }
        catch { $last = $_; Start-Sleep -Milliseconds 300 }
    }
    throw "copy $src -> $dest failed after retries: $last"
}

function New-Shortcut([string]$path) {
    $sh = New-Object -ComObject WScript.Shell
    $lnk = $sh.CreateShortcut($path)
    $lnk.TargetPath = $dst
    $lnk.WorkingDirectory = $dir
    $lnk.Description = 'EbbTop: live server monitor'
    $lnk.Save()
}

try {
    Log ('install start rollback={0} exe={1}' -f [bool]$Rollback, $Exe)

    # Stop the running app, then WAIT for it to exit. Stop-Process returns before
    # the process releases its lock on the running exe, so copying right away hits
    # a locked file. Poll until the process is gone (bounded), then copy with retry.
    $proc = Get-Process -Name ebbtop -ErrorAction SilentlyContinue
    if ($proc) {
        $proc | Stop-Process -Force
        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-Date) -lt $deadline) {
            if (-not (Get-Process -Name ebbtop -ErrorAction SilentlyContinue)) { break }
            Start-Sleep -Milliseconds 200
        }
        Log 'stopped running ebbtop'
    }

    if ($Rollback) {
        if (-not (Test-Path "$dst.bak")) { throw "no ebbtop.exe.bak to roll back to" }
        Copy-WithRetry "$dst.bak" $dst
        Log 'rolled back from .bak'
        Write-Host "rolled back"
    } else {
        if (Test-Path $dst) { Copy-WithRetry $dst "$dst.bak" }
        Copy-WithRetry $Exe $dst
        Log 'swapped exe'
    }

    New-Shortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) 'EbbTop.lnk')
    if ($Autostart) {
        New-Shortcut (Join-Path ([Environment]::GetFolderPath('Startup')) 'EbbTop.lnk')
    }

    # Relaunch the installed version so "Restart to update" actually restarts.
    Start-Process -FilePath $dst -WorkingDirectory $dir
    Log 'relaunched'

    $hash = (Get-FileHash $dst -Algorithm SHA256).Hash.Substring(0, 16)
    Write-Host "installed $dst sha256:$hash"
    Write-Host "shortcut: Desktop\EbbTop.lnk$(if ($Autostart) { ' + Startup' })"
    Write-Host "log: $dir\ebbtop.log (updater: $log)"
} catch {
    Log ('ERROR {0}' -f $_)
    Write-Host "install failed: $_ (see $log)"
    throw
}
