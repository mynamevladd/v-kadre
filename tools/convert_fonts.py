"""Convert the ecosystem fonts (woff2 from Strochka) into static TTF files Qt can load.

python tools/convert_fonts.py <strochka src/assets/fonts dir>
Needs: pip install fonttools brotli
"""
import shutil
import sys
from pathlib import Path

from fontTools.merge import Merger
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

src = Path(sys.argv[1])
out = Path(__file__).resolve().parents[1] / "speakertrack" / "ui" / "fonts"
out.mkdir(parents=True, exist_ok=True)
tmp = out / "_tmp"
tmp.mkdir(exist_ok=True)


def to_ttf(name, weight=None):
    f = TTFont(src / name)
    if weight is not None and "fvar" in f:
        f = instantiateVariableFont(f, {"wght": weight})
    f.flavor = None
    p = tmp / (Path(name).stem + (f"-{weight}" if weight else "") + ".ttf")
    f.save(p)
    return p


def merge(parts, dest, family=None, style=None, weight=None):
    """Latin + Cyrillic subsets -> one file; optionally give the static instance proper names."""
    font = Merger().merge([str(p) for p in parts])
    if family:
        name = font["name"]
        legacy_family = family if style in ("Regular", "Bold") else f"{family} {style}"
        legacy_style = style if style in ("Regular", "Bold") else "Regular"
        for nid in (1, 2, 4, 6, 16, 17):
            name.removeNames(nameID=nid)
        for nid, val in ((1, legacy_family), (2, legacy_style), (4, f"{family} {style}"),
                         (6, f"{family}-{style}"), (16, family), (17, style)):
            name.setName(val, nid, 3, 1, 0x409)
            name.setName(val, nid, 1, 0, 0)
        font["OS/2"].usWeightClass = weight
        font["OS/2"].fsSelection = (font["OS/2"].fsSelection & ~0b1100001) | (0b100000 if style == "Bold" else 0b1000000)
        font["head"].macStyle = 1 if style == "Bold" else 0
    font.save(out / dest)
    print("wrote", dest)


merge([to_ttf("unbounded-latin-900-normal.woff2"), to_ttf("unbounded-cyrillic-900-normal.woff2")],
      "Unbounded-Black.ttf")
for w, n in ((500, "Medium"), (700, "Bold"), (800, "ExtraBold")):
    merge([to_ttf("manrope-latin-wght-normal.woff2", w), to_ttf("manrope-cyrillic-wght-normal.woff2", w)],
          f"Manrope-{n}.ttf", "Manrope", n, w)
merge([to_ttf("jetbrains-mono-latin-800-normal.woff2"), to_ttf("jetbrains-mono-cyrillic-800-normal.woff2")],
      "JetBrainsMono-ExtraBold.ttf")
for lic in ("Unbounded-OFL.txt", "Manrope-OFL.txt", "JetBrainsMono-OFL.txt"):
    shutil.copy(src / lic, out / lic)
shutil.rmtree(tmp)
