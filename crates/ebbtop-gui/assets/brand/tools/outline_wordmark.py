#!/usr/bin/env python3
"""
Outline the EBBTOP wordmark and lockups from Exo 2 Medium (no <text>, no
font dependency in the output).

    pip install fonttools
    python tools/outline_wordmark.py path/to/Exo2[wght].ttf
    python tools/outline_wordmark.py path/to/Exo2-Medium.ttf

Accepts the Google Fonts variable file (instanced to wght 500 here) or a
static Medium. Writes into svg/:

    ebbtop-wordmark.svg        EBB graphite + TOP teal
    ebbtop-wordmark-dark.svg   EBB frost    + TOP teal bright
    ebbtop-lockup.svg          mark + wordmark, light
    ebbtop-lockup-dark.svg     mark + wordmark, dark

Spec: all caps, tracking -0.04em, font kerning applied, cap height = mark
height, gap between mark and word = one row (ROW units), flush left.
"""
import os, sys
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.boundsPen import BoundsPen

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from build_ebbtop import (ROOT, ROW, GRAPHITE, TEAL, TEAL_BRIGHT, FROST,
                          polygons, master_rows, pts_str)

WORD, SPLIT = "EBBTOP", 3        # EBB | TOP
TRACKING = -0.04                 # em
WEIGHT = 500                     # Exo 2 Medium


def load(path):
    font = TTFont(path)
    if "fvar" in font:
        from fontTools.varLib.instancer import instantiateVariableFont
        font = instantiateVariableFont(font, {"wght": WEIGHT})
    return font


def gpos_kern(font, left, right):
    """Pair adjustment from GPOS 'kern' (PairPos fmt 1/2), else legacy kern."""
    if "GPOS" in font:
        gpos = font["GPOS"].table
        lookups = set()
        for fr in gpos.FeatureList.FeatureRecord:
            if fr.FeatureTag == "kern":
                lookups.update(fr.Feature.LookupListIndex)
        for li in sorted(lookups):
            lk = gpos.LookupList.Lookup[li]
            for st in lk.SubTable:
                if lk.LookupType == 9:
                    st = st.ExtSubTable
                if getattr(st, "LookupType", 2) != 2 or left not in st.Coverage.glyphs:
                    continue
                if st.Format == 1:
                    ps = st.PairSet[st.Coverage.glyphs.index(left)]
                    for pvr in ps.PairValueRecord:
                        if pvr.SecondGlyph == right and pvr.Value1:
                            return getattr(pvr.Value1, "XAdvance", 0) or 0
                elif st.Format == 2:
                    c1 = st.ClassDef1.classDefs.get(left, 0)
                    c2 = st.ClassDef2.classDefs.get(right, 0)
                    v = st.Class1Record[c1].Class2Record[c2].Value1
                    if v and getattr(v, "XAdvance", 0):
                        return v.XAdvance
    if "kern" in font:
        for t in font["kern"].kernTables:
            if (left, right) in t.kernTable:
                return t.kernTable[(left, right)]
    return 0


def word_paths(font, height):
    """Return ([(d, part)], width) with cap height scaled to `height`, baseline at y=height."""
    upm = font["head"].unitsPerEm
    cap = getattr(font["OS/2"], "sCapHeight", 0) or font.getGlyphSet()["H"]._glyph.yMax
    s = height / cap
    cmap, gs, hmtx = font.getBestCmap(), font.getGlyphSet(), font["hmtx"]
    names = [cmap[ord(c)] for c in WORD]
    x, out, ys = 0.0, [], [0.0, float(height)]
    for i, g in enumerate(names):
        pen = SVGPathPen(gs)
        gs[g].draw(TransformPen(pen, (s, 0, 0, -s, x * s, height)))
        bp = BoundsPen(gs); gs[g].draw(TransformPen(bp, (s, 0, 0, -s, x * s, height)))
        if bp.bounds:
            ys += [bp.bounds[1], bp.bounds[3]]
        out.append((pen.getCommands(), "ink" if i < SPLIT else "lit"))
        x += hmtx[g][0]
        if i < len(names) - 1:
            x += gpos_kern(font, g, names[i + 1]) + TRACKING * upm
    # visual width: last glyph's right side bearing excluded
    last = names[-1]
    adv, lsb = hmtx[last]
    xmax = gs[last]._glyph.xMax if hasattr(gs[last], "_glyph") and hasattr(gs[last]._glyph, "xMax") else adv
    width = (x - adv + xmax) * s
    first_lsb = hmtx[names[0]][1] * s
    return out, width, first_lsb, (min(ys), max(ys))


def group(paths, ink, lit, dx):
    ink_d = " ".join(d for d, r in paths if r == "ink")
    lit_d = " ".join(d for d, r in paths if r == "lit")
    return (f'<g transform="translate({dx:g} 0)"><path fill="{ink}" d="{ink_d}"/>'
            f'<path fill="{lit}" d="{lit_d}"/></g>')


def mark(ink, lit):
    return "".join(f'<polygon points="{pts_str(p)}" fill="{lit if r == "lit" else ink}"/>'
                   for p, r in polygons(master_rows()))


def svg(w, y0, y1, body, title="EbbTop"):
    """viewBox spans the true glyph bounds so round overshoots are never clipped."""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 {y0:.3f} {w:.3f} {y1 - y0:.3f}" role="img">'
            f'<title>{title}</title>{body}</svg>\n')


def main(path):
    font = load(path)
    H = 96
    paths, width, lsb, (y0, y1) = word_paths(font, H)
    out = os.path.join(ROOT, "svg")
    os.makedirs(out, exist_ok=True)
    for suffix, ink, lit in (("", GRAPHITE, TEAL), ("-dark", FROST, TEAL_BRIGHT)):
        # wordmark alone: flush left on the E's stem
        open(f"{out}/ebbtop-wordmark{suffix}.svg", "w").write(
            svg(width - lsb, y0, y1, group(paths, ink, lit, -lsb)))
        # lockup: mark | one row | word
        x0 = 96 + ROW
        open(f"{out}/ebbtop-lockup{suffix}.svg", "w").write(
            svg(x0 + width - lsb, y0, y1, mark(ink, lit) + group(paths, ink, lit, x0 - lsb)))
    print("wrote wordmark + lockups from", os.path.basename(path))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
