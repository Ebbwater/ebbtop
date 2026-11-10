# Install ebbtop.exe for the current user and create a one-click desktop shortcut.
# No admin needed. Re-run to update; the previous exe is kept as ebbtop.exe.bak.
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
New-Item -ItemType Directory -Force -Path $dir | Out-Null

Get-Process ebbtop -ErrorAction SilentlyContinue | Stop-Process -Force

if ($Rollback) {
    if (-not (Test-Path "$dst.bak")) { throw "no ebbtop.exe.bak to roll back to" }
    Move-Item -Force "$dst.bak" $dst
    Write-Host "rolled back"
} else {
    if (Test-Path $dst) { Copy-Item -Force $dst "$dst.bak" }
    Copy-Item -Force $Exe $dst
}

function New-Shortcut([string]$path) {
    $sh = New-Object -ComObject WScript.Shell
    $lnk = $sh.CreateShortcut($path)
    $lnk.TargetPath = $dst
    $lnk.WorkingDirectory = $dir
    $lnk.Description = 'EbbTop: live server monitor'
    $lnk.Save()
}

New-Shortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) 'EbbTop.lnk')
if ($Autostart) {
    New-Shortcut (Join-Path ([Environment]::GetFolderPath('Startup')) 'EbbTop.lnk')
}

$hash = (Get-FileHash $dst -Algorithm SHA256).Hash.Substring(0, 16)
Write-Host "installed $dst sha256:$hash"
Write-Host "shortcut: Desktop\EbbTop.lnk$(if ($Autostart) { ' + Startup' })"
Write-Host "log: $dir\ebbtop.log"
