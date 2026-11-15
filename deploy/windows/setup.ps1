# EbbTop Windows wizard: install, uninstall and inspect the client.
#
#   powershell -ExecutionPolicy Bypass -File setup.ps1            menu
#   powershell -ExecutionPolicy Bypass -File setup.ps1 -Action install
#   powershell -ExecutionPolicy Bypass -File setup.ps1 -Action install -Exe .\ebbtop.exe -Autostart
#   powershell -ExecutionPolicy Bypass -File setup.ps1 -Action uninstall [-Purge]
#   powershell -ExecutionPolicy Bypass -File setup.ps1 -Action status
#
# Install verifies ebbtop.exe against SHA256SUMS when that file sits next to it,
# then hands off to install.ps1 (the same script the in-app updater runs), so
# the backup, shortcut and startup behaviour stay in one place.
#
# Flags:
#   -Exe <path>    ebbtop.exe to install (default: next to setup.ps1, then cwd)
#   -Autostart     also create the Startup shortcut (start with Windows)
#   -NoLaunch      do not start EbbTop when the install finishes
#   -Purge         uninstall also deletes config, logs and stored credentials
#   -Yes           take every default, no prompts
#   -WhatIf        print the plan, change nothing
#
# No admin needed. Reads no secret: the per-host tokens live in Windows
# Credential Manager and are only listed by name, never read out.
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [ValidateSet('auto', 'install', 'uninstall', 'status', 'menu')][string]$Action = 'auto',
    [string]$Exe = '',
    [switch]$Autostart,
    [switch]$NoLaunch,
    [switch]$Purge,
    [switch]$Yes,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
$script:Dry = [bool]$WhatIfPreference
$ScriptDir = $PSScriptRoot
if (-not $ScriptDir) { $ScriptDir = (Get-Location).Path }

$UserBase = $env:USERPROFILE
if (-not $UserBase) { $UserBase = $HOME }
$LocalBase = $env:LOCALAPPDATA
if (-not $LocalBase) { $LocalBase = Join-Path $UserBase 'AppData\Local' }
$RoamBase = $env:APPDATA
if (-not $RoamBase) { $RoamBase = Join-Path $UserBase 'AppData\Roaming' }

$Root = Join-Path $LocalBase 'ebbtop'
$Dst = Join-Path $Root 'ebbtop.exe'
$Bak = "$Dst.bak"
$UpdateLog = Join-Path $Root 'update.log'
$AppLog = Join-Path $Root 'ebbtop.log'
$CfgDir = Join-Path $RoamBase 'ebbtop'
$Cfg = Join-Path $CfgDir 'config.json'
$Installer = Join-Path $ScriptDir 'install.ps1'
$DesktopLnk = ''
$StartupLnk = ''
$deskDir = [Environment]::GetFolderPath('Desktop')
$startDir = [Environment]::GetFolderPath('Startup')
if ($deskDir) { $DesktopLnk = Join-Path $deskDir 'EbbTop.lnk' }
if ($startDir) { $StartupLnk = Join-Path $startDir 'EbbTop.lnk' }

function Write-Step([string]$Text) {
    Write-Host ''
    Write-Host ('== ' + $Text) -ForegroundColor White
}
function Write-Info([string]$Text) { Write-Host ('   ' + $Text) }
function Write-Ok([string]$Text) { Write-Host ('   ok   ' + $Text) -ForegroundColor Green }
function Write-Warn2([string]$Text) { Write-Host ('   warn ' + $Text) -ForegroundColor Yellow }
function Write-Fail([string]$Text) { Write-Host ('   fail ' + $Text) -ForegroundColor Red }
function Write-Note([string]$Text) { Write-Host ('   ' + $Text) -ForegroundColor DarkGray }
function Write-Kv([string]$Key, [string]$Value) {
    Write-Host ('   {0,-10} {1}' -f $Key, $Value)
}

function Ask([string]$Prompt, [string]$Default = '') {
    if ($Yes -or $script:Dry) {
        Write-Host ('   {0}: {1}' -f $Prompt, $Default)
        return $Default
    }
    $suffix = ''
    if ($Default) { $suffix = " [$Default]" }
    Write-Host -NoNewline ('   ' + $Prompt + $suffix + ': ')
    $ans = Read-Host
    if ([string]::IsNullOrWhiteSpace($ans)) { return $Default }
    return $ans
}

function AskYes([string]$Prompt, [string]$Default = 'n') {
    if ($Yes) {
        Write-Host ('   {0} [yes]' -f $Prompt)
        return $true
    }
    if ($script:Dry) {
        Write-Host ('   {0} [{1}] (dry)' -f $Prompt, $Default)
        return ($Default -eq 'y')
    }
    $hint = 'y/N'
    if ($Default -eq 'y') { $hint = 'Y/n' }
    Write-Host -NoNewline ('   ' + $Prompt + ' [' + $hint + ']: ')
    $ans = Read-Host
    if ([string]::IsNullOrWhiteSpace($ans)) { $ans = $Default }
    return ($ans -match '^[Yy]')
}

function Invoke-Planned([string]$Description, [scriptblock]$Block) {
    if ($script:Dry) {
        Write-Note ('would: ' + $Description)
        return
    }
    & $Block
}

function Test-Installed { return (Test-Path $Dst) }

function Get-ExeHash([string]$Path) {
    if (-not (Test-Path $Path)) { return '' }
    return (Get-FileHash $Path -Algorithm SHA256).Hash.Substring(0, 16)
}

function Get-ExeVersion([string]$Path) {
    if (-not (Test-Path $Path)) { return '' }
    $vi = (Get-Item $Path).VersionInfo
    if ($vi.ProductVersion) { return $vi.ProductVersion }
    return ''
}

function Get-Running {
    return @(Get-Process -Name ebbtop -ErrorAction SilentlyContinue)
}

function New-Shortcut([string]$Path) {
    if (-not $Path) { return }
    Invoke-Planned ('create shortcut ' + $Path) {
        $sh = New-Object -ComObject WScript.Shell
        $lnk = $sh.CreateShortcut($Path)
        $lnk.TargetPath = $Dst
        $lnk.WorkingDirectory = $Root
        $lnk.Description = 'EbbTop: live server monitor'
        $lnk.Save()
    }
}

function Get-CredentialTargets {
    $targets = @()
    if (-not (Get-Command cmdkey -ErrorAction SilentlyContinue)) { return $targets }
    $out = & cmdkey /list 2>$null
    foreach ($line in $out) {
        if ($line -match 'Target:\s*(.+)$') {
            $t = $Matches[1].Trim()
            if ($t -match 'ebbtop') { $targets += $t }
        }
    }
    return $targets
}

function Find-SourceExe {
    if ($Exe) {
        if (-not (Test-Path $Exe)) { throw "no such file: $Exe" }
        return (Resolve-Path $Exe).Path
    }
    $cands = @((Join-Path $ScriptDir 'ebbtop.exe'), (Join-Path (Get-Location).Path 'ebbtop.exe'))
    foreach ($c in $cands) {
        if (Test-Path $c) { return (Resolve-Path $c).Path }
    }
    return ''
}

function Test-ExeBytes([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 4096) { return $false }
    if ($bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { return $false }
    return $true
}

function Assert-Checksum([string]$Path) {
    $sums = Join-Path (Split-Path $Path -Parent) 'SHA256SUMS'
    if (-not (Test-Path $sums)) {
        Write-Warn2 'no SHA256SUMS next to the exe: hash not verified'
        Write-Note ('expected sha256: ' + (Get-FileHash $Path -Algorithm SHA256).Hash)
        return
    }
    $leaf = Split-Path $Path -Leaf
    $expected = $null
    foreach ($line in Get-Content $sums) {
        if ($line -match '^\s*([0-9a-fA-F]{64})\s+\*?(.+?)\s*$') {
            if ($Matches[2] -eq $leaf) { $expected = $Matches[1].ToLower() }
        }
    }
    if (-not $expected) {
        Write-Warn2 ("SHA256SUMS has no entry for " + $leaf + ": hash not verified")
        return
    }
    $actual = (Get-FileHash $Path -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) {
        throw ("checksum mismatch for {0}: expected {1}, got {2}" -f $leaf, $expected.Substring(0, 16), $actual.Substring(0, 16))
    }
    Write-Ok ('checksum matches SHA256SUMS (sha256:' + $expected.Substring(0, 16) + ')')
}

function Invoke-Install {
    Write-Step 'preflight'
    if (-not (Test-Path $Installer)) { throw "install.ps1 not found next to setup.ps1: $Installer" }
    Write-Ok ('installer: ' + $Installer)

    $src = Find-SourceExe
    if (-not $src) { throw 'no ebbtop.exe found: pass -Exe <path> or put it next to setup.ps1' }
    Write-Ok ('source: ' + $src + ' (' + [math]::Round((Get-Item $src).Length / 1MB, 2) + ' MB)')
    if (-not (Test-ExeBytes $src)) { throw "$src does not look like a Windows executable (no MZ header)" }
    Write-Ok 'PE header present'
    Assert-Checksum $src

    Write-Step 'current state'
    if (Test-Installed) {
        Write-Kv 'installed' ($Dst + '  sha256:' + (Get-ExeHash $Dst) + ' ' + (Get-ExeVersion $Dst))
        Write-Info 'this install replaces it and keeps the previous exe as ebbtop.exe.bak'
    }
    else {
        Write-Info 'not installed yet'
    }
    Write-Kv 'config' ($(if (Test-Path $Cfg) { "$Cfg (kept)" } else { 'none yet, onboarding creates it' }))

    Write-Step 'plan'
    Write-Info ('1. stop a running EbbTop, then copy the new exe to ' + $Dst)
    Write-Info '2. keep the previous exe as ebbtop.exe.bak and log to update.log'
    Write-Info '3. create Desktop\EbbTop.lnk'
    if ($Autostart) { Write-Info '4. create the Startup shortcut (starts with Windows)' }
    else { Write-Info '4. no Startup shortcut (add -Autostart if you want it to start with Windows)' }
    if ($NoLaunch) { Write-Info '5. do not launch it now' }
    else { Write-Info '5. launch the installed EbbTop' }
    if (-not (AskYes 'install EbbTop now?' 'y')) {
        Write-Info 'cancelled, nothing changed'
        return
    }

    Write-Step 'install'
    $splat = @{ Exe = $src; NoRelaunch = [bool]$NoLaunch }
    if ($Autostart) { $splat.Autostart = $true }
    Invoke-Planned ('run install.ps1 -Exe ' + $src + $(if ($Autostart) { ' -Autostart' } else { '' })) {
        & $Installer @splat
    }
    if ($script:Dry) { return }

    Write-Step 'verify'
    if (Test-Installed) { Write-Ok ('exe in place: ' + $Dst) } else { Write-Fail 'exe missing after install'; return }
    $a = (Get-FileHash $Dst -Algorithm SHA256).Hash
    $b = (Get-FileHash $src -Algorithm SHA256).Hash
    if ($a -eq $b) { Write-Ok ('installed bytes match the source (sha256:' + $a.Substring(0, 16) + ')') }
    else { Write-Fail ('installed bytes differ: ' + $a.Substring(0, 16) + ' vs ' + $b.Substring(0, 16)) }
    if ($DesktopLnk -and (Test-Path $DesktopLnk)) { Write-Ok ('shortcut: ' + $DesktopLnk) } else { Write-Fail 'Desktop shortcut missing' }
    if ($Autostart) {
        if ($StartupLnk -and (Test-Path $StartupLnk)) { Write-Ok ('startup shortcut: ' + $StartupLnk) }
        else { Write-Fail 'Startup shortcut missing' }
    }
    $running = Get-Running
    if ($running.Count -gt 0) { Write-Ok ('EbbTop is running (pid ' + $running[0].Id + ')') }
    elseif ($NoLaunch) { Write-Info 'not started (-NoLaunch)' }
    else { Write-Warn2 'EbbTop is not running: start it from the Desktop shortcut' }

    Write-Step 'next'
    Write-Info 'first launch: the onboarding wizard asks for the host URL (ws://<host>:45976/ws) and that host''s token'
    Write-Info ('config: ' + $Cfg)
    Write-Info ('log:    ' + $AppLog)
}

function Invoke-Uninstall {
    Write-Step 'what is installed'
    $running = Get-Running
    $found = $false
    if (Test-Installed) { $found = $true; Write-Kv 'exe' ($Dst + '  sha256:' + (Get-ExeHash $Dst) + ' ' + (Get-ExeVersion $Dst)) }
    if (Test-Path $Bak) { $found = $true; Write-Kv 'backup' $Bak }
    if ($DesktopLnk -and (Test-Path $DesktopLnk)) { $found = $true; Write-Kv 'shortcut' $DesktopLnk }
    if ($StartupLnk -and (Test-Path $StartupLnk)) { $found = $true; Write-Kv 'startup' $StartupLnk }
    if (Test-Path $CfgDir) { $found = $true; Write-Kv 'state' ($CfgDir + ' (config, log)') }
    if ($running.Count -gt 0) { $found = $true; Write-Kv 'running' ('yes (pid ' + $running[0].Id + ')') }
    if (-not $found) {
        Write-Ok 'nothing to remove: no ebbtop.exe, no shortcuts, no config'
        return
    }

    Write-Step 'plan'
    Write-Info '1. stop ebbtop.exe if it is running'
    Write-Info '2. remove the Desktop and Startup shortcuts'
    Write-Info '3. remove ebbtop.exe and ebbtop.exe.bak (never any other file)'
    if ($Purge) {
        Write-Warn2 ('4. remove ' + $CfgDir + ': config and log')
        Write-Warn2 '5. delete every stored ebbtop credential from Windows Credential Manager'
    }
    else {
        Write-Info ('4. keep ' + $CfgDir + ' (config, log and stored tokens are reused by the next install)')
    }
    if (-not (AskYes 'uninstall EbbTop from this user?' 'n')) {
        Write-Info 'cancelled, nothing changed'
        return
    }

    Write-Step 'remove'
    if (-not $script:Dry) {
        $running = Get-Running
        if ($running.Count -gt 0) {
            $running | Stop-Process -Force
            for ($i = 0; $i -lt 50; $i++) {
                if ((Get-Running).Count -eq 0) { break }
                Start-Sleep -Milliseconds 200
            }
            Write-Ok 'stopped the running EbbTop'
        }
    }
    if ($DesktopLnk) {
        Invoke-Planned ('remove ' + $DesktopLnk) { Remove-Item -Force $DesktopLnk -ErrorAction SilentlyContinue }
    }
    if ($StartupLnk) {
        Invoke-Planned ('remove ' + $StartupLnk) { Remove-Item -Force $StartupLnk -ErrorAction SilentlyContinue }
    }
    Invoke-Planned ('remove ' + $Dst + ' and ' + $Bak) {
        Remove-Item -Force $Dst -ErrorAction SilentlyContinue
        Remove-Item -Force $Bak -ErrorAction SilentlyContinue
    }
    if ($Purge) {
        Invoke-Planned ('remove ' + $CfgDir) { Remove-Item -Recurse -Force $CfgDir -ErrorAction SilentlyContinue }
        $creds = Get-CredentialTargets
        if ($creds.Count -eq 0) {
            Write-Info 'no ebbtop credential entries found in Credential Manager'
        }
        foreach ($t in $creds) {
            Invoke-Planned ('delete credential ' + $t) { & cmdkey ("/delete:" + $t) | Out-Null }
        }
    }
    if ($script:Dry) { return }

    Write-Step 'verify'
    if (Test-Installed) { Write-Fail ('exe still present: ' + $Dst) } else { Write-Ok 'exe removed' }
    if (Test-Path $Bak) { Write-Fail ('backup still present: ' + $Bak) } else { Write-Ok 'backup removed' }
    if ($DesktopLnk -and (Test-Path $DesktopLnk)) { Write-Fail 'Desktop shortcut still present' } else { Write-Ok 'Desktop shortcut removed' }
    if ($StartupLnk -and (Test-Path $StartupLnk)) { Write-Fail 'Startup shortcut still present' } else { Write-Ok 'Startup shortcut removed' }
    if ((Get-Running).Count -gt 0) { Write-Fail 'EbbTop is still running' } else { Write-Ok 'no EbbTop process' }
    if ($Purge) {
        if (Test-Path $CfgDir) { Write-Warn2 ($CfgDir + ' still present') } else { Write-Ok ($CfgDir + ' removed') }
        $left = Get-CredentialTargets
        if ($left.Count -eq 0) { Write-Ok 'no ebbtop credentials left' }
        else { Write-Warn2 ('credentials left: ' + ($left -join ', ') + ' (Credential Manager)') }
    }
    else {
        if (Test-Path $CfgDir) {
            Write-Note ('kept: ' + $CfgDir + ' (config, log, stored tokens). Remove it with: setup.ps1 -Action uninstall -Purge')
        }
    }
}

function Invoke-Status {
    Write-Step ('EbbTop status on ' + $env:COMPUTERNAME)
    if (Test-Installed) {
        Write-Kv 'exe' $Dst
        Write-Kv 'sha256' (Get-ExeHash $Dst)
        if (Get-ExeVersion $Dst) { Write-Kv 'version' (Get-ExeVersion $Dst) }
        Write-Kv 'size' ([math]::Round((Get-Item $Dst).Length / 1MB, 2).ToString() + ' MB')
        if (Test-Path $Bak) { Write-Kv 'backup' $Bak }
    }
    else {
        Write-Info ('not installed: no ' + $Dst)
    }
    $running = Get-Running
    if ($running.Count -gt 0) { Write-Kv 'running' ('yes (pid ' + $running[0].Id + ')') } else { Write-Kv 'running' 'no' }
    if ($DesktopLnk -and (Test-Path $DesktopLnk)) { Write-Kv 'shortcut' $DesktopLnk } else { Write-Kv 'shortcut' 'none' }
    if ($StartupLnk -and (Test-Path $StartupLnk)) { Write-Kv 'startup' 'enabled' } else { Write-Kv 'startup' 'off' }
    if (Test-Path $Cfg) {
        Write-Kv 'config' ($Cfg + ' (' + (Get-Item $Cfg).Length + ' bytes)')
    }
    else {
        Write-Kv 'config' 'none yet'
    }
    if (Test-Path $AppLog) {
        Write-Kv 'log' ($AppLog + ' (' + [math]::Round((Get-Item $AppLog).Length / 1KB, 1) + ' KB)')
    }
    $creds = Get-CredentialTargets
    if ($creds.Count -gt 0) { Write-Kv 'tokens' ($creds.Count.ToString() + ' credential(s) in Credential Manager (values not read)') }
    else { Write-Kv 'tokens' 'no credential entries found' }
}

function Show-Menu {
    while ($true) {
        Write-Host ''
        Write-Host 'EbbTop Windows wizard' -ForegroundColor White
        Write-Host ('   ' + $env:COMPUTERNAME + '   user ' + $env:USERNAME) -ForegroundColor DarkGray
        Write-Host '------------------------------------------------------------'
        if (Test-Installed) { Write-Info ('state: installed at ' + $Dst) } else { Write-Info 'state: not installed' }
        Write-Host ''
        Write-Host '   what do you want to do?'
        Write-Host '     1) install or update EbbTop'
        Write-Host '     2) uninstall EbbTop'
        Write-Host '     3) show status'
        Write-Host '     4) quit'
        $choice = Read-Host '   choice [1]'
        if ([string]::IsNullOrWhiteSpace($choice)) { $choice = '1' }
        switch ($choice) {
            '1' { Invoke-Install }
            '2' { Invoke-Uninstall }
            '3' { Invoke-Status }
            '4' { return }
            default { Write-Warn2 'enter a number from 1 to 4' }
        }
    }
}

try {
    if ($Help) {
        Get-Content -LiteralPath $PSCommandPath |
            Select-Object -First 40 |
            Where-Object { $_ -like '#*' } |
            ForEach-Object { $_ -replace '^# ?', '' }
        return
    }
    if ($Action -eq 'auto') {
        $Action = 'menu'
    }
    switch ($Action) {
        'install' { Invoke-Install }
        'uninstall' { Invoke-Uninstall }
        'status' { Invoke-Status }
        'menu' { Show-Menu }
    }
}
catch {
    Write-Fail $_.Exception.Message
    exit 1
}
