#!/usr/bin/env python3
"""
EbbTop brand pack builder — S1 "Ledger" mark.

Regenerates every SVG, PNG, ICO and RGBA asset from the geometry below.
Needs only Pillow (pip install pillow). Run from anywhere:

    python tools/build_ebbtop.py            # writes into the pack root

The mark: three forward-leaning slabs on the 96-unit grid, sorted longest
first like top's process list. Left ends ride one forward slash (the
Ebbwater lean, 33.6:52); right ends settle on the grid edge. Gaps between
rows equal the Ebbwater waterline height (8 units). Only the top row is teal.
"""
import json, os, struct, io
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# ---- tokens (Ebbwater brand kit v2) -------------------------------------
GRAPHITE, TEAL, TEAL_BRIGHT = "#23272B", "#2AA79B", "#3BC4B6"
FROST, SLATE, MIST = "#F5F7F8", "#6B7680", "#9AA6AD"

# ---- geometry ------------------------------------------------------------
LEAN = 33.6 / 52          # dx per dy, inherited from the Ebbwater driftmark
GAP = 8                   # waterline height, inherited
ROW = (96 - 2 * GAP) / 3  # 26.667 — also the clearspace unit

def master_rows():
    return [(i * (ROW + GAP), i * (ROW + GAP) + ROW) for i in range(3)]

def rows_px(h, g):
    """Integer pixel rows (h tall, g gap) expressed in 96-units."""
    m = 3 * h + 2 * g
    u = 96 / m
    return [(i * (h + g) * u, (i * (h + g) + h) * u) for i in range(3)]

def polygons(rows):
    """[(points, role)] in 96-units; role 'lit' = top row."""
    out = []
    for i, (yt, yb) in enumerate(rows):
        h = yb - yt
        pts = [(LEAN * yt, yt), (96 - LEAN * h, yt), (96, yb), (LEAN * yb, yb)]
        out.append((pts, "lit" if i == 0 else "ink"))
    return out

def hint(n, frac):
    """Pick integer row height h and gap g so the mark (3h+2g px) lands near
    frac*n with g/h near the master ratio 0.3. Every gap is >= 1 whole pixel."""
    target, best = frac * n, None
    for h in range(1, n + 1):
        for g in range(1, h + 1):
            m = 3 * h + 2 * g
            if m > n:
                continue
            s = abs(m - target) / n * 4 + abs(g / h - 0.3)
            if best is None or s < best[0]:
                best = (s, h, g, m)
    _, h, g, m = best
    return h, g, m

# ---- SVG -----------------------------------------------------------------
def fmt(v):
    return f"{round(v, 3):g}"

def pts_str(pts, s=1.0, ox=0.0, oy=0.0):
    return " ".join(f"{fmt(x * s + ox)},{fmt(y * s + oy)}" for x, y in pts)

def svg_mark(ink, lit, title):
    body = "".join(f'<polygon points="{pts_str(p)}" fill="{lit if r == "lit" else ink}"/>'
                   for p, r in polygons(master_rows()))
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96" role="img">'
            f'<title>{title}</title>{body}</svg>\n')

def tile_layout(n, frac=0.61, hinted=True):
    if hinted:
        h, g, m = hint(n, frac)
        rows = rows_px(h, g)
        oy = round((n - m) / 2)
    else:
        m = frac * n
        rows = master_rows()
        oy = (n - m) / 2
    ox = (n - m) / 2
    return polygons(rows), m / 96, ox, oy

def svg_tile(n, hinted=True, title="EbbTop app icon"):
    polys, s, ox, oy = tile_layout(n, hinted=hinted)
    body = "".join(f'<polygon points="{pts_str(p, s, ox, oy)}" fill="{TEAL_BRIGHT if r == "lit" else FROST}"/>'
                   for p, r in polys)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}" role="img">'
            f'<title>{title}</title><rect width="{n}" height="{n}" fill="{GRAPHITE}"/>{body}</svg>\n')

# ---- raster (Pillow, 16x supersampled, box-filtered) ---------------------
SS = 16

def hexrgb(c, a=255):
    return tuple(int(c[i:i + 2], 16) for i in (1, 3, 5)) + (a,)

def raster(n, polys, s, ox, oy, ink, lit, bg=None):
    big = Image.new("RGBA", (n * SS, n * SS), hexrgb(bg) if bg else (0, 0, 0, 0))
    # draw each colour on its own layer so AA edges composite correctly
    layers = []
    for colour, role in ((ink, "ink"), (lit, "lit")):
        mask = Image.new("L", (n * SS, n * SS), 0)
        d = ImageDraw.Draw(mask)
        for p, r in polys:
            if r == role:
                # Pillow fills edges inclusively; pull the bottom edge up one
                # supersample row so a 1 px gap stays exactly 1 px clear.
                ymax = max(y for _, y in p)
                d.polygon([((x * s + ox) * SS, (y * s + oy) * SS - (1 if y == ymax else 0))
                           for x, y in p], fill=255)
        layers.append((colour, mask))
    for colour, mask in layers:
        solid = Image.new("RGBA", big.size, hexrgb(colour))
        big = Image.composite(solid, big, mask) if bg else _over(big, solid, mask)
    return big.resize((n, n), Image.BOX)

def _over(base, solid, mask):
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    layer.paste(solid, (0, 0), mask)
    return Image.alpha_composite(base, layer)

def png_tile(n):
    polys, s, ox, oy = tile_layout(n, hinted=True)
    return raster(n, polys, s, ox, oy, FROST, TEAL_BRIGHT, bg=GRAPHITE)

def png_mark(n, ink, lit):
    if n <= 48:  # transparent mark: 1 px margin, hinted rows
        h, g, m = hint(n, (n - 2) / n)
        rows, oy = rows_px(h, g), round((n - m) / 2)
    else:
        m, rows, oy = n, master_rows(), 0
    return raster(n, polygons(rows), m / 96, (n - m) / 2, oy, ink, lit)

# ---- ICO (PNG-compressed entries, Vista+) --------------------------------
def write_ico(path, images):
    blobs = []
    for im in images:
        b = io.BytesIO(); im.save(b, "PNG"); blobs.append(b.getvalue())
    head = struct.pack("<HHH", 0, 1, len(images))
    off = 6 + 16 * len(images); entries = b""
    for im, blob in zip(images, blobs):
        w, h = im.size
        entries += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(blob), off)
        off += len(blob)
    with open(path, "wb") as f:
        f.write(head + entries + b"".join(blobs))

# ---- build ---------------------------------------------------------------
def main():
    d = lambda *p: os.makedirs(os.path.join(ROOT, *p), exist_ok=True) or os.path.join(ROOT, *p)
    svg, png, ico, rgba = d("svg"), d("png"), d("ico"), d("rgba")
    w = lambda p, s: open(p, "w", encoding="utf-8").write(s)

    w(f"{svg}/ebbtop-mark.svg", svg_mark(GRAPHITE, TEAL, "EbbTop"))
    w(f"{svg}/ebbtop-mark-dark.svg", svg_mark(FROST, TEAL_BRIGHT, "EbbTop"))
    w(f"{svg}/ebbtop-mark-graphite.svg", svg_mark(GRAPHITE, GRAPHITE, "EbbTop"))
    w(f"{svg}/ebbtop-mark-frost.svg", svg_mark(FROST, FROST, "EbbTop"))
    w(f"{svg}/ebbtop-app-icon.svg", svg_tile(512, hinted=False))
    for n in (16, 24, 32, 48, 64):
        w(f"{svg}/ebbtop-app-icon-{n}.svg", svg_tile(n))
    w(f"{svg}/ebbtop-favicon.svg", svg_tile(32, title="EbbTop"))

    ladder = (16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024)
    tiles = {n: png_tile(n) for n in ladder}
    for n, im in tiles.items():
        im.save(f"{png}/ebbtop-app-icon-{n}.png")
    for n in (16, 32, 64, 128, 256, 512):
        png_mark(n, GRAPHITE, TEAL).save(f"{png}/ebbtop-mark-{n}.png")
        png_mark(n, FROST, TEAL_BRIGHT).save(f"{png}/ebbtop-mark-dark-{n}.png")

    write_ico(f"{ico}/ebbtop.ico", [tiles[n] for n in (16, 20, 24, 32, 40, 48, 64, 256)])
    write_ico(f"{ico}/favicon.ico", [tiles[n] for n in (16, 32, 48)])

    # Drop-in for ebbtop-gui's window icon: 64x64, straight RGBA8, row-major.
    t64 = tiles[64].convert("RGBA")
    open(f"{rgba}/ebbtop-64.rgba", "wb").write(t64.tobytes())

    report = {n: dict(zip(("row_px", "gap_px", "mark_px"), hint(n, 0.61))) for n in ladder}
    json.dump(report, open(f"{ROOT}/tools/hinting.json", "w"), indent=1)
    print("built", ROOT)

if __name__ == "__main__":
    main()
