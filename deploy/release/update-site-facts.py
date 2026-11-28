#!/usr/bin/env python3
"""Rewrite the EbbTop product page's version and digest facts from a release.

The page states a version, a ZIP member count and SHA-256 digests. Those
are facts about published bytes, so they are derived here from the assembled
release set (dist/release/SHA256SUMS plus the files themselves) rather than
edited by hand.

Usage:
  python3 deploy/release/update-site-facts.py <site/ebbtop/index.html> <dist/release> [published date]

The published date (e.g. "27 November 2026") is optional: given, the version
banner is rewritten with it; omitted, only the version is touched and the
existing date stands (a date is a fact about the already-published page).

It rewrites only the values it owns and prints what changed. Anything it cannot
find exactly once is an error, so a page that drifted in shape stops the run
instead of being silently half-updated. Exception: an asset listed in
OPTIONAL_ASSETS whose checksum row the page does not carry yet is reported and
skipped — the row arrives with the next page edit, and a new asset must not
block updating the rows the page already has.
"""
import hashlib
import pathlib
import re
import sys

PAGE = pathlib.Path(sys.argv[1])
RELEASE = pathlib.Path(sys.argv[2])

version = re.search(r"^version (\S+)$", (RELEASE / "BUILD").read_text(), re.M)
if not version:
    sys.exit(f"no version line in {RELEASE/'BUILD'}")
VERSION = version.group(1)

REQUIRED_ASSETS = [
    "EbbTop-Windows-x64.zip",
    "ebbtop.exe",
    "ebbtop-probe.exe",
    "ebbtop-agent-linux-x86_64.tar.gz",
    "ebbtop-install.sh",
    "install.ps1",
    "setup.ps1",
]
# Shipped since 0.3.1 (Apple Silicon agent). A release set always carries
# the file; the site's checksum table gains its row at the next page edit.
OPTIONAL_ASSETS = ["ebbtop-agent-macos-arm64.tar.gz"]


def digest(name):
    h = hashlib.sha256()
    with (RELEASE / name).open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


facts = {n: (digest(n), (RELEASE / n).stat().st_size) for n in REQUIRED_ASSETS}
# Optional assets may be absent from a pre-0.3.1 release set; an absent file
# simply skips the row step (the page cannot reference bytes that no release
# set in play carries).
for name in OPTIONAL_ASSETS:
    if (RELEASE / name).exists():
        facts[name] = (digest(name), (RELEASE / name).stat().st_size)

# The ZIP member count is read from the archive, not assumed.
import zipfile

with zipfile.ZipFile(RELEASE / "EbbTop-Windows-x64.zip") as z:
    members = len([i for i in z.infolist() if not i.is_dir()])

html = PAGE.read_text()
orig = html
changes = []


def sub_once(pattern, replacement, what, string):
    global html
    new, n = re.subn(pattern, replacement, string)
    if n != 1:
        sys.exit(f"{what}: expected exactly 1 match, found {n}")
    if new != string:
        changes.append(what)
    html = new


# Version banner (and, when given, the publish date). The date is a fact the
# caller owns — it is when the release was actually published, which this
# script cannot know from the assembled bytes.
PUBLISHED = sys.argv[3] if len(sys.argv) > 3 else None
if PUBLISHED:
    sub_once(
        r"(Version )0\.\d+\.\d+(, published )\d+ \w+ \d{4}",
        rf"\g<1>{VERSION}\g<2>{PUBLISHED}",
        "version banner",
        html,
    )
else:
    sub_once(
        r"(Version )0\.\d+\.\d+(, published )\d+ \w+ \d{4}",
        rf"\g<1>{VERSION}\g<2>",
        "version banner",
        html,
    )
sub_once(
    r'(<span class="spec-key">Version</span><span class="spec-val">)0\.\d+\.\d+(, x64</span>)',
    rf"\g<1>{VERSION}\g<2>",
    "Windows spec version",
    html,
)
zip_mb = facts["EbbTop-Windows-x64.zip"][1] / 1_000_000
sub_once(
    r'(<span class="spec-key">Size</span><span class="spec-val">)[\d.]+ MB, \d+ files(</span>)',
    rf"\g<1>{zip_mb:.1f} MB, {members} files\g<2>",
    "Windows spec size",
    html,
)
agent_mb = facts["ebbtop-agent-linux-x86_64.tar.gz"][1] / 1_000_000
sub_once(
    r'(<span class="spec-key">Size</span><span class="spec-val">)[\d.]+ MB archive(</span>)',
    rf"\g<1>{agent_mb:.1f} MB archive\g<2>",
    "agent spec size",
    html,
)

# Checksum table rows: bytes and digest for each named asset. Required rows
# stop the run on drift; an optional asset without its row yet is a warning.
for name in REQUIRED_ASSETS:
    sha, size = facts[name]
    row = re.compile(
        r'(<tr><td class="strong">' + re.escape(name) + r'</td><td class="num">)[\d,]+'
        r'(</td><td class="mono">)[0-9a-f]{64}(</td></tr>)'
    )
    sub_once(row, rf"\g<1>{size:,}\g<2>{sha}\g<3>", f"checksum row {name}", html)
for name in OPTIONAL_ASSETS:
    if name not in facts:
        print(f"  note: {name} is not in this release set; skipping its checksum row")
        continue
    sha, size = facts[name]
    row = re.compile(
        r'(<tr><td class="strong">' + re.escape(name) + r'</td><td class="num">)[\d,]+'
        r'(</td><td class="mono">)[0-9a-f]{64}(</td></tr>)'
    )
    new, n = row.subn(rf"\g<1>{size:,}\g<2>{sha}\g<3>", html)
    if n == 1:
        if new != html:
            changes.append(f"checksum row {name}")
        html = new
    elif n == 0:
        print(
            f"  note: {name} ships in this release but the page has no checksum row for it yet"
        )
    else:
        sys.exit(f"checksum row {name}: expected 0 or 1 match, found {n}")

# The member-count sentence.
sub_once(
    r"The ZIP holds (seven|eight|\w+) files:",
    f"The ZIP holds {'eight' if members == 8 else members} files:",
    "member sentence count",
    html,
)

PAGE.write_text(html)
print(f"version {VERSION}, zip {members} members, {zip_mb:.1f} MB")
for c in changes:
    print(f"  updated: {c}")
if html == orig:
    print("  (page already current)")
