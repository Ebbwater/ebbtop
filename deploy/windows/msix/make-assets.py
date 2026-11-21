#!/usr/bin/env python3
"""Generate the Microsoft Store MSIX image assets from the EbbTop brand pack.

Every image is derived from the repository's own icon source -- no new
artwork. The tile is the brand pack's app icon (graphite ground, "Ledger"
mark, teal lit row), rasterised by the brand builder itself
(`crates/ebbtop-gui/assets/brand/tools/build_ebbtop.py`), so an MSIX asset is
pixel-identical to the matching shipped PNG at that size. The unplated app
list variants use the brand pack's mark on transparency (frost/teal for the
dark theme, graphite/teal for the light theme), which is exactly what the
brand pack publishes as `ebbtop-mark-dark-*` and `ebbtop-mark-*`.

Sizes come from Microsoft's "Construct your Windows app's icon" tables
(learn.microsoft.com/windows/apps/design/iconography/app-icon-construction):

  StoreLogo          required  50, 63, 75, 100, 200   (scale 100/125/150/200/400)
  Square44x44Logo    required  44, 55, 66, 88, 176    (scale 100/125/150/200/400)
                     required  targetsize 16, 20, 24, 30, 32, 36, 40, 48,
                               60, 64, 72, 80, 96, 256, each in the default,
                               _altform-unplated and _altform-lightunplated forms
  Square150x150Logo  required  150, 188, 225, 300, 600 (the medium tile; the
                               Store refuses a package without the 100% one)
  Wide310x150Logo    required  310x150, 388x188, 465x225, 620x300, 1240x600
  SplashScreen       required  620x300, 775x375, 930x450, 1240x600, 2480x1200

Usage:  python3 deploy/windows/msix/make-assets.py <output-dir>
Needs Pillow (the brand builder's only dependency); the packaging script
installs it on the hosted runner.
"""

from __future__ import annotations

import importlib.util
import io
import os
import sys

from PIL import Image

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
BRAND_TOOLS = os.path.join(REPO, "crates", "ebbtop-gui", "assets", "brand", "tools")

# Qualifier -> multiplier, from the manifest table (base size x scale / 100).
SCALES = {"100": 1.0, "125": 1.25, "150": 1.5, "200": 2.0, "400": 4.0}

# App list target sizes (exact pixels, not scale factors).
TARGETSIZES = (16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256)


def load_brand():
    spec = importlib.util.spec_from_file_location(
        "ebbtop_brand", os.path.join(BRAND_TOOLS, "build_ebbtop.py")
    )
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load the brand builder from {BRAND_TOOLS}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def comper_wide(brand, w: int, h: int) -> Image.Image:
    """Wide tile / splash: the mark centred on the brand graphite ground.

    Not new artwork: the mark is the brand pack's own geometry drawn by the
    brand builder, and the ground is the brand tile colour. The mark occupies
    the same 0.61 share of the shorter edge that the square tile uses.
    """
    canvas = Image.new("RGBA", (w, h), brand.hexrgb(brand.GRAPHITE))
    side = int(round(min(w, h) * 0.61))
    mark = brand.png_mark(side, brand.FROST, brand.TEAL_BRIGHT)
    canvas.alpha_composite(mark, ((w - side) // 2, (h - side) // 2))
    return canvas


def write(im: Image.Image, path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    # Deterministic encoding: same pixels -> same bytes, so a rebuild is a
    # no-op diff and a digest comparison means something.
    buf = io.BytesIO()
    im.save(buf, "PNG", optimize=True)
    with open(path, "wb") as f:
        f.write(buf.getvalue())


def px(base: float, qualifier: str) -> int:
    return int(round(base * SCALES[qualifier]))


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__.strip().splitlines()[-2].strip(), file=sys.stderr)
        return 2
    out = os.path.abspath(argv[1])
    assets = os.path.join(out, "Assets")
    os.makedirs(assets, exist_ok=True)
    b = load_brand()

    written: list[tuple[str, int, int]] = []

    def put(name: str, im: Image.Image) -> None:
        path = os.path.join(assets, name)
        write(im, path)
        written.append((name, im.width, im.height))

    # Package logo (Microsoft Store logo) -- plated brand tile.
    for q in SCALES:
        put(f"StoreLogo.scale-{q}.png", b.png_tile(px(50, q)))

    # App list icon: plated tile at the scale factors ...
    for q in SCALES:
        put(f"Square44x44Logo.scale-{q}.png", b.png_tile(px(44, q)))
    # ... plus the required target-size triples (default, dark unplated,
    # light unplated) Windows uses for the taskbar, Start list and search.
    for n in TARGETSIZES:
        put(f"Square44x44Logo.targetsize-{n}.png", b.png_tile(n))
        put(
            f"Square44x44Logo.targetsize-{n}_altform-unplated.png",
            b.png_mark(n, b.FROST, b.TEAL_BRIGHT),
        )
        put(
            f"Square44x44Logo.targetsize-{n}_altform-lightunplated.png",
            b.png_mark(n, b.GRAPHITE, b.TEAL),
        )

    # Medium tile (required by the Store) and the app tile scale set.
    for q in SCALES:
        put(f"Square150x150Logo.scale-{q}.png", b.png_tile(px(150, q)))

    # Wide tile and splash screen: mark on the brand ground.
    for q, mult in SCALES.items():
        put(
            f"Wide310x150Logo.scale-{q}.png",
            comper_wide(b, int(round(310 * mult)), int(round(150 * mult))),
        )
        put(
            f"SplashScreen.scale-{q}.png",
            comper_wide(b, int(round(620 * mult)), int(round(300 * mult))),
        )

    # Small and large tiles are optional; the Store only needs the medium
    # tile, and these keep the Windows 10 Start menu from scaling the medium
    # tile up. Same source, same ground.
    for q in SCALES:
        put(f"SmallTile.scale-{q}.png", b.png_tile(px(71, q)))
        put(f"LargeTile.scale-{q}.png", b.png_tile(px(310, q)))

    # Base-name copies of the five logos the manifest references
    # (Assets\StoreLogo.png etc.). Windows resolves the qualified scale
    # variants through the naming convention, but the literal files ride
    # along too, so a direct manifest-asset check (verify-msix.py) and any
    # tooling that looks the referenced path up verbatim both find them.
    # Same pixels as the scale-100 variants.
    put("StoreLogo.png", b.png_tile(50))
    put("Square44x44Logo.png", b.png_tile(44))
    put("Square150x150Logo.png", b.png_tile(150))
    put("Wide310x150Logo.png", comper_wide(b, 310, 150))
    put("SplashScreen.png", comper_wide(b, 620, 300))

    for name, w, h in written:
        print(f"{w}x{h}\t{name}")
    print(f"{len(written)} assets written under {assets}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
