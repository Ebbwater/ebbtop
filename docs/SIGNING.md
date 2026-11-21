# EbbTop Code Signing Policy

This document describes how EbbTop release artifacts are signed, how a signed
binary can be traced back to its source code, and what remains unsigned. It is
published to satisfy the code signing policy requirement of the
[SignPath Foundation](https://signpath.org) open source signing program.

## Signed artifacts

Windows release artifacts are signed with a code signing certificate issued to
**SignPath Foundation**:

- `ebbtop.exe` (desktop client)
- `ebbtop-probe.exe` (connection diagnostics tool)
- `EbbTop-Setup-x64.exe` (installer, when published)
- the Windows executables inside `EbbTop-Windows-x64.zip`

The certificate's private key is generated and held by SignPath on their
hardware security module. The EbbTop project never possesses private key
material and cannot sign artifacts outside the SignPath pipeline.

Signatures are timestamped (RFC 3161) so they remain valid after certificate
expiry.

## Build provenance

Every signed Windows binary is built by the project's public GitHub Actions
workflow from the tagged release commit in
[github.com/Ebbwater/ebbtop](https://github.com/Ebbwater/ebbtop):

- All workflow jobs that lead to a signing request run on GitHub-hosted
  runners (`ubuntu-latest` in
  [`.github/workflows/windows-build.yml`](../.github/workflows/windows-build.yml),
  triggered by `v*` tags and manual dispatch only).
- The workflow configuration and build scripts are part of the repository.
  They are reviewed like any other change.
- SignPath verifies that the submitted artifact was produced by that workflow
  for that commit before signing.
- A signature therefore means: this binary is an automated build of the
  public source at the stated commit. It does not identify the maintainer
  personally; the publisher shown on the certificate is SignPath Foundation.

Locally built binaries are never signed.

## Release approval

Every signing request requires manual approval by the project maintainer
before SignPath executes it. Unsigned builds can be produced at any time, but
nothing is signed and published as a release without that approval step.

## Artifact metadata

Signed binaries carry product name `EbbTop` and a file and product version
matching the release tag, enforced through SignPath's artifact metadata
restrictions.

## Unsigned artifacts, stated plainly

- `ebbtop-agent-linux-x86_64.tar.gz` (the prebuilt Linux agent) is built from
  the same tagged release commit but not Windows-code-signed. Integrity is
  provided by the release checksums (`SHA256SUMS` and the archive's internal
  manifest) over the HTTPS download.
- The macOS application bundle is currently unsigned. It will move to Apple
  Developer ID signing and notarization when that identity is procured.
- Any artifact not listed above is unsigned.

## Verification

Users can verify a signature on Windows with PowerShell:

    Get-AuthenticodeSignature .\ebbtop.exe

A valid signature shows `SignPath Foundation` as the signer and `Valid` as
the status.

## Team and account requirements

- The repository and all project accounts require multi-factor
  authentication.
- The maintainer holds the Author, Reviewer, and Approver roles defined by
  the SignPath Foundation conditions; signing approval is not delegated.
- Reports about a signed artifact (suspected malicious behavior, policy
  violations) can be raised as a GitHub issue or via the SignPath Foundation
  report path; the project team will assist in investigation.
