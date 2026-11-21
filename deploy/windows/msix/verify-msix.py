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
  * the package is not signed (the Store re-signs it).

Exit 0 when every check holds, 1 on the first failure with the reason.

  python3 deploy/windows/msix/verify-msix.py dist/msix/EbbTop-0.2.9-x64.msix \
      --expect-version 0.2.9.0
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import posixpath
import sys
import zipfile
import xml.etree.ElementTree as ET

NS = {
    "f": "http://schemas.microsoft.com/appx/manifest/foundation/windows10",
    "uap": "http://schemas.microsoft.com/appx/manifest/uap/windows10",
    "bm": "http://schemas.microsoft.com/appx/2010/blockmap",
}
BLOCK = 64 * 1024  # MSIX block size


class Fail(Exception):
    pass


def check(cond: bool, msg: str) -> None:
    if not cond:
        raise Fail(msg)


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
    ap.add_argument("--expect-version", required=True, help="four-part Identity Version")
    args = ap.parse_args(argv[1:])

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
            ident.get("Version") == args.expect_version,
            f"Identity/Version is {ident.get('Version')!r}, expected {args.expect_version!r}",
        )
        check(bool(ident.get("Name")), "Identity/Name is empty")
        check(bool(ident.get("Publisher")), "Identity/Publisher is empty")
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

        # --- block map ---------------------------------------------------
        blockmap = ET.fromstring(z.read("AppxBlockMap.xml"))
        blocks = blockmap.findall("bm:File", NS)
        check(blocks, "block map lists no files")
        mapped: set[str] = set()
        for entry in blocks:
            name = entry.get("Name", "")
            mapped.add(name)
            check(name in names, f"block map references {name!r}, which is not in the package")
            raw = z.read(name)
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
                # a full 65536-byte block carries Size < 65536). The hash
                # above is the integrity invariant; Size only has to be a
                # plausible stored size for the block.
                if block.get("Size") is not None:
                    check(
                        0 < int(block.get("Size")) <= len(chunk),
                        f"block {i} of {name!r} has an implausible size in the block map",
                    )
        for name in names:
            if name in ("AppxBlockMap.xml", "[Content_Types].xml"):
                continue
            check(name in mapped, f"packaged file {name!r} has no block-map entry")

        print(f"OK {posixpath.basename(args.msix)}: {len(names)} parts, "
              f"{len(blocks)} block-map files, version {args.expect_version}, unsigned")
        return 0
    except Fail as e:
        print(f"FAIL {args.msix}: {e}", file=sys.stderr)
        return 1
    except (zipfile.BadZipFile, ET.ParseError, KeyError) as e:
        print(f"FAIL {args.msix}: {type(e).__name__}: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
