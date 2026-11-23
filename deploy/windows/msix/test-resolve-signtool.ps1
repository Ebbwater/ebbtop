<#
.SYNOPSIS
Fixture proof for resolve-signtool.ps1.

.DESCRIPTION
Builds synthetic Windows Kits trees under a temporary directory and proves, from
the resolver's own output, the properties the WACK gate depends on:

  R1 version ordering is numeric per part, not lexicographic, so 10.0.10.0
     outranks 10.0.9.0;
  R2 selection does not depend on directory creation order (two trees built in
     opposite orders resolve to the same relative path);
  R2c a tree with the exact runner-image shape (one installed SDK, 10.0.26100.0,
     no unversioned bin\x64 alias) resolves, which is the case the hardcoded
     bin\x64 path failed on;
  R3 a non-x64 PE image in a version directory is rejected on its COFF machine
     field (x86 0x014c and ARM64 0xaa64 each proved) and the next-newest x64
     candidate is selected;
  R4 a newer directory with no signtool.exe is skipped for an older one that
     has it;
  R5 the unversioned bin\x64 alias is the last rung: used when no versioned
     directory qualifies, ignored when one does;
  R6 non-version directories (a 3-part name, a non-numeric name) and a file
     that is not signtool.exe are never candidates;
  R7 an empty bin directory throws, and a version directory holding a
     non-PE file is rejected rather than accepted;
  R8 no supported executable throws a terminating error that names every path
     examined and states that no supported signtool.exe exists, and produces no
     path on the success stream;
  R9 the success stream carries exactly one object that is an existing file, so
     `$signtool = & resolve-signtool.ps1` cannot silently yield an array or
     $null;
  R10 the audit trail of rejected candidates is on the information stream, so
     it does not contaminate the captured path;
  R11 a missing kits root and a kits root with no bin directory each throw;
  R12 the fixture PE images are structurally valid at the offsets the resolver
     reads, checked independently of the resolver;
  W1 the workflow's sign step captures the resolver's path before invoking it
     and no longer joins the hardcoded Windows Kits\10\bin\x64 path;
  W2 the workflow still signs only a throwaway copy, with a one-day self-signed
     certificate whose subject is the manifest Publisher, trusted in both
     TrustedPeople stores, and still checks signtool's exit code and reads the
     Authenticode status back;
  W3 the preserved gates are present: the unsigned store-package artifact, the
     install step, the appcert run, the report parse, and the always-uploaded
     report with 30-day retention;
  W4 every pwsh run block in the workflow parses under the PowerShell parser
     (a syntax error there is fatal on the runner and invisible in YAML).

How the streams are observed: the resolver is invoked in-process exactly as the
workflow invokes it (`$path = & $Resolver -KitsRoot ...`), with the information
stream redirected to a buffer. A separate child-process run per case confirms
the exit code a workflow step would see.

What this cannot prove, and reports as GAP rather than PASS: that the real
runner image's SDK resolves, sign/install/appcert actually run, and the WACK
report is produced. The runner-image layout is taken from the image's own
published readme, recorded with -ImageEvidence and printed, not tested here.

.PARAMETER ImageEvidence
Path to the runner-image readme kept as evidence, printed for the record.

.EXAMPLE
pwsh -NoProfile -File deploy/windows/msix/test-resolve-signtool.ps1

.EXAMPLE
pwsh -NoProfile -File deploy/windows/msix/test-resolve-signtool.ps1 `
  -ImageEvidence /tmp/w2025-vs2026-315.md
#>
[CmdletBinding()]
param(
    [string]$ImageEvidence
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Here = $PSCmdlet.MyInvocation.MyCommand.Path
if (-not $Here) { $Here = $MyInvocation.MyCommand.Path }
$Here = Split-Path -Parent $Here
$Resolver = Join-Path $Here 'resolve-signtool.ps1'
if (-not (Test-Path -LiteralPath $Resolver -PathType Leaf)) {
    Write-Host "error: resolver not found at $Resolver"
    exit 2
}

$script:Passed = 0
$script:Failed = 0
$script:Gaps = 0

function Pass {
    param([string]$Name)
    $script:Passed++
    Write-Host "PASS  $Name"
}

function Fail {
    param([string]$Name, [string]$Detail)
    $script:Failed++
    Write-Host "FAIL  $Name"
    if ($Detail) { Write-Host "        $Detail" }
}

function Gap {
    param([string]$Name)
    $script:Gaps++
    Write-Host "GAP   $Name"
}

# ---- fixture helpers ------------------------------------------------------

function New-PeImage {
    # Minimal but structurally valid PE image carrying a chosen COFF machine
    # value: 'MZ', e_lfanew at 0x3C, 'PE\0\0', then the machine field.
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][int]$Machine
    )
    $bytes = New-Object byte[] 0x200
    $bytes[0] = 0x4D; $bytes[1] = 0x5A                     # 'MZ'
    $lfanew = 0x80
    [System.BitConverter]::GetBytes([int]$lfanew).CopyTo($bytes, 0x3C)
    $bytes[$lfanew + 0] = 0x50; $bytes[$lfanew + 1] = 0x45  # 'PE'
    $bytes[$lfanew + 2] = 0x00; $bytes[$lfanew + 3] = 0x00
    [System.BitConverter]::GetBytes([int]$Machine).CopyTo($bytes, $lfanew + 4)
    $dir = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    [System.IO.File]::WriteAllBytes($Path, $bytes)
}

function New-SdkVersion {
    param(
        [Parameter(Mandatory = $true)][string]$KitsRoot,
        [Parameter(Mandatory = $true)][string]$Version,
        [int]$Machine = 0x8664,
        [switch]$NoExe,
        [switch]$GarbageExe
    )
    $dir = Join-Path (Join-Path $KitsRoot 'bin') (Join-Path $Version 'x64')
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $exe = Join-Path $dir 'signtool.exe'
    if ($GarbageExe) {
        [System.IO.File]::WriteAllBytes($exe, [byte[]](1..64))
    } elseif (-not $NoExe) {
        New-PeImage -Path $exe -Machine $Machine
    }
    return $dir
}

function New-Tree {
    param([string]$Path)
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
    return $Path
}

function Invoke-Resolver {
    # In-process, the way the workflow calls it: capture the success stream
    # separately from the information stream, and the terminating error.
    param(
        [Parameter(Mandatory = $true)][string]$KitsRoot
    )
    $info = New-Object 'System.Collections.Generic.List[string]'
    $result = $null
    $errorText = $null
    try {
        $result = & $Resolver -KitsRoot $KitsRoot -InformationVariable infoVar 6>&1 |
            ForEach-Object {
                if ($_ -is [System.Management.Automation.InformationRecord]) {
                    $info.Add($_.MessageData.ToString())
                } else {
                    $_
                }
            }
    } catch {
        $errorText = $_.Exception.Message
    }
    # Items collected above that are not information records are the success
    # stream's output; split them out explicitly.
    $success = @()
    if ($null -ne $result) {
        foreach ($item in @($result)) {
            if ($item -is [System.Management.Automation.InformationRecord]) {
                $info.Add($item.MessageData.ToString())
            } else {
                $success += $item
            }
        }
    }
    return [pscustomobject]@{
        Success   = $success
        Info      = ($info -join "`n")
        Error     = $errorText
    }
}

function Get-ExitCode {
    # Child-process run: the exit code a workflow step observes.
    param([Parameter(Mandatory = $true)][string]$KitsRoot)
    $exe = (Get-Process -Id $PID).Path
    $args = @('-NoProfile', '-NonInteractive', '-File', $Resolver, '-KitsRoot', $KitsRoot)
    $p = Start-Process -FilePath $exe -ArgumentList $args -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput ([System.IO.Path]::GetTempFileName()) `
        -RedirectStandardError ([System.IO.Path]::GetTempFileName())
    return $p.ExitCode
}

$Root = Join-Path ([System.IO.Path]::GetTempPath()) ("ebbtop-signtool-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Root -Force | Out-Null
Write-Host "fixture root: $Root"
Write-Host "resolver:     $Resolver"
Write-Host "shell:        PowerShell $($PSVersionTable.PSVersion) ($($PSVersionTable.PSEdition))"
if ($ImageEvidence) {
    if (Test-Path -LiteralPath $ImageEvidence) {
        Write-Host "image evidence: $ImageEvidence"
        Select-String -LiteralPath $ImageEvidence -Pattern 'Image Version|Installed Windows SDKs|^\- 10\.' |
            ForEach-Object { Write-Host ("  | " + $_.Line.Trim()) }
    } else {
        Write-Host "image evidence: $ImageEvidence (NOT FOUND)"
    }
}
Write-Host ''

try {
    # ---- R1: numeric version ordering, not lexicographic -------------------
    $t1 = New-Tree (Join-Path $Root 'r1')
    New-SdkVersion -KitsRoot $t1 -Version '10.0.9.0' | Out-Null
    $want1Dir = New-SdkVersion -KitsRoot $t1 -Version '10.0.10.0'
    $r = Invoke-Resolver -KitsRoot $t1
    $expect1 = Join-Path $want1Dir 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect1) {
        Pass "R1 numeric ordering: 10.0.10.0 beats 10.0.9.0"
    } else {
        Fail 'R1 numeric ordering' "success=[$($r.Success -join ' | ')] want=$expect1 err=$($r.Error)"
    }

    # ---- R2: creation order does not matter -------------------------------
    $t2a = New-Tree (Join-Path $Root 'r2a')
    foreach ($v in '10.0.26100.0', '10.0.22621.0', '10.0.19041.0') {
        New-SdkVersion -KitsRoot $t2a -Version $v | Out-Null
    }
    $t2b = New-Tree (Join-Path $Root 'r2b')
    foreach ($v in '10.0.19041.0', '10.0.22621.0', '10.0.26100.0') {
        New-SdkVersion -KitsRoot $t2b -Version $v | Out-Null
    }
    $ra = Invoke-Resolver -KitsRoot $t2a
    $rb = Invoke-Resolver -KitsRoot $t2b
    $relA = if ($ra.Success.Count -eq 1) { $ra.Success[0].Substring($t2a.Length) } else { "<none>" }
    $relB = if ($rb.Success.Count -eq 1) { $rb.Success[0].Substring($t2b.Length) } else { "<none>" }
    if ($relA -eq $relB -and $relA -like '*10.0.26100.0*') {
        Pass "R2 creation order irrelevant: both trees select $relA"
    } else {
        Fail 'R2 creation order irrelevant' "a=$relA b=$relB"
    }

    $r2again = Invoke-Resolver -KitsRoot $t2a
    if ($r2again.Success.Count -eq 1 -and $r2again.Success[0] -eq $ra.Success[0]) {
        Pass 'R2b repeat runs are identical (deterministic selection)'
    } else {
        Fail 'R2b repeat runs identical' "first=[$($ra.Success -join '|')] second=[$($r2again.Success -join '|')]"
    }

    # ---- R2c: the exact runner-image shape -------------------------------
    # windows-2025-vs2026 20261116.315 publishes "Installed Windows SDKs: 10.0.26100.0"
    # (one version, no others). The pre-fix workflow joined the unversioned
    # bin\x64 alias, which that image does not carry, so the step failed before
    # installing anything. Model exactly that tree: one versioned directory and
    # no bin\x64.
    $t2c = New-Tree (Join-Path $Root 'r2c')
    $only = New-SdkVersion -KitsRoot $t2c -Version '10.0.26100.0' -Machine 0x8664
    $aliasExists = Test-Path -LiteralPath (Join-Path (Join-Path $t2c 'bin') 'x64')
    $r = Invoke-Resolver -KitsRoot $t2c
    $expect2c = Join-Path $only 'signtool.exe'
    if (-not $aliasExists -and $r.Success.Count -eq 1 -and $r.Success[0] -eq $expect2c) {
        Pass 'R2c runner-image shape (only 10.0.26100.0, no bin\x64 alias) resolves'
    } else {
        Fail 'R2c runner-image shape resolves' "aliasExists=$aliasExists success=[$($r.Success -join ' | ')] want=$expect2c"
    }
    # The negative half: the same tree with the old hardcoded join must not exist.
    $oldPath = Join-Path (Join-Path $t2c 'bin') 'x64\signtool.exe'
    if (-not (Test-Path -LiteralPath $oldPath)) {
        Pass 'R2d on that tree the old hardcoded bin\x64\signtool.exe path does not exist'
    } else {
        Fail 'R2d old hardcoded path absent on the runner-image shape' "it exists: $oldPath"
    }

    # ---- R3: wrong architecture is rejected on the PE machine field -------
    $t3 = New-Tree (Join-Path $Root 'r3')
    New-SdkVersion -KitsRoot $t3 -Version '10.0.26100.0' -Machine 0x014c | Out-Null
    $older3 = New-SdkVersion -KitsRoot $t3 -Version '10.0.22621.0' -Machine 0x8664
    $r = Invoke-Resolver -KitsRoot $t3
    $expect3 = Join-Path $older3 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect3) {
        Pass 'R3a x86 image in the newest directory rejected, older x64 selected'
    } else {
        Fail 'R3a x86 rejected' "success=[$($r.Success -join ' | ')] want=$expect3 err=$($r.Error)"
    }
    if ($r.Info -match 'machine 0x014c') {
        Pass 'R3b the rejection names the machine field (0x014c) in the audit trail'
    } else {
        Fail 'R3b rejection names machine field' "info: $($r.Info)"
    }

    $t3b = New-Tree (Join-Path $Root 'r3b')
    New-SdkVersion -KitsRoot $t3b -Version '10.0.26100.0' -Machine 0xaa64 | Out-Null
    $older3b = New-SdkVersion -KitsRoot $t3b -Version '10.0.19041.0' -Machine 0x8664
    $r = Invoke-Resolver -KitsRoot $t3b
    $expect3b = Join-Path $older3b 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect3b -and $r.Info -match 'machine 0xaa64') {
        Pass 'R3c ARM64 image rejected on its machine field, older x64 selected'
    } else {
        Fail 'R3c ARM64 rejected' "success=[$($r.Success -join ' | ')] want=$expect3b info=$($r.Info)"
    }

    # ---- R4: a newer directory with no exe loses to an older one with one --
    $t4 = New-Tree (Join-Path $Root 'r4')
    New-SdkVersion -KitsRoot $t4 -Version '10.0.26100.0' -NoExe | Out-Null
    $older4 = New-SdkVersion -KitsRoot $t4 -Version '10.0.22621.0'
    $r = Invoke-Resolver -KitsRoot $t4
    $expect4 = Join-Path $older4 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect4) {
        Pass 'R4 empty newer SDK directory skipped, older exe selected'
    } else {
        Fail 'R4 empty newer directory skipped' "success=[$($r.Success -join ' | ')] want=$expect4 err=$($r.Error)"
    }

    # ---- R5: the unversioned alias is the last rung ------------------------
    $t5a = New-Tree (Join-Path $Root 'r5a')
    $aliasDir = Join-Path (Join-Path $t5a 'bin') 'x64'
    New-PeImage -Path (Join-Path $aliasDir 'signtool.exe') -Machine 0x8664
    $r = Invoke-Resolver -KitsRoot $t5a
    $expect5a = Join-Path $aliasDir 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect5a) {
        Pass 'R5a unversioned bin\x64 alias used when no version directory qualifies'
    } else {
        Fail 'R5a alias fallback' "success=[$($r.Success -join ' | ')] want=$expect5a err=$($r.Error)"
    }

    $t5b = New-Tree (Join-Path $Root 'r5b')
    $ver5b = New-SdkVersion -KitsRoot $t5b -Version '10.0.19041.0'
    New-PeImage -Path (Join-Path (Join-Path $t5b 'bin') 'x64\signtool.exe') -Machine 0x8664
    $r = Invoke-Resolver -KitsRoot $t5b
    $expect5b = Join-Path $ver5b 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect5b) {
        Pass 'R5b a versioned directory outranks the unversioned alias'
    } else {
        Fail 'R5b versioned outranks alias' "success=[$($r.Success -join ' | ')] want=$expect5b err=$($r.Error)"
    }

    # ---- R6: non-version directories and stray files are not candidates ---
    $t6 = New-Tree (Join-Path $Root 'r6')
    New-PeImage -Path (Join-Path (Join-Path $t6 'bin') '10.0.26100\x64\signtool.exe') -Machine 0x8664
    New-PeImage -Path (Join-Path (Join-Path $t6 'bin') 'sdk-latest\x64\signtool.exe') -Machine 0x8664
    New-PeImage -Path (Join-Path (Join-Path $t6 'bin') '10.0.30000.0\x64\not-signtool.exe') -Machine 0x8664
    $wanted6 = New-SdkVersion -KitsRoot $t6 -Version '10.0.19041.0'
    $r = Invoke-Resolver -KitsRoot $t6
    $expect6 = Join-Path $wanted6 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect6) {
        Pass 'R6 3-part version, non-numeric dir and non-signtool file are not candidates'
    } else {
        Fail 'R6 non-version dirs ignored' "success=[$($r.Success -join ' | ')] want=$expect6 err=$($r.Error)"
    }

    # ---- R7: a non-PE file is rejected, an empty bin directory throws ------
    $t7 = New-Tree (Join-Path $Root 'r7')
    New-SdkVersion -KitsRoot $t7 -Version '10.0.26100.0' -GarbageExe | Out-Null
    $older7 = New-SdkVersion -KitsRoot $t7 -Version '10.0.19041.0'
    $r = Invoke-Resolver -KitsRoot $t7
    $expect7 = Join-Path $older7 'signtool.exe'
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect7 -and $r.Info -match 'not a readable PE image') {
        Pass 'R7a a non-PE file named signtool.exe is rejected, older x64 selected'
    } else {
        Fail 'R7a non-PE rejected' "success=[$($r.Success -join ' | ')] want=$expect7 info=$($r.Info)"
    }

    $t7b = New-Tree (Join-Path $Root 'r7b')
    New-Item -ItemType Directory -Path (Join-Path $t7b 'bin') -Force | Out-Null
    $r = Invoke-Resolver -KitsRoot $t7b
    if ($r.Success.Count -eq 0 -and $r.Error -match 'no supported signtool\.exe') {
        Pass 'R7b an empty bin directory throws the explicit error'
    } else {
        Fail 'R7b empty bin directory' "success=[$($r.Success -join ' | ')] err=$($r.Error)"
    }

    # ---- R8: no candidate -> terminating error naming every path ----------
    $t8 = New-Tree (Join-Path $Root 'r8')
    New-SdkVersion -KitsRoot $t8 -Version '10.0.26100.0' -Machine 0x014c | Out-Null   # x86
    New-SdkVersion -KitsRoot $t8 -Version '10.0.19041.0' -NoExe | Out-Null            # empty
    $r = Invoke-Resolver -KitsRoot $t8
    if ($r.Error -and $r.Success.Count -eq 0) {
        Pass 'R8a no supported executable throws with no path on the success stream'
    } else {
        Fail 'R8a throws with empty success stream' "success=[$($r.Success -join ' | ')] err=$($r.Error)"
    }
    $namesBoth = ($r.Error -match [regex]::Escape('10.0.26100.0')) -and
                 ($r.Error -match [regex]::Escape('10.0.19041.0')) -and
                 ($r.Error -match [regex]::Escape('x64'))
    if ($namesBoth) {
        Pass 'R8b the error names every path examined (both version directories)'
    } else {
        Fail 'R8b error names every path examined' "err: $($r.Error)"
    }
    if ($r.Error -match 'no supported signtool\.exe exists') {
        Pass 'R8c the error states explicitly that no supported executable exists'
    } else {
        Fail 'R8c explicit no-supported-executable error' "err: $($r.Error)"
    }
    $rc = Get-ExitCode -KitsRoot $t8
    if ($rc -ne 0) {
        Pass "R8d the child-process exit code is non-zero ($rc), so a workflow step fails"
    } else {
        Fail 'R8d non-zero exit code' "exit=$rc"
    }

    # ---- R9: exactly one success-stream object, an existing file ----------
    $t9 = New-Tree (Join-Path $Root 'r9')
    New-SdkVersion -KitsRoot $t9 -Version '10.0.26100.0' | Out-Null
    New-SdkVersion -KitsRoot $t9 -Version '10.0.22621.0' | Out-Null
    $r = Invoke-Resolver -KitsRoot $t9
    if ($r.Success.Count -eq 1 -and $r.Success[0] -is [string] -and
        (Test-Path -LiteralPath $r.Success[0] -PathType Leaf)) {
        Pass 'R9 the success stream is exactly one existing path (string, not array)'
    } else {
        Fail 'R9 single path on success stream' "count=$($r.Success.Count) type=$($r.Success[0].GetType().Name) value=$($r.Success -join ' | ')"
    }
    $rc = Get-ExitCode -KitsRoot $t9
    if ($rc -eq 0) {
        Pass 'R9b the child-process exit code is 0 on success'
    } else {
        Fail 'R9b zero exit code on success' "exit=$rc"
    }

    # ---- R10: the audit trail never contaminates the success stream -------
    $t10 = New-Tree (Join-Path $Root 'r10')
    New-SdkVersion -KitsRoot $t10 -Version '10.0.30000.0' -Machine 0x014c | Out-Null
    New-SdkVersion -KitsRoot $t10 -Version '10.0.26100.0' -NoExe | Out-Null
    $win10 = New-SdkVersion -KitsRoot $t10 -Version '10.0.19041.0'
    $r = Invoke-Resolver -KitsRoot $t10
    $expect10 = Join-Path $win10 'signtool.exe'
    $rejectedCount = ([regex]::Matches($r.Info, 'reject\s+SDK')).Count
    if ($r.Success.Count -eq 1 -and $r.Success[0] -eq $expect10 -and $rejectedCount -eq 2) {
        Pass "R10 two rejections recorded on the information stream, success stream still one path"
    } else {
        Fail 'R10 audit stays on the information stream' "success=[$($r.Success -join ' | ')] rejects=$rejectedCount info=$($r.Info)"
    }
    if ($r.Info -match 'signtool:' -and $r.Success[0] -notmatch 'signtool:') {
        Pass 'R10b the human-readable summary line is informational, not on the success stream'
    } else {
        Fail 'R10b summary line off the success stream' "success=[$($r.Success -join ' | ')] info=$($r.Info)"
    }

    # ---- R11: missing kits root, and a kits root with no bin --------------
    $r = Invoke-Resolver -KitsRoot (Join-Path $Root 'does-not-exist')
    if ($r.Error -and $r.Error -match 'does not exist or is not a directory' -and $r.Success.Count -eq 0) {
        Pass 'R11a a missing kits root throws naming the root'
    } else {
        Fail 'R11a missing kits root' "err=$($r.Error) success=[$($r.Success -join ' | ')]"
    }

    $t11 = New-Tree (Join-Path $Root 'r11')
    $r = Invoke-Resolver -KitsRoot $t11
    if ($r.Error -and $r.Error -match "no 'bin' directory" -and $r.Success.Count -eq 0) {
        Pass 'R11b a kits root with no bin directory throws'
    } else {
        Fail 'R11b kits root without bin' "err=$($r.Error) success=[$($r.Success -join ' | ')]"
    }

    # ---- R12: the fixture PE images are valid at the read offsets ---------
    $probe = Join-Path $Root 'probe.exe'
    New-PeImage -Path $probe -Machine 0xaa64
    $raw = [System.IO.File]::ReadAllBytes($probe)
    $peOff = [System.BitConverter]::ToInt32($raw, 0x3C)
    $sig = [System.BitConverter]::ToUInt32($raw, $peOff)
    $machineRaw = [System.BitConverter]::ToUInt16($raw, $peOff + 4)
    if ($sig -eq 0x00004550 -and $machineRaw -eq 0xaa64) {
        Pass 'R12 the fixture PE image is structurally valid at the offsets the resolver reads'
    } else {
        Fail 'R12 fixture PE image valid' ("sig=0x{0:x8} machine=0x{1:x4} at peOff=0x{2:x}" -f $sig, $machineRaw, $peOff)
    }

    # ---- GAPs: what this host cannot prove -------------------------------
    Gap 'the real runner image''s signtool.exe resolving: this host has no Windows SDK and no Windows'
    Gap 'signtool signing a package (exit 0 and a valid Authenticode signature)'
    Gap 'Add-AppxPackage install, appcert execution and wack-report.xml production'

} finally {
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
}

# ---- workflow properties (W1-W4) ------------------------------------------
# These read the checked-in workflow as text plus a real PowerShell AST parse of
# each pwsh run block. They are proved here, next to the resolver, so the
# invocation cannot drift from the resolver's contract without this failing.
$Workflow = Join-Path (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $Here))) `
    '.github/workflows/windows-build.yml'
if (-not (Test-Path -LiteralPath $Workflow -PathType Leaf)) {
    # $Here is <repo>/deploy/windows/msix; three parents is <repo>.
    $Workflow = Join-Path (Split-Path -Parent $Here) 'windows-build.yml'
}

if (-not (Test-Path -LiteralPath $Workflow -PathType Leaf)) {
    Fail 'W0 workflow file located' "not found; looked from $Here"
} else {
    Write-Host ''
    Write-Host "workflow: $Workflow"
    $text = Get-Content -Raw -LiteralPath $Workflow

    if ($text -notmatch [regex]::Escape('Windows Kits\10\bin\x64') -and
        $text -match 'resolve-signtool\.ps1') {
        Pass 'W1a the hardcoded Windows Kits\10\bin\x64 path is gone and the resolver is invoked'
    } else {
        Fail 'W1a hardcoded path removed / resolver invoked' 'workflow still names the unversioned path or not the resolver'
    }

    $captureIdx = $text.IndexOf('$signtool = & ./deploy/windows/msix/resolve-signtool.ps1')
    $invokeIdx = $text.IndexOf('& $signtool sign')
    if ($captureIdx -ge 0 -and $invokeIdx -gt $captureIdx) {
        Pass 'W1b the path is captured before the sign invocation'
    } else {
        Fail 'W1b capture precedes invocation' "capture=$captureIdx invoke=$invokeIdx"
    }
    # W1c: the capture must use the call operator against the .ps1, not
    # `pwsh -File`. Measured: `& script.ps1` with Write-Host inside yields one
    # String, while `pwsh -File script.ps1` yields an Object[] of the host lines
    # plus the path -- assigning that gives signtool an array to invoke.
    $noFileCapture = $text -notmatch '\$signtool\s*=\s*(pwsh|powershell)\s[^\n]*-File'
    $callOpCapture = $text -match '\$signtool\s*=\s*&\s*\./deploy/windows/msix/resolve-signtool\.ps1'
    if ($noFileCapture -and $callOpCapture) {
        Pass 'W1c the capture uses the call operator on the .ps1 (not "pwsh -File", which returns an array)'
    } else {
        Fail 'W1c capture idiom' "noFileCapture=$noFileCapture callOpCapture=$callOpCapture"
    }

    $signLines = @($text -split "`r?`n" | Where-Object { $_ -match '^\s*&\s+\$signtool\s+sign' })
    if ($signLines.Count -eq 1 -and $signLines[0].Trim().EndsWith(' wack-test.msix')) {
        Pass 'W2a the sign target is the throwaway copy wack-test.msix'
    } else {
        Fail 'W2a sign target is the throwaway copy' "lines=[$($signLines -join ' | ')]"
    }
    if ($text -match 'Copy-Item \$msix\.FullName wack-test\.msix') {
        Pass 'W2b the copy is taken from the downloaded package'
    } else {
        Fail 'W2b copy from the downloaded package' 'Copy-Item line not found'
    }
    $preserved = [ordered]@{
        'W2c throwaway self-signed cert created'      = 'New-SelfSignedCertificate'
        'W2d cert subject is the manifest Publisher'  = '-Subject $env:MSIX_PUBLISHER'
        'W2e cert is one-day'                         = '-NotAfter (Get-Date).AddDays(1)'
        'W2f cert lives in CurrentUser\My (/s My)'    = "-CertStoreLocation 'Cert:\CurrentUser\My'"
        'W2g signtool exit code checked'              = 'if ($LASTEXITCODE -ne 0)'
        'W2h Authenticode status read back'           = 'Get-AuthenticodeSignature wack-test.msix'
        'W3a install step present'                    = 'Add-AppxPackage -Path wack-test.msix'
        'W3b appcert run present'                     = 'appcert test -appxpackagepath'
        'W3c report parse step present'               = 'parse-wack-report.ps1'
        'W3d report uploaded with 30-day retention'   = 'retention-days: 30'
        'W3e uninstall step present'                  = 'Remove-AppxPackage'
    }
    foreach ($k in $preserved.Keys) {
        if ($text.Contains($preserved[$k])) { Pass $k } else { Fail $k "missing: $($preserved[$k])" }
    }
    $tp = ([regex]::Matches($text, 'Import-Certificate[^\r\n]*TrustedPeople')).Count
    if ($tp -eq 2 -and $text -match 'LocalMachine\\TrustedPeople' -and $text -match 'CurrentUser\\TrustedPeople') {
        Pass 'W2i the certificate is trusted in machine and user TrustedPeople'
    } else {
        Fail 'W2i TrustedPeople machine+user' "count=$tp"
    }
    # The release artifact must never be signed: no signtool outside wack-gate.
    $storeJobStart = $text.IndexOf('  store-package:')
    $wackJobStart = $text.IndexOf('  wack-gate:')
    $storeJob = if ($storeJobStart -ge 0 -and $wackJobStart -gt $storeJobStart) {
        $text.Substring($storeJobStart, $wackJobStart - $storeJobStart)
    } else { '' }
    if ($storeJob -ne '' -and $storeJob -notmatch 'signtool|New-SelfSignedCertificate') {
        Pass 'W3f the store-package job still produces the unsigned artifact and never signs it'
    } else {
        Fail 'W3f store-package unsigned' 'signing appeared inside store-package'
    }

    # ---- W4: every pwsh run block parses ----------------------------------
    $lines = $text -split "`r?`n"
    $blocks = New-Object 'System.Collections.Generic.List[object]'
    $i = 0
    while ($i -lt $lines.Count) {
        if ($lines[$i] -match '^(\s*)run:\s*\|\s*$') {
            $indent = $Matches[1].Length
            $start = $i + 1
            $body = New-Object 'System.Collections.Generic.List[string]'
            $j = $start
            while ($j -lt $lines.Count) {
                $ln = $lines[$j]
                if ($ln.Trim() -ne '' -and ($ln.Length - $ln.TrimStart().Length) -le $indent) { break }
                $body.Add($ln); $j++
            }
            $shell = 'bash'
            for ($k = $i - 1; $k -ge 0; $k--) {
                if ($lines[$k] -match '^\s*- name:') { break }
                if ($lines[$k] -match '^\s*shell:\s*(\S+)') { $shell = $Matches[1]; break }
            }
            $blocks.Add([pscustomobject]@{ Line = $start; Shell = $shell; Text = ($body -join "`n") })
            $i = $j; continue
        }
        $i++
    }
    $psBlocks = @($blocks | Where-Object { $_.Shell -eq 'pwsh' -or $_.Shell -eq 'powershell' })
    $syntaxBad = 0
    foreach ($b in $psBlocks) {
        $tokens = $null; $errs = $null
        [void][System.Management.Automation.Language.Parser]::ParseInput($b.Text, [ref]$tokens, [ref]$errs)
        if ($errs.Count -gt 0) {
            $syntaxBad++
            $msgs = ($errs | ForEach-Object { $_.Message }) -join '; '
            Fail ("W4 pwsh block at workflow line {0} parses" -f $b.Line) $msgs
        }
    }
    if ($psBlocks.Count -ge 5 -and $syntaxBad -eq 0) {
        Pass ("W4 all {0} pwsh run blocks parse under the PowerShell parser" -f $psBlocks.Count)
    } elseif ($psBlocks.Count -lt 5) {
        Fail 'W4 pwsh block discovery' "found only $($psBlocks.Count) pwsh blocks; expected >= 5"
    } else {
        Fail 'W4 pwsh blocks parse' "$syntaxBad block(s) failed"
    }
}

Write-Host ''
$total = $script:Passed + $script:Failed
Write-Host "== $($script:Passed)/$total properties passed, $($script:Failed) failed, $($script:Gaps) reported as GAP"

if ($script:Failed -eq 0) {
    Write-Host ("RESULT resolver selection and negative behaviour proved against synthetic kits trees; " +
        "the real SDK, signing, install and WACK report remain the Captain's hosted checks")
    exit 0
}
Write-Host 'RESULT failures above'
exit 1
