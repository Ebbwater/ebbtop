# EbbTop Code Signing Policy

This document states how EbbTop artifacts are signed today, how a signed
artifact traces back to its source, and what is unsigned. It describes the
current posture, not a plan.

## The short version

- **GitHub downloads are unsigned.** The Windows executables, the Windows ZIP
  and the Linux agent archive on [Releases](https://github.com/Ebbwater/ebbtop/releases)
  carry no Authenticode signature. Integrity comes from the published
  `SHA256SUMS` over HTTPS (and the archive's own internal manifest).
- **The Microsoft Store channel is signed by Microsoft.** The Store takes the
  MSIX we submit and re-signs it with a Microsoft certificate during
  publishing. The signer shown to users is Microsoft; the publisher shown on
  the listing is Ebbwater.
- **No code signing certificate is currently purchased.** There is no OV or EV
  certificate, no cloud HSM subscription, and no pending application.

## Why the GitHub downloads are unsigned

EbbTop applied to the [SignPath Foundation](https://signpath.org) open source
signing program. SignPath declined the application: at the time of review the
project did not have the download reputation their criteria require. That
route is closed, and no certificate authority product has been purchased
instead.

The practical consequence, stated plainly: a browser download of `ebbtop.exe`
may show a SmartScreen warning, and Defender's ML heuristics may quarantine it
until reputation accumulates. Verify the download against `SHA256SUMS`; if it
is a false positive, excluding it is the machine owner's decision. If this
posture ever changes, this document changes in the same commit as the build
change.

The Store channel avoids the problem: Microsoft signs the MSIX, so a Store
install carries a trusted signature.

## The Microsoft Store channel

- Package: MSIX for a packaged Win32 desktop app (`ebbtop.exe`, x64), built by
  `.github/workflows/windows-build.yml` (`store-package` job) from the tagged
  release commit with the `store` cargo feature enabled, so the app carries no
  self-updater.
- Identity, issued by Partner Center when the app name was reserved:

  | Field | Value |
  |---|---|
  | Package/Identity Name | `Ebbwater.EbbTop` |
  | Publisher | `CN=32700780-5AAE-4D15-A591-B49543B86C16` |
  | Package Family Name | `Ebbwater.EbbTop_46f4w9xy8kqdr` |
  | Store ID | `9PJJXH1SBQ23` |

  Source of truth: `deploy/windows/msix/AppxManifest.xml`.
- The package we produce is **unsigned**. The Store re-signs it; a self-signed
  package would be rejected at upload, so the workflow has no signing step.
- The `wack-gate` job does sign a copy locally, with a throwaway self-signed
  certificate whose subject is the Publisher DN above, only to side-load the
  package on a disposable runner and run Microsoft's Windows App Certification
  Kit. That certificate is created and discarded inside one job; it never
  signs a release artifact and is never published.
  - SignTool is located by `deploy/windows/msix/resolve-signtool.ps1`, not by a
    fixed path. The SDK installs it under a version-stamped directory,
    `%ProgramFiles(x86)%\Windows Kits\10\bin\<sdk version>\x64\signtool.exe`,
    and the unversioned `bin\x64` alias is not guaranteed: the
    `windows-2025-vs2026` image `20261116.315` publishes exactly one installed
    SDK (`10.0.26100.0`) and carries no `bin\x64\signtool.exe`, which is where
    a hardcoded path failed before the gate could install anything. The
    resolver walks the version directories newest-first by numeric version,
    accepts only a readable PE image whose COFF machine field is AMD64
    (`0x8664`), falls back to `bin\x64` last, and throws — naming every path it
    examined — rather than returning an empty path.
  - `deploy/windows/msix/test-resolve-signtool.ps1` is the fixture proof for
    that resolver and for the workflow's sign-step shape. It runs on any host
    with PowerShell; the real SDK resolution, the signing call, the install and
    the WACK report remain hosted checks.
  - The signing certificate is selected by **thumbprint** (`/sha1`), not by
    subject name. SignTool documents `/n` as a substring of the subject, and the
    Win32 find behind it renders the subject as `CERT_SIMPLE_NAME_STR`, which
    discards the attribute OIDs — so the rendered subject of this certificate is
    the bare GUID `32700780-5AAE-4D15-A591-B49543B86C16`, and
    `/n CN=32700780-...` matched nothing ("No certificates were found that met
    all the given criteria"). `deploy/windows/msix/wack-cert.ps1` re-reads the
    created certificate from `Cert:\CurrentUser\My`, refuses it unless it has a
    private key, the Code Signing EKU, a current validity window, a Key Usage
    extension permitting DigitalSignature, and a subject equal to the manifest
    Publisher exactly, and returns its thumbprint for `/sha1`.
    `test-wack-cert.ps1` is the fixture proof; the store lookup and the signing
    call remain hosted checks.

What the Microsoft signature means: the bytes match the package we submitted
and Microsoft vouches they have not been altered since. It is not an audit of
the source.

## Build provenance

Every package comes from the project's public GitHub Actions workflow at the
tagged release commit in
[github.com/Ebbwater/ebbtop](https://github.com/Ebbwater/ebbtop):

- The `windows-x64`, `store-package` and `wack-gate` jobs run on GitHub-hosted
  runners (`ubuntu-latest`, `windows-latest`), triggered by `v*` tags or manual
  dispatch only, so nothing runs for an ordinary push.
- The workflow and the scripts it runs (`deploy/cross-build-windows.sh`,
  `deploy/windows/msix/package-msix.sh`, `deploy/windows/msix/make-assets.py`,
  `deploy/windows/msix/verify-msix.py`) live in the repository and are
  reviewed like any other change.
- The package records its commit in `dist/msix/BUILD`. The packaging script
  re-reads product name and version out of the built PE
  (`deploy/windows/versioninfo.py`) and refuses a mismatch with the manifest.
- `deploy/windows/msix/verify-msix.py` validates the packed bytes: OPC parts,
  manifest shape, every referenced asset present, every block-map hash over the
  packaged files, that the package is unsigned, that the identity is the Partner
  Center identity, that the declared resource languages are exactly the three
  the client ships, and that `LICENSE-OFL.txt` beside `ebbtop.exe` is
  byte-identical to the checked-in OFL notice for the embedded fonts.
- The MSIX SDK used to pack is pinned by commit
  (`deploy/windows/msix/bootstrap-makemsix.sh`) and its source worktree digest
  is checked before compiling; a mismatch aborts before `cmake`/`make`. The
  digest is computed under a forced `C` collation, with `LC_ALL=C` exported for
  the whole pipeline and `sort -z` ordering the file list, because `sort`
  otherwise collates in the caller's locale: `en_US.UTF-8` orders the same
  paths differently from `C`, so identical bytes would have produced a
  different digest and the gate would refuse a pristine tree. Reproduce the
  pinned value with:

      ( cd <sdk-tree> && export LC_ALL=C LANG=C LC_COLLATE=C
        find . -path ./.git -prune -o -type f -print0 | sort -z \
          | xargs -0 sha256sum | sha256sum | cut -d' ' -f1 )

  The `49c28612…` value that preceded the current pin came from that
  locale-dependent collation, not from different source bytes: the same
  21117-file worktree yields `5b850191…` under `C` and `49c28612…` under an
  `en_US.UTF-8` request. Filesystem readdir order and the four dropped
  vendored-openssl gitlinks are not involved (the file list is sorted before
  hashing, and `git rm --cached` leaves worktree bytes alone), and this commit
  has no root `.gitmodules`, so the recursive submodule step cannot change the
  file list; where the old pin was first computed is not recorded and remains
  a hypothesis. `deploy/windows/msix/test-sdk-digest.sh` proves, offline and
  with no network, that a matching tree is accepted, a one-byte change is
  refused with both digests printed, a wrong pin is refused, and the value is
  the same under `LANG`/`LC_ALL` changes and a stripped environment.

Locally built binaries are never signed and are never published.

## Release approval

Nothing is published automatically. A release is the maintainer's decision: the
tag is created by hand, the workflow artifacts are inspected, and the Store
submission is a manual upload in Partner Center.

## Artifact metadata

Both Windows executables carry product name `EbbTop` and a file and product
version matching the release tag. The cross-build refuses to package an exe
whose PE version block is missing or wrong, and the manifest's
`Identity/Version` must equal the workspace version or the packaging script
stops.

## Unsigned artifacts, stated plainly

| Artifact | Signature | Integrity |
|---|---|---|
| `ebbtop.exe`, `ebbtop-probe.exe`, `EbbTop-Windows-x64.zip`, `install.ps1`, `setup.ps1` (GitHub Releases) | none | `SHA256SUMS` over HTTPS |
| `ebbtop-agent-linux-x86_64.tar.gz` (GitHub Releases) | none | release digest plus the archive's internal manifest |
| `EbbTop.app.zip` (macOS, GitHub Releases) | ad-hoc only | `SHA256SUMS` over HTTPS |
| `EbbTop-<version>-x64.msix` (Microsoft Store) | Microsoft, by Store re-signing at publish time | MSIX block map plus the Store signature |
| Any artifact not listed above | none | — |

## Verification

A Store-installed copy:

    Get-AppxPackage -Name Ebbwater.EbbTop | Select-Object -Expand SignatureKind

reads `Store` for a package installed from the Store.

A GitHub download (PowerShell, from the folder holding the release files):

    Get-FileHash ebbtop.exe -Algorithm SHA256   # compare with SHA256SUMS
    Get-AuthenticodeSignature .\ebbtop.exe      # NotSigned is expected

A package before upload:

    sha256sum dist/msix/EbbTop-<version>-x64.msix
    python3 deploy/windows/msix/verify-msix.py dist/msix/EbbTop-<version>-x64.msix \
        --expect-version <version>.0

## Reports

Suspected malicious behavior, or a signed artifact with altered bytes, can be
raised as a GitHub issue on the repository or through the Microsoft Store
report path.
