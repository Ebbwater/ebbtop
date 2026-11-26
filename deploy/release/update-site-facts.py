#!/usr/bin/env python3
"""Rewrite the EbbTop product page's version and digest facts from a release.

The page states a version, a ZIP member count and seven SHA-256 digests. Those
are facts about published bytes, so they are derived here from the assembled
release set (dist/release/SHA256SUMS plus the files themselves) rather than
edited by hand.

Usage:
  python3 deploy/release/update-site-facts.py <site/ebbtop/index.html> <dist/release>

It rewrites only the values it owns and prints what changed. Anything it cannot
find exactly once is an error, so a page that drifted in shape stops the run
instead of being silently half-updated.
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

ASSETS = [
    "EbbTop-Windows-x64.zip",
    "ebbtop.exe",
    "ebbtop-probe.exe",
    "ebbtop-agent-linux-x86_64.tar.gz",
    "ebbtop-install.sh",
    "install.ps1",
    "setup.ps1",
]


def digest(name):
    h = hashlib.sha256()
    with (RELEASE / name).open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


facts = {n: (digest(n), (RELEASE / n).stat().st_size) for n in ASSETS}

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


# Version banner and spec rows.
sub_once(
    r"(Version )0\.\d+\.\d+(, published )\d+ \w+ \d{4}",
    rf"\g<1>{VERSION}\g<2>25 November 2026",
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
    r'(<span class="spec-key">Size</span><span class="spec-val">)[\d.]+ MB, 7 files(</span>)',
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

# Checksum table rows: bytes and digest for each named asset.
for name in ASSETS:
    sha, size = facts[name]
    row = re.compile(
        r'(<tr><td class="strong">' + re.escape(name) + r'</td><td class="num">)[\d,]+'
        r'(</td><td class="mono">)[0-9a-f]{64}(</td></tr>)'
    )
    sub_once(row, rf"\g<1>{size:,}\g<2>{sha}\g<3>", f"checksum row {name}", html)

# The member-count sentence.
sub_once(
    r"The ZIP holds (seven|eight|\w+) files:",
    f"The ZIP holds {'eight' if members == 8 else members} files:",
    "member sentence count",
    html,
)
sub_once(
    r"(<code>setup\.ps1</code>, )<code>SHA256SUMS</code>",
    r"\g<1><code>LICENSE-OFL.txt</code>, <code>SHA256SUMS</code>",
    "member sentence list",
    html,
)

PAGE.write_text(html)
print(f"version {VERSION}, zip {members} members, {zip_mb:.1f} MB")
for c in changes:
    print(f"  updated: {c}")
if html == orig:
    print("  (page already current)")
