#!/usr/bin/env python3
"""Read the VERSIONINFO resource out of a Windows PE file (stdlib only).

SignPath enforces artifact metadata on every signed binary (product name and
product version). This script is the build-side readback: it parses the PE
resource directory itself -- no llvm-objdump, no wine, no python packages --
so the cross-build script and the hosted workflow can refuse to ship an exe
whose version block is missing or wrong.

  python3 deploy/windows/versioninfo.py dist/ebbtop.exe
  python3 deploy/windows/versioninfo.py --require-product-name EbbTop \\
      --require-version 0.2.9 --require-file-description "EbbTop ..." \\
      dist/ebbtop.exe dist/ebbtop-probe.exe

Exit status: 0 when every requested requirement holds for every file,
1 when a requirement fails or a file carries no VERSIONINFO,
2 on a usage or parse error.

The parser is deliberately literal about the on-disk format: DOS header ->
PE header -> optional header data directory 2 (resource) -> three-level
resource tree -> RT_VERSION (16) leaf -> VS_VERSIONINFO walk. Nothing is
read from the filename, the build log or Cargo.toml.
"""

from __future__ import annotations

import argparse
import hashlib
import struct
import sys

RT_VERSION = 16


class ParseError(Exception):
    pass


def _u16(buf: bytes, off: int) -> int:
    return struct.unpack_from("<H", buf, off)[0]


def _u32(buf: bytes, off: int) -> int:
    return struct.unpack_from("<I", buf, off)[0]


def _align4(off: int) -> int:
    return (off + 3) & ~3


def pe_layout(buf: bytes) -> tuple[list[tuple[int, int, int, int]], int]:
    """Return ([(vaddr, vsize, raw_ptr, raw_size)], resource_rva)."""
    if len(buf) < 0x40 or buf[:2] != b"MZ":
        raise ParseError("not a PE file: missing MZ header")
    pe_off = _u32(buf, 0x3C)
    if buf[pe_off : pe_off + 4] != b"PE\0\0":
        raise ParseError("not a PE file: missing PE signature")
    coff = pe_off + 4
    nsections = _u16(buf, coff + 2)
    opt_size = _u16(buf, coff + 16)
    opt = coff + 20
    magic = _u16(buf, opt)
    if magic == 0x10B:  # PE32
        nrva_off, dirs_off = opt + 92, opt + 96
    elif magic == 0x20B:  # PE32+
        nrva_off, dirs_off = opt + 108, opt + 112
    else:
        raise ParseError(f"unknown optional header magic 0x{magic:04x}")
    if _u32(buf, nrva_off) < 3:
        raise ParseError("PE has no resource data directory")
    resource_rva = _u32(buf, dirs_off + 2 * 8)
    if resource_rva == 0:
        raise ParseError("PE resource data directory is empty")
    sections = []
    sec = opt + opt_size
    for i in range(nsections):
        base = sec + i * 40
        vsize = _u32(buf, base + 8)
        vaddr = _u32(buf, base + 12)
        raw_size = _u32(buf, base + 16)
        raw_ptr = _u32(buf, base + 20)
        sections.append((vaddr, vsize, raw_ptr, raw_size))
    return sections, resource_rva


def rva_to_off(sections, rva: int) -> int:
    for vaddr, vsize, raw_ptr, raw_size in sections:
        if vaddr <= rva < vaddr + max(vsize, raw_size):
            delta = rva - vaddr
            if delta >= raw_size:
                raise ParseError(f"rva 0x{rva:x} falls in an uninitialised tail")
            return raw_ptr + delta
    raise ParseError(f"rva 0x{rva:x} is outside every section")


def _resource_leaf(buf: bytes, sections, res_base: int, dir_rva: int, depth: int,
                   want_type: int | None):
    """Walk one IMAGE_RESOURCE_DIRECTORY level and return (payload_rva, size)."""
    off = rva_to_off(sections, dir_rva)
    named = _u16(buf, off + 12)
    ids = _u16(buf, off + 14)
    for i in range(named + ids):
        entry = off + 16 + i * 8
        name_or_id = _u32(buf, entry)
        child = _u32(buf, entry + 4)
        if depth == 0 and (name_or_id & 0x80000000 or name_or_id != want_type):
            continue
        if child & 0x80000000:
            found = _resource_leaf(
                buf, sections, res_base, res_base + (child & 0x7FFFFFFF), depth + 1,
                want_type,
            )
            if found is not None:
                return found
            continue
        data_off = rva_to_off(sections, res_base + child)
        payload_rva = _u32(buf, data_off)
        payload_size = _u32(buf, data_off + 4)
        return payload_rva, payload_size
    return None


def version_blob(buf: bytes) -> bytes:
    sections, res_base = pe_layout(buf)
    found = _resource_leaf(buf, sections, res_base, res_base, 0, RT_VERSION)
    if found is None:
        raise ParseError("no RT_VERSION resource in the PE resource directory")
    payload_rva, payload_size = found
    start = rva_to_off(sections, payload_rva)
    return buf[start : start + payload_size]


def _read_wstr(buf: bytes, off: int, end: int) -> tuple[str, int]:
    start = off
    while off + 1 < end:
        if buf[off : off + 2] == b"\x00\x00":
            return buf[start:off].decode("utf-16-le", "replace"), off + 2
        off += 2
    raise ParseError(f"unterminated key string at 0x{off:x}")


class Node:
    __slots__ = ("key", "value", "children")

    def __init__(self, key: str, value: bytes, children: list["Node"]):
        self.key = key
        self.value = value
        self.children = children


def parse_version_node(buf: bytes, off: int, limit: int) -> tuple[Node, int]:
    if off + 6 > limit:
        raise ParseError(f"version node header at 0x{off:x} runs past the block")
    length = _u16(buf, off)
    value_len = _u16(buf, off + 2)
    node_end = off + length
    if length == 0 or node_end > limit:
        raise ParseError(f"version node at 0x{off:x} has bad length {length}")
    key, cursor = _read_wstr(buf, off + 6, node_end)
    cursor = _align4(cursor)
    value = b""
    if value_len:
        if key == "VS_VERSION_INFO":
            value = buf[cursor : cursor + 52]
        elif key == "Translation":
            value = buf[cursor : cursor + value_len]
        else:
            value = buf[cursor : cursor + value_len * 2]
        cursor = _align4(cursor + len(value))
    children: list[Node] = []
    while True:
        cursor = _align4(cursor)
        if cursor + 6 > node_end:
            break
        if _u16(buf, cursor) == 0:
            # Zero padding rounds the last child out to a 4-byte boundary.
            break
        child, cursor = parse_version_node(buf, cursor, node_end)
        if child.key:
            children.append(child)
    return Node(key, value, children), node_end


def collect(node: Node, out: dict[str, str]) -> None:
    for child in node.children:
        if child.children:
            collect(child, out)
        elif child.key != "Translation":
            out[child.key] = child.value.decode("utf-16-le", "replace").rstrip("\x00")


def fixed_version(root: Node) -> dict[str, str]:
    """VS_FIXEDFILEINFO is the value of the VS_VERSION_INFO node."""
    if len(root.value) < 52:
        raise ParseError("VS_VERSION_INFO carries no VS_FIXEDFILEINFO value")
    signature = _u32(root.value, 0)
    if signature != 0xFEEF04BD:
        raise ParseError(f"VS_FIXEDFILEINFO signature is 0x{signature:08x}")
    fms, fls, pms, pls = struct.unpack_from("<IIII", root.value, 8)
    return {
        "FileVersionNumber": f"{fms >> 16}.{fms & 0xFFFF}.{fls >> 16}.{fls & 0xFFFF}",
        "ProductVersionNumber": f"{pms >> 16}.{pms & 0xFFFF}.{pls >> 16}.{pls & 0xFFFF}",
    }


def read_file(path: str) -> tuple[dict[str, str], bytes]:
    with open(path, "rb") as fh:
        buf = fh.read()
    blob = version_blob(buf)
    root, _ = parse_version_node(blob, 0, len(blob))
    if root.key != "VS_VERSION_INFO":
        raise ParseError(f"first node is {root.key!r}, not VS_VERSION_INFO")
    fields: dict[str, str] = {}
    collect(root, fields)
    fields.update(fixed_version(root))
    fields["_blob_sha256"] = hashlib.new("sha256", blob).hexdigest()
    fields["_blob_size"] = str(len(blob))
    return fields, blob


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Read PE VERSIONINFO metadata.")
    ap.add_argument("files", nargs="+")
    ap.add_argument("--require-product-name", action="append", default=[])
    ap.add_argument("--require-version", action="append", default=[])
    ap.add_argument(
        "--require-file-description",
        action="append",
        default=[],
        help="acceptable FileDescription values (any one matches)",
    )
    ap.add_argument("--require-company", action="append", default=[])
    ap.add_argument("--dump", action="store_true", help="hex dump the VERSIONINFO block")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args(argv)

    results: dict[str, dict[str, str]] = {}
    for path in args.files:
        try:
            fields, blob = read_file(path)
        except (OSError, ParseError) as exc:
            print(f"{path}: ERROR {exc}", file=sys.stderr)
            return 1
        results[path] = fields
        if args.dump:
            print(f"{path}: VERSIONINFO block {len(blob)} bytes")
            for i in range(0, len(blob), 16):
                chunk = blob[i : i + 16]
                hexed = " ".join(f"{b:02x}" for b in chunk)
                text = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
                print(f"  {i:04x}  {hexed:<47}  {text}")

    if args.json:
        import json

        print(json.dumps(results, indent=2, sort_keys=True))
    else:
        for path, fields in results.items():
            print(f"{path}:")
            for key in (
                "ProductName",
                "ProductVersion",
                "FileVersion",
                "FileDescription",
                "OriginalFilename",
                "CompanyName",
                "ProductVersionNumber",
                "FileVersionNumber",
            ):
                print(f"  {key:<20} {fields.get(key, '<missing>')}")
            print(f"  {'VERSIONINFO sha256':<20} {fields['_blob_sha256']}")
            print(f"  {'VERSIONINFO bytes':<20} {fields['_blob_size']}")

    failures: list[str] = []
    for path, fields in results.items():
        for want in args.require_product_name:
            if fields.get("ProductName") != want:
                failures.append(f"{path}: ProductName is {fields.get('ProductName')!r}, required {want!r}")
        for want in args.require_version:
            for field in ("ProductVersion", "FileVersion"):
                if fields.get(field) != want:
                    failures.append(f"{path}: {field} is {fields.get(field)!r}, required {want!r}")
        for want in args.require_company:
            if fields.get("CompanyName") != want:
                failures.append(f"{path}: CompanyName is {fields.get('CompanyName')!r}, required {want!r}")
        if args.require_file_description:
            have = fields.get("FileDescription")
            if have not in args.require_file_description:
                failures.append(
                    f"{path}: FileDescription is {have!r}, required one of {args.require_file_description!r}"
                )
        expected_name = path.rsplit("/", 1)[-1]
        original = fields.get("OriginalFilename")
        if original != expected_name:
            failures.append(f"{path}: OriginalFilename is {original!r}, expected {expected_name!r}")

    if failures:
        for line in failures:
            print(f"FAIL {line}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
