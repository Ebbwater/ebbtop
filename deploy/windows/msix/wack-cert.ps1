<#
.SYNOPSIS
Validate the throwaway certificate the WACK gate signs with.

.DESCRIPTION
The WACK gate signs a copy of the unsigned MSIX with a one-day self-signed
certificate so the package can be installed and handed to appcert. SignTool
does the certificate selection, and its filter is narrower than it looks: for
`sign` it opens the store given by `/s` (default `My`), keeps only certificates
that carry the Code Signing EKU, are inside their validity window, and have a
usable private key, and then applies the selector.

The selector used before this file existed was `/n $env:MSIX_PUBLISHER` with
`MSIX_PUBLISHER=CN=32700780-5AAE-4D15-A591-B49543B86C16`. SignTool documents
`/n` as "the name of the subject of the signing certificate ... a substring of
the entire subject name", and the Win32 find it is built on
(`CertFindCertificateInStore` with `CERT_FIND_SUBJECT_STR`) documents that "the
certificate's subject member is converted to a name string ... formatted as
CERT_SIMPLE_NAME_STR", where `CERT_SIMPLE_NAME_STR` means "all object
identifiers (OIDs) are discarded" and "RDN entries are separated by a comma
followed by a space". The rendered subject of that certificate is therefore

    32700780-5AAE-4D15-A591-B49543B86C16

and the query string `CN=32700780-5AAE-...` is not a substring of it, because
the attribute keyword `CN=` was discarded in the rendering. That is the
documented behaviour behind "No certificates were found that met all the given
criteria" with no other filter left to blame.

So the gate no longer selects by name. It creates the certificate, then selects
the exact certificate it just created by thumbprint (`/sha1`), and it refuses to
invoke SignTool unless the certificate in the store satisfies every condition
SignTool is documented to filter on. The checks are ordinary X509 properties, so
they are testable without Windows:

  * the store entry has a private key (SignTool's private-key filter);
  * the Code Signing EKU 1.3.6.1.5.5.7.3.3 is present (SignTool's EKU filter);
  * the certificate is inside its validity window (SignTool's expiry filter);
  * if a Key Usage extension is present it allows DigitalSignature;
  * the subject is exactly the manifest Publisher DN, compared ordinal, so a
    certificate that would make Add-AppxPackage or SignerSign refuse the
    package is caught before any bytes are signed.

This file defines functions only. It has no side effects, never touches a
certificate store, and is safe to dot-source. `test-wack-cert.ps1` exercises it.
#>

function Test-WackSigningCertificate {
    <#
    .SYNOPSIS
    Report whether a certificate can be used to sign the WACK package copy.

    .PARAMETER Certificate
    The certificate to judge, normally the store entry looked up by thumbprint.

    .PARAMETER ExpectedSubject
    The manifest Publisher DN. The certificate subject must equal it exactly.

    .PARAMETER CodeSigningOid
    EKU SignTool requires. Defaults to 1.3.6.1.5.5.7.3.3 (Code Signing).

    .PARAMETER Now
    Reference time for the validity window. Defaults to the current time.

    .OUTPUTS
    An object with Ok (bool), Reasons (string[]), Subject and Thumbprint.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
        [Parameter(Mandatory = $true)][string]$ExpectedSubject,
        [string]$CodeSigningOid = '1.3.6.1.5.5.7.3.3',
        [datetime]$Now = (Get-Date)
    )

    $reasons = New-Object 'System.Collections.Generic.List[string]'

    # SignTool's private-key filter. A certificate without a reachable key can
    # be selected by name but cannot sign, so it is not a usable signer.
    if (-not $Certificate.HasPrivateKey) {
        $reasons.Add('the certificate has no private key in this store')
    }

    # SignTool's EKU filter. The extension must be present and must carry the
    # Code Signing OID; a certificate with no EKU extension, or with a
    # different EKU only, leaves zero candidates after that filter.
    $ekuExt = @($Certificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.37' })
    if ($ekuExt.Count -eq 0) {
        $reasons.Add("no Enhanced Key Usage extension, so the Code Signing EKU ($CodeSigningOid) is absent")
    } else {
        $oids = @()
        try {
            $oids = @($ekuExt[0].EnhancedKeyUsages | ForEach-Object { $_.Value })
        } catch {
            $oids = @()
        }
        if ($oids -notcontains $CodeSigningOid) {
            $reasons.Add(("Enhanced Key Usage does not contain the Code Signing EKU {0} (has: {1})" -f
                    $CodeSigningOid, (($oids -join ', '))))
        }
    }

    # SignTool's expiry filter.
    if ($Now -lt $Certificate.NotBefore) {
        $reasons.Add(("the certificate is not valid yet (NotBefore {0:o})" -f $Certificate.NotBefore))
    }
    if ($Now -gt $Certificate.NotAfter) {
        $reasons.Add(("the certificate has expired (NotAfter {0:o})" -f $Certificate.NotAfter))
    }

    # If a Key Usage extension is present it must allow DigitalSignature. An
    # absent extension is not a rejection here; the EKU check above is the one
    # SignTool enforces.
    $kuExt = @($Certificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.15' })
    if ($kuExt.Count -gt 0) {
        try {
            $ku = [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]$kuExt[0]
            if (-not ($ku.KeyUsages -band [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature)) {
                $reasons.Add(("the Key Usage extension does not allow DigitalSignature (has: {0})" -f $ku.KeyUsages))
            }
        } catch {
            $reasons.Add('the Key Usage extension could not be decoded')
        }
    }

    # Subject alignment with Package/Identity/@Publisher. Ordinal, not
    # case-insensitive and not a substring: the manifest Publisher and the
    # certificate subject must be the same DN for SignerSign and
    # Add-AppxPackage to accept the package.
    if (-not [string]::Equals($Certificate.Subject, $ExpectedSubject, [System.StringComparison]::Ordinal)) {
        $reasons.Add(("the subject '{0}' is not exactly the manifest Publisher '{1}'" -f
                $Certificate.Subject, $ExpectedSubject))
    }

    [pscustomobject]@{
        Ok         = ($reasons.Count -eq 0)
        Reasons    = @($reasons)
        Subject    = $Certificate.Subject
        Thumbprint = $Certificate.Thumbprint
    }
}

function Get-WackSigningCertThumbprint {
    <#
    .SYNOPSIS
    Look the certificate up in the store SignTool will read, then validate it.

    .DESCRIPTION
    Re-fetching by thumbprint is deliberate: it proves the certificate the gate
    is about to select is the one actually present in `Cert:\CurrentUser\My`
    (the store `/s My` opens), not merely an object still in memory. A missing
    entry is a terminating error, so the step fails before SignTool runs rather
    than after it reports an empty candidate set.

    .PARAMETER StorePath
    Store to look in. Defaults to the current user's Personal store.

    .OUTPUTS
    The thumbprint, as a string.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Thumbprint,
        [Parameter(Mandatory = $true)][string]$ExpectedSubject,
        [string]$StorePath = 'Cert:\CurrentUser\My'
    )

    $normalised = ($Thumbprint -replace '\s', '').ToUpperInvariant()
    if ($normalised -notmatch '^[0-9A-F]{40}$') {
        throw "Not a SHA-1 certificate thumbprint (40 hex digits): '$Thumbprint'"
    }

    $certPath = Join-Path $StorePath $normalised
    if (-not (Test-Path -LiteralPath $certPath)) {
        throw (("The certificate {0} is not in {1}, so SignTool's /sha1 selection would find nothing there. " +
                'The certificate must be created in, or imported to, the same store the sign step opens.') -f
            $normalised, $StorePath)
    }

    $cert = Get-Item -LiteralPath $certPath
    $check = Test-WackSigningCertificate -Certificate $cert -ExpectedSubject $ExpectedSubject
    if (-not $check.Ok) {
        throw ("The certificate {0} in {1} cannot sign the package: {2}" -f
            $normalised, $StorePath, (($check.Reasons) -join '; '))
    }
    return $normalised
}
