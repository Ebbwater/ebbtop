# EbbTop 0.3.0

Chinese interface, and the first build packaged for the Microsoft Store.

- Interface language. Settings > Display switches the whole desktop window
  between English, Simplified Chinese (Mainland) and Traditional Chinese
  (Taiwan). The choice is saved and lasts across launches; a value the build
  does not know falls back to English. Host data stays canonical: hostnames,
  process names, users, commands, paths and every number are painted exactly as
  the agent reports them, so nothing a support paste or a screenshot shows is
  translated. `docs/l10n-scope.md` lists every surface that is localized and
  every item that is deliberately not.
- Bundled Chinese fonts. Noto Sans SC and Noto Sans TC (SIL OFL 1.1) are
  compiled into the executable, so Chinese renders offline with no font
  install. The license is readable inside the app at Settings > Display >
  Fonts and license, and `LICENSE-OFL.txt` now ships beside `ebbtop.exe` in
  both the MSIX and the ZIP.
- Microsoft Store package. The MSIX is built by the `store-package` job with
  the in-app updater compiled out, because an MSIX is sealed and the Store
  delivers updates; the Store re-signs it with a Microsoft certificate. The
  `wack-gate` job runs Microsoft's Windows App Certification Kit against it on
  every release run and uploads the report. The GitHub ZIP keeps the updater
  and stays the way to install without the Store.
- Windows App Certification Kit harness. Three failures in the gate itself are
  fixed, none of which touched the shipped app: the SDK digest is computed
  under a forced `C` collation so the pinned makemsix build is reproducible,
  `signtool.exe` is resolved from the installed SDK instead of a fixed path,
  and the throwaway test certificate is selected by thumbprint rather than by
  subject name.

Accessibility, unchanged in this release but now stated where the Store asks
for it: `docs/store-listing.md` carries the accessibility statement and the
notes for certification, and `docs/accessibility-audit.md` records the gaps
that remain (screen-reader access to table rows and painted gauges, keyboard
scrolling at high zoom, live-region announcements, and the missing language
property on the accessibility tree).

Upgrading: Windows clients built before this release can update in-app from
the ZIP channel as usual; settings and stored credentials are preserved and the
previous executable is kept as a `.bak`. The Linux agent archive and
`ebbtop-install.sh` are rebuilt at 0.3.0; an existing agent host upgrades with
the same one-line installer and keeps its bind address, token and pairing
state.

macOS: the bundle remains 0.2.6 in this release. It is unchanged and still
built only on a Mac.

Windows downloads from GitHub are still unsigned. Integrity comes from
`SHA256SUMS` over HTTPS; compare the hash before running anything. The Store
channel is the signed one.
