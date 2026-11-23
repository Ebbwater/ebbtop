<#
.SYNOPSIS
Resolve the x64 SignTool that ships with an installed Windows SDK.

.DESCRIPTION
The Windows SDK installs SignTool under a version-stamped bin directory,
`%ProgramFiles(x86)%\Windows Kits\10\bin\<sdk-version>\x64\signtool.exe`. The
unversioned `bin\x64` alias is not guaranteed: the windows-2025-vs2026 image
20261116.315 (image readme: installed Windows SDKs = 10.0.26100.0 only) carries
no `bin\x64\signtool.exe`, which is why joining that fixed path fails before the
WACK gate can install anything at all.

This script picks the executable deterministically:

  * every `bin` child whose name is exactly four dot-separated numeric parts is
    an SDK version directory;
  * candidates are ordered by that version, highest first, comparing each part
    numerically (never as a string, so 10.0.10.0 outranks 10.0.9.0);
  * inside a version directory only `x64\signtool.exe` is a candidate, and it
    must be a readable PE image whose COFF machine field is AMD64 (0x8664);
    anything else — a missing file, an unreadable image, an x86 or ARM64
    build — is rejected with the reason recorded;
  * the unversioned `bin\x64\signtool.exe` is the last rung, used only when no
    versioned candidate qualifies.

The success stream carries exactly one line: the absolute path of the chosen
executable. The audit trail of every candidate examined goes to the information
stream (`Write-Host`), so it shows in a workflow log but is not captured by
`$signtool = & resolve-signtool.ps1`.

Throws a terminating error when no supported executable exists, naming every
path examined and why each was rejected. An empty result would let a caller
carry on to `& $null`.

.PARAMETER KitsRoot
Windows Kits 10 root. Defaults to `%ProgramFiles(x86)%\Windows Kits\10`.

.EXAMPLE
# Workflow use: capture the path, then invoke it.
$signtool = & deploy/windows/msix/resolve-signtool.ps1
& $signtool sign /fd SHA256 /a /s My /n $env:MSIX_PUBLISHER wack-test.msix

.EXAMPLE
# Fixture use: point the resolver at a synthetic kits tree.
& ./resolve-signtool.ps1 -KitsRoot '/tmp/fixture/Windows Kits/10'
#>
[CmdletBinding()]
param(
    [string]$KitsRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# COFF machine values that are positively not x64. A file whose header says one
# of these is never a usable SignTool for an x64 package, wherever it sits.
$Amd64 = 0x8664
$NonX64Machines = @{
    0x014c = 'x86 (i386)'
    0x01c0 = 'ARM'
    0x01c4 = 'ARM (Thumb-2)'
    0x0200 = 'Itanium'
    0xaa64 = 'ARM64'
}

$Audit = New-Object 'System.Collections.Generic.List[string]'

function Get-PeMachine {
    # Returns the PE COFF machine field, or $null when the file is not a
    # readable PE image. $null is never accepted as a candidate: a path that
    # cannot be proved to be an x64 image is not a supported executable.
    param([Parameter(Mandatory = $true)][string]$Path)
    try {
        $fs = [System.IO.File]::Open(
            $Path,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite)
    } catch {
        return $null
    }
    try {
        if ($fs.Length -lt 0x40) { return $null }
        $br = New-Object System.IO.BinaryReader($fs)
        if ($br.ReadUInt16() -ne 0x5A4D) { return $null }        # 'MZ'
        [void]$fs.Seek(0x3C, [System.IO.SeekOrigin]::Begin)
        $lfanew = $br.ReadInt32()
        if ($lfanew -lt 0x40 -or ($lfanew + 6) -gt $fs.Length) { return $null }
        [void]$fs.Seek($lfanew, [System.IO.SeekOrigin]::Begin)
        if ($br.ReadUInt32() -ne 0x00004550) { return $null }    # 'PE\0\0'
        return [int]$br.ReadUInt16()
    } catch {
        return $null
    } finally {
        $fs.Dispose()
    }
}

# ---- the kits root --------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($KitsRoot)) {
    $pf86 = ${env:ProgramFiles(x86)}
    if ([string]::IsNullOrWhiteSpace($pf86)) {
        throw ('Cannot resolve SignTool: ProgramFiles(x86) is not set, so the Windows Kits root is unknown. ' +
            'Pass -KitsRoot with the SDK root (normally <ProgramFiles(x86)>\Windows Kits\10).')
    }
    $KitsRoot = Join-Path (Join-Path $pf86 'Windows Kits') '10'
}

if (-not (Test-Path -LiteralPath $KitsRoot -PathType Container)) {
    throw ("Cannot resolve SignTool: the Windows Kits root '$KitsRoot' does not exist or is not a directory.")
}

$BinRoot = Join-Path $KitsRoot 'bin'
if (-not (Test-Path -LiteralPath $BinRoot -PathType Container)) {
    throw ("Cannot resolve SignTool: no 'bin' directory under the Windows Kits root '$KitsRoot'.")
}

# ---- candidate list -------------------------------------------------------
# Deterministic order: versioned directories newest first (numeric per part),
# then the unversioned alias. Get-ChildItem order is filesystem order, so every
# ordering decision below is made on an explicit key, never on arrival order.
$VersionPattern = '^\d{1,10}(\.\d{1,10}){3}$'
$Versioned = New-Object 'System.Collections.Generic.List[object]'
foreach ($dir in @(Get-ChildItem -LiteralPath $BinRoot -Directory -Force)) {
    if ($dir.Name -notmatch $VersionPattern) { continue }
    $parts = $dir.Name.Split('.')
    # Fixed-width key: numeric order without relying on [version] comparison,
    # and stable for the same set of version directories.
    $sortKey = ($parts | ForEach-Object { $_.PadLeft(10, '0') }) -join '.'
    $Versioned.Add([pscustomobject]@{
            Name    = $dir.Name
            Path    = $dir.FullName
            SortKey = $sortKey
        })
}

$Ordered = New-Object 'System.Collections.Generic.List[object]'
$sorted = @($Versioned | Sort-Object -Property `
        @{ Expression = { $_.SortKey }; Descending = $true },
        @{ Expression = { $_.Name }; Descending = $false })
foreach ($v in $sorted) {
    $Ordered.Add([pscustomobject]@{
            Label = "SDK $($v.Name)"
            Exe   = Join-Path (Join-Path $v.Path 'x64') 'signtool.exe'
        })
}
$Ordered.Add([pscustomobject]@{
        Label = 'unversioned bin\x64'
        Exe   = Join-Path (Join-Path $BinRoot 'x64') 'signtool.exe'
    })

# ---- selection ------------------------------------------------------------
$Selected = $null
$SelectedLabel = $null

foreach ($candidate in $Ordered) {
    $path = $candidate.Exe
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        $line = "reject  {0,-22} {1}  (no such file)" -f $candidate.Label, $path
        $Audit.Add($line); Write-Host $line
        continue
    }

    $machine = Get-PeMachine -Path $path
    if ($null -eq $machine) {
        $line = "reject  {0,-22} {1}  (not a readable PE image)" -f $candidate.Label, $path
        $Audit.Add($line); Write-Host $line
        continue
    }

    if ($machine -ne $Amd64) {
        $what = if ($NonX64Machines.ContainsKey($machine)) {
            $NonX64Machines[$machine]
        } else {
            'unrecognised machine'
        }
        $line = "reject  {0,-22} {1}  (PE machine 0x{2:x4}: {3}, not x64)" -f $candidate.Label, $path, $machine, $what
        $Audit.Add($line); Write-Host $line
        continue
    }

    $line = "accept  {0,-22} {1}  (PE machine 0x{2:x4}: x64)" -f $candidate.Label, $path, $machine
    $Audit.Add($line); Write-Host $line
    $Selected = $path
    $SelectedLabel = $candidate.Label
    break
}

if ($null -eq $Selected) {
    $examined = ($Ordered | ForEach-Object { '  - ' + $_.Exe }) -join [Environment]::NewLine
    $rejected = ($Audit | ForEach-Object { '  ' + $_ }) -join [Environment]::NewLine
    throw ("Cannot resolve SignTool: no supported signtool.exe exists under '$KitsRoot'." + [Environment]::NewLine +
        'Examined, in order:' + [Environment]::NewLine + $examined + [Environment]::NewLine +
        'Rejections:' + [Environment]::NewLine + $rejected + [Environment]::NewLine +
        'Install the Windows SDK (its Windows App Certification Kit carries SignTool) or pass -KitsRoot with the SDK root.')
}

Write-Host ("signtool: {0}  [{1}]" -f $Selected, $SelectedLabel)
Write-Output $Selected
