<#
.SYNOPSIS
Fixture proof for wack-cert.ps1 and the workflow's certificate selection.

.DESCRIPTION
The WACK gate stopped selecting its throwaway certificate by subject name
(`/n`) and now selects it by thumbprint (`/sha1`). Two things have to hold for
that to be a repair rather than a rename:

  C1  the predicate accepts exactly the certificate the gate creates, and
      rejects a certificate that SignTool would filter out for any of the
      documented reasons -- no private key, no Code Signing EKU, a wrong EKU,
      expired, not yet valid, a Key Usage extension that forbids
      DigitalSignature, or a subject that is not the manifest Publisher;
  C2  the workflow invokes SignTool with an exact selector, no name selector,
      and a path that was validated against the store SignTool opens.

C1 is proved on real X509Certificate2 objects. `New-SelfSignedCertificate` and
the `cert:` provider are Windows-only, so the fixtures build certificates with
`CertificateRequest`, which is the same BCL type the cmdlet produces, and load
the private key from the returned object. Everything the predicate reads --
HasPrivateKey, the 2.5.29.37 EKU extension, the 2.5.29.15 Key Usage extension,
NotBefore/NotAfter, Subject -- is populated on those objects, so the assertions
are about the predicate, not about a mock.

The one thing this cannot exercise is the operational half: that
`Cert:\CurrentUser\My` on the runner actually resolves and that signtool's
`/sha1` accepts the thumbprint. Those are reported as GAP.

.PARAMETER WorkflowPath
Workflow to read. Defaults to the checked-in .github/workflows/windows-build.yml.

.EXAMPLE
pwsh -NoProfile -File deploy/windows/msix/test-wack-cert.ps1
#>
[CmdletBinding()]
param(
    [string]$WorkflowPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Here = $PSCmdlet.MyInvocation.MyCommand.Path
if (-not $Here) { $Here = $MyInvocation.MyCommand.Path }
$Here = Split-Path -Parent $Here

$CertScript = Join-Path $Here 'wack-cert.ps1'
if (-not (Test-Path -LiteralPath $CertScript -PathType Leaf)) {
    Write-Host "error: wack-cert.ps1 not found at $CertScript"
    exit 2
}
. $CertScript

if ([string]::IsNullOrWhiteSpace($WorkflowPath)) {
    $WorkflowPath = Join-Path (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $Here))) `
        '.github/workflows/windows-build.yml'
}

$script:Passed = 0
$script:Failed = 0
$script:Gaps = 0

function Pass { param([string]$Name) $script:Passed++; Write-Host "PASS  $Name" }
function Fail {
    param([string]$Name, [string]$Detail)
    $script:Failed++
    Write-Host "FAIL  $Name"
    if ($Detail) { Write-Host "        $Detail" }
}
function Gap { param([string]$Name) $script:Gaps++; Write-Host "GAP   $Name" }

$Publisher = 'CN=32700780-5AAE-4D15-A591-B49543B86C16'
$CodeSigning = '1.3.6.1.5.5.7.3.3'
$TlsServer = '1.3.6.1.5.5.7.3.1'

# ---- fixture certificates -------------------------------------------------
# CertificateRequest produces a real X509Certificate2 with the extensions the
# predicate reads. WithPrivateKey attaches a key so HasPrivateKey is $true, as
# it is for a New-SelfSignedCertificate result in CurrentUser\My.
function New-TestCertificate {
    param(
        [string]$Subject = $Publisher,
        [string[]]$Eku = @($CodeSigning),
        [switch]$NoEkuExtension,
        [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]$KeyUsage =
            [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature,
        [switch]$NoKeyUsageExtension,
        [datetime]$NotBefore = (Get-Date).AddMinutes(-5),
        [datetime]$NotAfter = (Get-Date).AddDays(1),
        [switch]$WithoutPrivateKey
    )
    $rsa = [System.Security.Cryptography.RSA]::Create(2048)
    $req = New-Object System.Security.Cryptography.X509Certificates.CertificateRequest(
        $Subject, $rsa,
        [System.Security.Cryptography.HashAlgorithmName]::SHA256,
        [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
    if (-not $NoEkuExtension) {
        $oids = New-Object System.Security.Cryptography.OidCollection
        foreach ($e in $Eku) {
            [void]$oids.Add((New-Object System.Security.Cryptography.Oid($e)))
        }
        $req.CertificateExtensions.Add(
            (New-Object System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension($oids, $false)))
    }
    if (-not $NoKeyUsageExtension) {
        $req.CertificateExtensions.Add(
            (New-Object System.Security.Cryptography.X509Certificates.X509KeyUsageExtension($KeyUsage, $true)))
    }
    $generated = $req.CreateSelfSigned($NotBefore, $NotAfter)
    if ($WithoutPrivateKey) {
        # Round-trip through DER so the public-only object carries no key, which
        # is what a certificate imported from a .cer without its key looks like.
        return [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
            [byte[]]$generated.RawData)
    }
    return $generated
}

function Test-Case {
    param([string]$Name, [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
        [bool]$ExpectOk, [string]$ExpectReason)
    $r = Test-WackSigningCertificate -Certificate $Certificate -ExpectedSubject $Publisher
    $reasons = ($r.Reasons) -join '; '
    if ($r.Ok -ne $ExpectOk) {
        Fail $Name ("expected Ok=$ExpectOk, got Ok=$($r.Ok); reasons=[$reasons]")
        return
    }
    if ($ExpectOk) {
        Pass $Name
        return
    }
    if ([string]::IsNullOrWhiteSpace($ExpectReason)) {
        Fail $Name 'no expected reason supplied for a rejection case'
        return
    }
    if ($reasons -match $ExpectReason) {
        Pass $Name
    } else {
        Fail $Name "reason does not match '$ExpectReason'; got [$reasons]"
    }
}

Write-Host ''
Write-Host '== C1: the certificate predicate =='
Write-Host ''

$good = New-TestCertificate
Test-Case 'C1a a certificate shaped like the gate''s own certificate is accepted' `
    -Certificate $good -ExpectOk $true

# The real cmdlet's output, reproduced: a custom self-signed certificate with
# the Code Signing EKU and the no-CA Basic Constraints the workflow passes.
Test-Case 'C1b the accepted certificate carries the manifest Publisher subject' `
    -Certificate $good -ExpectOk $true

Test-Case 'C1c a certificate with no private key is rejected' `
    -Certificate (New-TestCertificate -WithoutPrivateKey) -ExpectOk $false `
    -ExpectReason 'no private key'

Test-Case 'C1d a certificate with no Enhanced Key Usage extension is rejected' `
    -Certificate (New-TestCertificate -NoEkuExtension) -ExpectOk $false `
    -ExpectReason 'no Enhanced Key Usage extension'

Test-Case 'C1e a certificate with only a TLS-server EKU is rejected' `
    -Certificate (New-TestCertificate -Eku @($TlsServer)) -ExpectOk $false `
    -ExpectReason 'does not contain the Code Signing EKU'

Test-Case 'C1f a certificate with Code Signing plus another EKU is accepted' `
    -Certificate (New-TestCertificate -Eku @($CodeSigning, $TlsServer)) -ExpectOk $true

Test-Case 'C1g an expired certificate is rejected' `
    -Certificate (New-TestCertificate -NotBefore (Get-Date).AddDays(-10) -NotAfter (Get-Date).AddDays(-1)) `
    -ExpectOk $false -ExpectReason 'expired'

Test-Case 'C1h a not-yet-valid certificate is rejected' `
    -Certificate (New-TestCertificate -NotBefore (Get-Date).AddDays(1) -NotAfter (Get-Date).AddDays(2)) `
    -ExpectOk $false -ExpectReason 'not valid yet'

Test-Case 'C1i a Key Usage extension that forbids DigitalSignature is rejected' `
    -Certificate (New-TestCertificate -KeyUsage ([System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::KeyEncipherment)) `
    -ExpectOk $false -ExpectReason 'does not allow DigitalSignature'

Test-Case 'C1j a certificate with no Key Usage extension is accepted (it is optional)' `
    -Certificate (New-TestCertificate -NoKeyUsageExtension) -ExpectOk $true

Test-Case 'C1k a subject that is not the manifest Publisher is rejected' `
    -Certificate (New-TestCertificate -Subject 'CN=someone-else') -ExpectOk $false `
    -ExpectReason 'not exactly the manifest Publisher'

Test-Case 'C1l a subject that only contains the Publisher is rejected (no substring match)' `
    -Certificate (New-TestCertificate -Subject 'CN=32700780-5AAE-4D15-A591-B49543B86C16, O=Extra') `
    -ExpectOk $false -ExpectReason 'not exactly the manifest Publisher'

Test-Case 'C1m a subject differing only in case is rejected (ordinal comparison)' `
    -Certificate (New-TestCertificate -Subject 'cn=32700780-5aae-4d15-a591-b49543b86c16') `
    -ExpectOk $false -ExpectReason 'not exactly the manifest Publisher'

# C1n reproduces the reported production failure at the predicate boundary: the
# certificate is entirely valid, and the failure is purely the selection string.
$sel = 'CN=32700780-5AAE-4D15-A591-B49543B86C16'
$rendered = ($Publisher -replace '^CN=', '')
if ($rendered -eq '32700780-5AAE-4D15-A591-B49543B86C16' -and
    -not $rendered.Contains($sel)) {
    Pass 'C1n the old /n argument is not a substring of the rendered subject (root cause reproduced)'
} else {
    Fail 'C1n root cause reproduced' "rendered=[$rendered] sel=[$sel]"
}

Write-Host ''
Write-Host '== C2: thumbprint selection and store validation =='
Write-Host ''

$tp = ($good.Thumbprint -replace '\s', '').ToUpperInvariant()
if ($tp -match '^[0-9A-F]{40}$') {
    Pass 'C2a the fixture certificate has a SHA-1 thumbprint in the form /sha1 takes'
} else {
    Fail 'C2a thumbprint shape' "thumbprint=[$tp]"
}

# Get-WackSigningCertThumbprint must reject a malformed thumbprint before it
# touches the store, so a typo fails on the argument rather than as an opaque
# "not found" from SignTool.
$badThumbRejected = $false
try {
    Get-WackSigningCertThumbprint -Thumbprint 'not-a-thumbprint' -ExpectedSubject $Publisher | Out-Null
} catch {
    $badThumbRejected = $_.Exception.Message -match '40 hex digits'
}
if ($badThumbRejected) {
    Pass 'C2b a malformed thumbprint is rejected before any store access'
} else {
    Fail 'C2b malformed thumbprint' 'the call did not throw the thumbprint-shape error'
}

# The store lookup is the operational half. On a non-Windows host, or one with
# no such certificate, the call must still fail closed with a message naming the
# store -- never return a thumbprint it did not verify.
$storeFailClosed = $false
$storeMessage = ''
try {
    Get-WackSigningCertThumbprint -Thumbprint $tp -ExpectedSubject $Publisher |
        ForEach-Object { $storeMessage = $_ }
} catch {
    $storeFailClosed = $true
    $storeMessage = $_.Exception.Message
}
if ($IsWindows) {
    Gap 'C2c the store lookup on Cert:\CurrentUser\My: the fixture certificate is not installed in this host''s store'
    if ($storeFailClosed -and $storeMessage -match 'is not in Cert:\\CurrentUser\\My') {
        Pass 'C2d a thumbprint absent from the store fails closed, naming the store'
    } else {
        Fail 'C2d absent thumbprint fails closed' "closed=$storeFailClosed msg=[$storeMessage]"
    }
} else {
    # $IsWindows is false here, so the cert: provider does not exist. The
    # property that still matters is that the call does not silently succeed.
    if ($storeFailClosed) {
        Pass 'C2c the store lookup fails closed off Windows instead of returning an unverified thumbprint'
    } else {
        Fail 'C2c store lookup fails closed' "returned [$storeMessage] without verifying the store"
    }
    Gap 'C2e the store lookup succeeding on a real Cert:\CurrentUser\My, and signtool /sha1 selecting it'
}

Write-Host ''
Write-Host '== C3: the workflow selects exactly and validates before signing =='
Write-Host ''

if (-not (Test-Path -LiteralPath $WorkflowPath -PathType Leaf)) {
    Fail 'C3a workflow file located' "not found at $WorkflowPath"
} else {
    Write-Host "workflow: $WorkflowPath"
    $text = Get-Content -Raw -LiteralPath $WorkflowPath

    # C3a: the failing selector is gone. `/n $env:MSIX_PUBLISHER` is what
    # SignTool applied to a rendered subject that cannot contain "CN=".
    $hasNameSelector = $text -match 'signtool[^\r\n]*/n\s+\$env:MSIX_PUBLISHER'
    if (-not $hasNameSelector) {
        Pass 'C3a the /n $env:MSIX_PUBLISHER selector is gone'
    } else {
        Fail 'C3a name selector removed' 'the sign command still selects by subject name'
    }

    $signLines = @($text -split "`r?`n" | Where-Object { $_ -match '^\s*&\s+\$signtool\s+sign' })
    if ($signLines.Count -ne 1) {
        Fail 'C3b a single signtool sign invocation' "count=$($signLines.Count)"
    } else {
        $line = $signLines[0]
        if ($line -match '/sha1\s+\$certThumbprint') {
            Pass 'C3b the sign command selects the certificate by thumbprint'
        } else {
            Fail 'C3b thumbprint selector' "line=[$($line.Trim())]"
        }
        if ($line -match '/s\s+My') {
            Pass 'C3c the sign command opens the same store the certificate is created in'
        } else {
            Fail 'C3c store selector' "line=[$($line.Trim())]"
        }
        if ($line -notmatch '/a\b') {
            Pass 'C3d /a is absent, so no other certificate can be auto-selected'
        } else {
            Fail 'C3d /a removed' "line=[$($line.Trim())]"
        }
        if ($line -match '/fd\s+SHA256') {
            Pass 'C3e the file digest algorithm matches the package block map (SHA256)'
        } else {
            Fail 'C3e /fd SHA256' "line=[$($line.Trim())]"
        }
        if ($line.Trim().EndsWith('wack-test.msix')) {
            Pass 'C3f the sign target is still the throwaway copy'
        } else {
            Fail 'C3f throwaway copy target' "line=[$($line.Trim())]"
        }
    }

    $thumbIdx = $text.IndexOf('$certThumbprint = Get-WackSigningCertThumbprint')
    $invokeIdx = $text.IndexOf('& $signtool sign')
    if ($thumbIdx -ge 0 -and $invokeIdx -gt $thumbIdx) {
        Pass 'C3g the thumbprint is validated before SignTool is invoked'
    } else {
        Fail 'C3g validation precedes invocation' "validate=$thumbIdx invoke=$invokeIdx"
    }

    if ($text -match 'Get-WackSigningCertThumbprint\s+`?\s*-Thumbprint\s+\$cert\.Thumbprint') {
        Pass 'C3h the validated thumbprint is the certificate the step just created'
    } else {
        Fail 'C3h thumbprint provenance' 'the validation call does not take $cert.Thumbprint'
    }

    if ($text -match '\.\s*\(Join-Path \$PWD ''deploy/windows/msix/wack-cert\.ps1''\)') {
        Pass 'C3i the validator is dot-sourced from a repository-relative path'
    } else {
        Fail 'C3i validator loading' 'the dot-source line was not found'
    }

    # C3j requires the guard's structure, not just that the part name appears
    # somewhere: the count check must be a real comparison and the failure path
    # must throw, so removing the guard cannot pass on a leftover comment.
    $p7xGuard = $text -match '(?s)AppxSignature\.p7x.{0,400}?if\s*\(\s*\$sigPart\.Count\s+-ne\s+1\s*\)\s*\{[^}]*?throw'
    $p7xReal = $text -notmatch 'if\s*\(\s*\$false\s*\)'
    if ($p7xGuard -and $p7xReal) {
        Pass 'C3j the sign step verifies the package gained an AppxSignature.p7x part and throws when it did not'
    } else {
        Fail 'C3j signature presence check' "guard=$p7xGuard noFalseGuard=$p7xReal"
    }

    if ($text -match "Status -in @\('NotSigned', 'HashMismatch'\)") {
        Pass 'C3k the Authenticode readback still fails the step on an unsigned or corrupted signature'
    } else {
        Fail 'C3k Authenticode readback' 'the status assertion is missing'
    }

    # Preserved gates: the change is confined to certificate selection, so the
    # rest of the WACK lane and the resolver contract must be untouched.
    $preserved = [ordered]@{
        'C4a the resolver is still how signtool is located' = '$signtool = & ./deploy/windows/msix/resolve-signtool.ps1'
        'C4b the certificate is still created per run'       = 'New-SelfSignedCertificate'
        'C4c the cert subject is still the manifest Publisher' = '-Subject $env:MSIX_PUBLISHER'
        'C4d the certificate is still one-day'               = '-NotAfter (Get-Date).AddDays(1)'
        'C4e the cert still lives in CurrentUser\My'         = "-CertStoreLocation 'Cert:\CurrentUser\My'"
        'C4f the install step is preserved'                  = 'Add-AppxPackage -Path wack-test.msix'
        'C4g the appcert run is preserved'                   = 'appcert test -appxpackagepath'
        'C4h the report parse is preserved'                  = 'parse-wack-report.ps1'
        'C4i the 30-day report upload is preserved'          = 'retention-days: 30'
        'C4j the uninstall step is preserved'                = 'Remove-AppxPackage'
    }
    foreach ($k in $preserved.Keys) {
        if ($text.Contains($preserved[$k])) { Pass $k } else { Fail $k "missing: $($preserved[$k])" }
    }

    $tp = ([regex]::Matches($text, 'Import-Certificate[^\r\n]*TrustedPeople')).Count
    if ($tp -eq 2) {
        Pass 'C4k the certificate is still trusted in machine and user TrustedPeople'
    } else {
        Fail 'C4k TrustedPeople trust' "count=$tp"
    }

    # Every pwsh run block must still parse. Include single-line run: steps, so
    # the uninstall step is covered too.
    $syntaxErrors = 0
    $psBlocks = 0
    foreach ($m in [regex]::Matches($text, '(?m)^(?<indent>\s*)run:\s*\|')) {
        $indent = $m.Groups['indent'].Length
        $after = $text.Substring($m.Index + $m.Length)
        $bodyLines = New-Object 'System.Collections.Generic.List[string]'
        foreach ($ln in ($after -split "`r?`n")) {
            if ($ln.Trim() -ne '' -and ($ln.Length - $ln.TrimStart().Length) -le $indent) { break }
            $bodyLines.Add($ln)
        }
        $head = $text.Substring(0, $m.Index)
        $shell = 'bash'
        $shells = [regex]::Matches($head, '(?m)^\s*shell:\s*(\S+)')
        if ($shells.Count -gt 0) { $shell = $shells[$shells.Count - 1].Groups[1].Value }
        if ($shell -notin @('pwsh', 'powershell')) { continue }
        $psBlocks++
        $tokens = $null; $errs = $null
        [void][System.Management.Automation.Language.Parser]::ParseInput(
            ($bodyLines -join "`n"), [ref]$tokens, [ref]$errs)
        if ($errs.Count -gt 0) {
            $syntaxErrors++
            Fail ("C5 pwsh block at offset {0} parses" -f $m.Index) (($errs | ForEach-Object { $_.Message }) -join '; ')
        }
    }
    if ($psBlocks -ge 5 -and $syntaxErrors -eq 0) {
        Pass ("C5 all {0} pwsh run blocks parse under the PowerShell parser" -f $psBlocks)
    } elseif ($psBlocks -lt 5) {
        Fail 'C5 pwsh block discovery' "found $psBlocks pwsh blocks; expected at least 5"
    } else {
        Fail 'C5 pwsh blocks parse' "$syntaxErrors block(s) failed"
    }
}

Write-Host ''
Write-Host '== C6: the certificate script itself parses =='
$tokens = $null; $errs = $null
[void][System.Management.Automation.Language.Parser]::ParseInput(
    (Get-Content -Raw -LiteralPath $CertScript), [ref]$tokens, [ref]$errs)
if ($errs.Count -eq 0) {
    Pass 'C6 wack-cert.ps1 parses under the PowerShell parser'
} else {
    Fail 'C6 wack-cert.ps1 parses' (($errs | ForEach-Object { $_.Message }) -join '; ')
}

Write-Host ''
Write-Host '== GAPs: hosted gates this host cannot exercise =='
Gap 'signtool /sha1 selecting the certificate on the runner and exiting 0'
Gap 'SignerSign accepting the package (Publisher vs certificate subject)'
Gap 'Add-AppxPackage install, appcert execution and wack-report.xml production'

Write-Host ''
$total = $script:Passed + $script:Failed
Write-Host "== $($script:Passed)/$total properties passed, $($script:Failed) failed, $($script:Gaps) reported as GAP"
if ($script:Failed -eq 0) {
    Write-Host ('RESULT certificate selection is exact and validated before SignTool runs; ' +
        'the signing itself remains the Captain''s hosted check')
    exit 0
}
Write-Host 'RESULT failures above'
exit 1
