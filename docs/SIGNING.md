# EbbTop Windows distribution and signing

This document describes how the EbbTop Windows build reaches users, what is
signed and what is not, and how to check a download. It describes the route as
it stands and does not claim a signature that does not exist yet.

## Where the Windows build ships

EbbTop for Windows is moving to the Microsoft Store. The listing is created in
Partner Center, which holds the app's publisher identity, and that identity is
what a package declares.

A Store install is signed by Microsoft during submission when the app ships as
an **MSIX** package. An MSI or EXE submitted to the Store does not receive Store
signing and would need a publisher code signing certificate of its own, so the
package is prepared as MSIX. Until the listing is live, the direct download is
how the app is installed.

## What is signed today

Nothing. The current Windows build is a per-user ZIP
(`EbbTop-Windows-x64.zip`) holding unsigned executables, so Windows may show a
SmartScreen warning on first run. When the Store listing ships, installs from
the Store carry a Microsoft issued signature, and this document names the
submission that first carries it.

## Build provenance

Every Windows artifact is built by the public workflow in
[`.github/workflows/windows-build.yml`](../.github/workflows/windows-build.yml)
from the tagged release commit in
[github.com/Ebbwater/ebbtop](https://github.com/Ebbwater/ebbtop). The workflow
runs on GitHub hosted runners, uses a pinned cross compiler, and is triggered by
`v*` tags or manual dispatch only. The packaged executables carry a
`VERSIONINFO` block with product name `EbbTop` and the release version, which is
also what Store packaging expects. Locally built binaries are never published as
a release.

## What stays unsigned

- `ebbtop-agent-linux-x86_64.tar.gz`, the prebuilt Linux agent, is built from
  the same tagged commit and is not code signed on any platform. Integrity comes
  from the digest the release reports, which `ebbtop-install.sh` checks before
  running anything, plus the archive's own `SHA256SUMS`.
- The macOS bundle is ad-hoc signed only. It moves to Apple Developer ID
  signing and notarization when that identity exists.
- Any artifact not named above is unsigned.

## Checking a download

The release publishes `SHA256SUMS` beside the artifacts, and the Windows ZIP
carries a copy inside it:

    sha256sum -c SHA256SUMS                        # Linux
    Get-FileHash .\ebbtop.exe -Algorithm SHA256    # Windows PowerShell

A checksum confirms the bytes you received match the bytes that were published.
It does not prove who published them, which is what a signature does. Until a
signed package exists, the HTTPS download from the project's release page is the
trust anchor.

## Team and account requirements

- The repository and all project accounts require multi-factor authentication.
- The maintainer holds the Author, Reviewer, and Approver roles used by the
  store submission and release process; none of them is delegated. The people in
  those roles are public on
  [the organization people page](https://github.com/orgs/Ebbwater/people), and
  the repository grants write access through the Ebbwater Members team.
- Reports about a release artifact, suspected malicious behavior, or a policy
  violation can be raised as a GitHub issue; the project team will assist in
  investigation.
