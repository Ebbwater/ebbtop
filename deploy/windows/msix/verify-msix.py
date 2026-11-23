#!/usr/bin/env python3
"""Verify a packed EbbTop .msix the way the Store and Windows read it.

Checks, all from the package bytes (no Windows tooling):

  * the OPC parts exist: AppxManifest.xml, AppxBlockMap.xml,
    [Content_Types].xml;
  * the manifest parses, is x64, runs ebbtop.exe as a full-trust desktop
    app, and its Identity/Version is the expected four-part version;
  * every file the manifest points at (logos, splash, executable) is in the
    package;
  * every block-map hash matches the uncompressed file bytes, and every
    packaged file except the block map itself has a block-map entry (an
    unpacked or altered byte would install-fail on Windows);
  * the package is not signed (the Store re-signs it);
  * the package identity is exactly the Partner Center identity;
  * the declared languages are the three the desktop ships, as valid BCP-47
    tags, and nothing else;
  * `LICENSE-OFL.txt` ships beside `ebbtop.exe` and its bytes are exactly the
    checked-in license for the embedded Noto Sans CJK faces.

Exit 0 when every check holds, 1 on the first failure with the reason.

  python3 deploy/windows/msix/verify-msix.py dist/msix/EbbTop-0.3.0-x64.msix
  python3 deploy/windows/msix/verify-msix.py dist/msix/EbbTop-0.3.0-x64.msix \
      --expect-version 0.3.0.0

Without --expect-version the expected value is the workspace `Cargo.toml`
version plus a fourth `.0` component, which is the form the manifest carries.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import pathlib
import posixpath
import re
import sys
import zipfile
import xml.etree.ElementTree as ET

NS = {
    "f": "http://schemas.microsoft.com/appx/manifest/foundation/windows10",
    "uap": "http://schemas.microsoft.com/appx/manifest/uap/windows10",
    "bm": "http://schemas.microsoft.com/appx/2010/blockmap",
}
BLOCK = 64 * 1024  # MSIX block size

REPO = pathlib.Path(__file__).resolve().parents[3]
CARGO_TOML = REPO / "Cargo.toml"
# The OFL notice that must accompany the embedded Noto Sans CJK faces. It is
# the checked-in file, byte for byte, staged beside ebbtop.exe by
# package-msix.sh.
LICENSE_FILE = REPO / "crates/ebbtop-gui/assets/fonts/LICENSE-OFL.txt"
LICENSE_MEMBER = "LICENSE-OFL.txt"

# Partner Center issued these when the app name was reserved. They are public
# distribution identities, not secrets, and must match byte for byte or the
# upload is rejected.
IDENTITY_NAME = "Ebbwater.EbbTop"
IDENTITY_PUBLISHER = "CN=32700780-5AAE-4D15-A591-B49543B86C16"
PUBLISHER_DISPLAY_NAME = "Ebbwater"

# The desktop client ships English, Simplified Chinese (Mainland) and
# Traditional Chinese (Taiwan). The manifest declares them as BCP-47 tags; the
# in-app setting stores the Chinese ones script-tagged (zh-Hans-CN,
# zh-Hant-TW). A declared language without matching content is as wrong as a
# missing one, so the set must match exactly.
SUPPORTED_LANGUAGES = ("en-US", "zh-CN", "zh-TW")
BCP47 = re.compile(r"^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$")


class Fail(Exception):
    pass


def check(cond: bool, msg: str) -> None:
    if not cond:
        raise Fail(msg)


def workspace_version() -> str:
    """The workspace release version from Cargo.toml `[workspace.package]`."""
    in_section = False
    for line in CARGO_TOML.read_text(encoding="utf-8").splitlines():
        if line.strip() == "[workspace.package]":
            in_section = True
            continue
        if in_section:
            if line.startswith("["):
                break
            if line.startswith("version = "):
                return line.split('"')[1]
    raise KeyError(f"no [workspace.package] version in {CARGO_TOML}")


def manifest_languages(manifest: ET.Element) -> list[str]:
    resources = manifest.find("f:Resources", NS)
    check(resources is not None, "manifest has no Resources (no declared language)")
    return [r.get("Language", "") for r in resources.findall("f:Resource", NS)]


def manifest_assets(manifest: ET.Element) -> list[str]:
    """Every package-relative path the manifest may reference."""
    paths: list[str] = []

    def add(value: str | None) -> None:
        if value:
            paths.append(value.replace("\\", "/"))

    props = manifest.find("f:Properties", NS)
    check(props is not None, "manifest has no Properties")
    add(props.findtext("f:Logo", None, NS))
    app = manifest.find("f:Applications/f:Application", NS)
    check(app is not None, "manifest has no Application")
    add(app.get("Executable"))
    ve = app.find("uap:VisualElements", NS)
    check(ve is not None, "manifest has no uap:VisualElements")
    for attr in ("Square150x150Logo", "Square44x44Logo", "Square310x310Logo", "Square71x71Logo"):
        add(ve.get(attr))
    tile = ve.find("uap:DefaultTile", NS)
    if tile is not None:
        for attr in ("Wide310x150Logo", "Square310x310Logo", "Square71x71Logo"):
            add(tile.get(attr))
    splash = ve.find("uap:SplashScreen", NS)
    if splash is not None:
        add(splash.get("Image"))
    return [p for p in paths if p]


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("msix")
    ap.add_argument(
        "--expect-version",
        help="four-part Identity Version (default: the Cargo.toml version + .0)",
    )
    args = ap.parse_args(argv[1:])

    if args.expect_version:
        expect_version = args.expect_version
    else:
        try:
            expect_version = workspace_version() + ".0"
        except (OSError, KeyError, IndexError) as e:
            print(f"FAIL {args.msix}: cannot resolve the expected version: {e}", file=sys.stderr)
            return 1

    try:
        z = zipfile.ZipFile(args.msix)
        names = set(z.namelist())

        # --- OPC parts ---------------------------------------------------
        for part in ("AppxManifest.xml", "AppxBlockMap.xml", "[Content_Types].xml"):
            check(part in names, f"missing OPC part {part!r}")
        check(
            not any(n.lower().endswith("appxsignature.p7x") for n in names),
            "package is signed; the Store package must be submitted unsigned",
        )

        # --- manifest ----------------------------------------------------
        manifest = ET.fromstring(z.read("AppxManifest.xml"))
        ident = manifest.find("f:Identity", NS)
        check(ident is not None, "manifest has no Identity")
        check(
            ident.get("ProcessorArchitecture") == "x64",
            f"architecture is {ident.get('ProcessorArchitecture')!r}, expected x64",
        )
        check(
            ident.get("Version") == expect_version,
            f"Identity/Version is {ident.get('Version')!r}, expected {expect_version!r}",
        )
        check(
            ident.get("Name") == IDENTITY_NAME,
            f"Identity/Name is {ident.get('Name')!r}, expected {IDENTITY_NAME!r}",
        )
        check(
            ident.get("Publisher") == IDENTITY_PUBLISHER,
            f"Identity/Publisher is {ident.get('Publisher')!r}, expected {IDENTITY_PUBLISHER!r}",
        )
        check(
            manifest.findtext("f:Properties/f:PublisherDisplayName", "", NS)
            == PUBLISHER_DISPLAY_NAME,
            f"Properties/PublisherDisplayName must be {PUBLISHER_DISPLAY_NAME!r}",
        )
        app = manifest.find("f:Applications/f:Application", NS)
        check(app is not None, "manifest has no Application")
        check(
            app.get("Executable") == "ebbtop.exe",
            f"Application/@Executable is {app.get('Executable')!r}, expected ebbtop.exe",
        )
        check(
            app.get("EntryPoint") == "Windows.FullTrustApplication",
            "Application/@EntryPoint must be Windows.FullTrustApplication (packaged desktop app)",
        )
        check(
            manifest.findtext("f:Properties/f:DisplayName", "", NS) == "EbbTop",
            "Properties/DisplayName must be EbbTop",
        )

        # --- referenced files --------------------------------------------
        for asset in manifest_assets(manifest):
            check(asset in names, f"manifest references {asset!r}, which is not in the package")
        check("ebbtop.exe" in names, "ebbtop.exe is not in the package")

        # --- declared languages -------------------------------------------
        declared = manifest_languages(manifest)
        check(declared, "manifest declares no Resource Language")
        for lang in declared:
            check(
                bool(BCP47.match(lang)),
                f"declared language {lang!r} is not a well-formed BCP-47 tag",
            )
        check(
            {lang.lower() for lang in declared}
            == {lang.lower() for lang in SUPPORTED_LANGUAGES},
            f"declared languages {declared!r} are not exactly {list(SUPPORTED_LANGUAGES)!r}",
        )

        # --- the OFL notice beside the executable --------------------------
        check(
            LICENSE_MEMBER in names,
            f"{LICENSE_MEMBER} is not in the package (the embedded fonts' license "
            "must ship beside ebbtop.exe)",
        )
        try:
            expected_license = LICENSE_FILE.read_bytes()
        except OSError as e:
            print(f"FAIL {args.msix}: cannot read {LICENSE_FILE}: {e}", file=sys.stderr)
            return 1
        check(bool(expected_license), f"{LICENSE_FILE} is empty")
        check(
            b"SIL OPEN FONT LICENSE" in expected_license,
            f"{LICENSE_FILE} does not look like the SIL OFL text",
        )
        packaged_license = z.read(LICENSE_MEMBER)
        check(
            packaged_license == expected_license,
            f"{LICENSE_MEMBER} in the package is not byte-identical to {LICENSE_FILE}",
        )

        # --- block map ---------------------------------------------------
        blockmap = ET.fromstring(z.read("AppxBlockMap.xml"))
        blocks = blockmap.findall("bm:File", NS)
        check(blocks, "block map lists no files")
        mapped: set[str] = set()
        for entry in blocks:
            name = entry.get("Name", "")
            # makemsix writes block-map File@Name with backslash separators;
            # OPC part names in the package use forward slashes. Compare on
            # the normalized form.
            norm = name.replace("\\", "/")
            mapped.add(norm)
            check(norm in names, f"block map references {name!r}, which is not in the package")
            raw = z.read(norm)
            check(
                int(entry.get("Size", "-1")) == len(raw),
                f"block map size for {name!r} does not match the packaged file",
            )
            hashes = entry.findall("bm:Block", NS)
            check(hashes, f"block map has no blocks for {name!r}")
            for i, block in enumerate(hashes):
                chunk = raw[i * BLOCK : (i + 1) * BLOCK]
                raw_digest = hashlib.sha256(chunk).digest()
                encoded = base64.b64encode(raw_digest).decode("ascii")
                check(
                    block.get("Hash") == encoded,
                    f"block {i} of {name!r} does not match its block-map hash",
                )
                # Block@Size is the COMPRESSED size of the block as stored in
                # the package (observed on Microsoft's own makemsix output:
                # a full 65536-byte block carries Size < 65536; an
                # incompressible block can carry slightly more than its raw
                # size from deflate framing). The per-block SHA-256 hash is
                # the integrity invariant; Size only has to be a plausible
                # stored size for the block.
                if block.get("Size") is not None:
                    size = block.get("Size")
                    check(
                        size is not None and 0 < int(size) < len(chunk) + 1024,
                        f"block {i} of {name!r} has an implausible size in the block map",
                    )
        for name in names:
            if name in ("AppxBlockMap.xml", "[Content_Types].xml"):
                continue
            check(name in mapped, f"packaged file {name!r} has no block-map entry")

        print(f"OK {posixpath.basename(args.msix)}: {len(names)} parts, "
              f"{len(blocks)} block-map files, version {expect_version}, "
              f"languages {','.join(declared)}, "
              f"{LICENSE_MEMBER} {len(packaged_license)} bytes == checked-in, unsigned")
        return 0
    except Fail as e:
        print(f"FAIL {args.msix}: {e}", file=sys.stderr)
        return 1
    except (zipfile.BadZipFile, ET.ParseError, KeyError) as e:
        print(f"FAIL {args.msix}: {type(e).__name__}: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
