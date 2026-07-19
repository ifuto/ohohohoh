#!/usr/bin/env python3
"""mc_sprite_sheet.png 組み立て: 17 スプライトページ + 絵画アトラスを縦に結合.
各ページは 2x 最近傍拡大、ヘッダバーにページ名、タイル格子線つき."""
import os
from PIL import Image, ImageDraw

BASE = os.path.dirname(os.path.abspath(__file__))
SPR = os.path.join(BASE, "sprites")
PAINT = os.path.join(BASE, "paintings", "paintings_atlas.png")
OUT = os.path.join(BASE, "mc_sprite_sheet.png")

PAGES = ["items_0", "blocks_0", "blocks_1", "decor", "nature", "gui", "icons",
         "particles", "environment", "mobs_0", "paintings_0", "map", "misc",
         "mobile", "armor", "font", "models"]

SCALE = 2
W = 256 * SCALE  # 512
HDR = 22


def main():
    pages = []
    for name in PAGES:
        im = Image.open(os.path.join(SPR, name + ".png")).convert("RGBA")
        # checker underlay so alpha tiles are visible
        bg = Image.new("RGBA", im.size, (34, 34, 42, 255))
        bg.alpha_composite(im)
        pages.append((name, bg.resize((W, W), Image.NEAREST).convert("RGB")))
    atlas = Image.open(PAINT).convert("RGB")
    asc = 2
    atlas = atlas.resize((atlas.width * asc, atlas.height * asc), Image.NEAREST)

    total_h = sum(W + HDR for _, _ in pages) + atlas.height + HDR + 8
    width = max(W, atlas.width)
    sheet = Image.new("RGB", (width, total_h), (18, 18, 24))
    d = ImageDraw.Draw(sheet)
    y = 0
    for i, (name, pim) in enumerate(pages):
        d.rectangle([0, y, width - 1, y + HDR - 1], fill=(40, 38, 52))
        d.rectangle([0, y, 3, y + HDR - 1], fill=(140, 120, 220))
        d.text((10, y + 6), f"P{i:02d}  {name}.png   16x16 grid, 256px",
               fill=(200, 196, 216))
        y += HDR
        sheet.paste(pim, (0, y))
        for g in range(1, 16):
            d.line([(g * 32, y), (g * 32, y + W)], fill=(0, 0, 0), width=1)
            d.line([(0, g * 32), (W, y + g * 32)], fill=(0, 0, 0), width=1)
        y += W
    d.rectangle([0, y, width - 1, y + HDR - 1], fill=(40, 38, 52))
    d.rectangle([0, y, 3, y + HDR - 1], fill=(220, 160, 120))
    d.text((10, y + 6), "P17  paintings atlas  (47 vanilla paintings, native res)",
           fill=(200, 196, 216))
    y += HDR + 4
    sheet.paste(atlas, (0, y))
    sheet.save(OUT)
    print("sheet:", OUT, sheet.size)


if __name__ == "__main__":
    main()
