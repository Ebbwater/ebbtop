#!/usr/bin/env python3
"""Round the corners of a PNG tile (macOS .app icon step).

Applies a superellipse ("squircle", exponent 4) alpha mask so the EbbTop
graphite tile reads as a rounded badge in the Dock. This runs at bundle
time (deploy/mac/package.sh) against copies of the brand-pack PNGs; the
vendored pack itself stays square per BRAND.md.

Stdlib only (zlib/struct) so it works with the Mac's system python3.
Supports 8-bit RGBA PNGs only; anything else is a hard error.
"""
import struct
import sys
import zlib

# Corner radius as a fraction of the tile edge. Larger than the system's own
# squircle mask so the rounding survives it and still reads badge-like.
RADIUS = 0.28
EXPONENT = 4


def read_png(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos, ihdr, idat = 8, None, b""
    while pos < len(data):
        (ln,) = struct.unpack(">I", data[pos : pos + 4])
        typ = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + ln]
        if typ == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", body)
        elif typ == b"IDAT":
            idat += body
        elif typ == b"IEND":
            break
        pos += 12 + ln
    if ihdr is None:
        raise ValueError(f"{path}: no IHDR")
    w, h, bitd, ctype, comp, filt, inter = ihdr
    if (bitd, ctype, inter) != (8, 6, 0):
        raise ValueError(f"{path}: need 8-bit non-interlaced RGBA, got {ihdr}")
    return w, h, zlib.decompress(idat)


def write_png(path, w, h, px):
    stride = w * 4
    raw = bytearray()
    for y in range(h):
        raw.append(0)  # filter 0 (None): simple, compresses fine on flat tiles
        raw += px[y * stride : (y + 1) * stride]
    def chunk(typ, body):
        return struct.pack(">I", len(body)) + typ + body + struct.pack(
            ">I", zlib.crc32(typ + body) & 0xFFFFFFFF
        )
    out = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw)))
        + chunk(b"IEND", b"")
    )
    open(path, "wb").write(out)


def rounded_alpha(w, h, px):
    """Multiply alpha by the superellipse mask, RGB left untouched."""
    out = bytearray(px)
    r = RADIUS * min(w, h)
    # Superellipse |x|^n + |y|^n <= r^n tested against each corner centre.
    for y in range(h):
        for x in range(w):
            cx = min(x, w - 1 - x)
            cy = min(y, h - 1 - y)
            if cx >= r or cy >= r:
                continue
            dx, dy = r - cx, r - cy
            if (dx / r) ** EXPONENT + (dy / r) ** EXPONENT > 1.0:
                out[(y * w + x) * 4 + 3] = 0
    return out


def unfilter(w, h, raw):
    stride = w * 4
    px = bytearray(w * h * 4)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        f = raw[p]
        p += 1
        line = bytearray(raw[p : p + stride])
        p += stride
        if f == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 255
        elif f == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 255
        elif f == 3:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 255
        elif f == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                b, c = prev[i], prev[i - 4] if i >= 4 else 0
                q = a + b - c
                pa, pb, pc = abs(q - a), abs(q - b), abs(q - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        elif f != 0:
            raise ValueError(f"unsupported PNG filter {f}")
        px[y * stride : (y + 1) * stride] = line
        prev = line
    return px


def main(paths):
    for path in paths:
        w, h, raw = read_png(path)
        write_png(path, w, h, rounded_alpha(w, h, unfilter(w, h, raw)))
        print(f"rounded {path} ({w}x{h})")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("usage: round_icon.py <png> [<png> ...]")
    main(sys.argv[1:])
