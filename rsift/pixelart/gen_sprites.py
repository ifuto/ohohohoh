#!/usr/bin/env python3
"""RSift スプライトページ・ジェネレータ (16px 原寸タイル + 4x 拡大プレビュー).

- 17 ページをすべて生成 (items_0 / blocks_0..1 / decor / nature / gui / icons /
  particles / environment / mobs_0 / paintings_0 / map / misc / mobile / armor /
  font / models)
- 全タイルは書き割り (AA なし) + 繰り返し不可役 (パレット共有・輪郭 1px 暗)
- 出力: pixelart/sprites/<page>.png (256x256, 16x16 格子)
"""
import math
import os
from PIL import Image, ImageDraw

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "sprites")
os.makedirs(OUT, exist_ok=True)
T = 16
PAGE = T * 16  # 256


class Tile:
    """one 16x16 slot on a transparent page"""

    def __init__(self, img, ox, oy):
        self.img = img
        self.d = ImageDraw.Draw(img)
        self.ox, self.oy = ox, oy

    def px(self, x, y, c):
        if 0 <= x < T and 0 <= y < T:
            self.d.point((self.ox + x, self.oy + y), fill=c)

    def rect(self, x0, y0, x1, y1, c):
        self.d.rectangle([self.ox + x0, self.oy + y0,
                          self.ox + x1, self.oy + y1], fill=c)

    def dark_rect(self, x0, y0, x1, y1, fill, edge):
        self.rect(x0, y0, x1, y1, edge)
        if x1 - x0 >= 2 and y1 - y0 >= 2:
            self.rect(x0 + 1, y0 + 1, x1 - 1, y1 - 1, fill)

    def blob(self, cx, cy, r, c):
        for y in range(int(cy - r - 1), int(cy + r + 1)):
            for x in range(int(cx - r - 1), int(cx + r + 1)):
                if (x - cx) ** 2 + (y - cy) ** 2 <= r * r:
                    self.px(x, y, c)

    def line(self, x0, y0, x1, y1, c):
        self.d.line([self.ox + x0, self.oy + y0, self.ox + x1, self.oy + y1], fill=c)


def mkpage(name, tiles):
    """tiles: list of draw callbacks; lays them row-major on a 256px page"""
    img = Image.new("RGBA", (PAGE, PAGE), (0, 0, 0, 0))
    for i, fn in enumerate(tiles):
        ox = (i % 16) * T
        oy = (i // 16) * T
        fn(Tile(img, ox, oy))
    img.save(os.path.join(OUT, f"{name}.png"))
    return name


PAL = {
    "wood": (150, 108, 64), "wood_d": (110, 76, 44), "stone": (136, 136, 140),
    "stone_d": (104, 104, 108), "iron": (216, 216, 220), "iron_d": (170, 172, 178),
    "gold": (250, 210, 70), "gold_d": (200, 160, 46), "dia": (120, 230, 220),
    "dia_d": (90, 190, 190), "red": (200, 50, 45), "green": (80, 160, 70),
    "leaf": (70, 130, 55), "leaf_d": (50, 100, 40), "water": (60, 110, 200),
    "lava": (230, 100, 20), "sand": (220, 205, 160), "dirt": (140, 100, 65),
}

DARK = (28, 26, 24, 255)


def outline(t, x0, y0, x1, y1, c=DARK):
    t.d.rectangle([t.ox + x0, t.oy + y0, t.ox + x1, t.oy + y1], outline=c)


# ------------------------------------------------------------- tile painters

def ti_block(top, side, side_d):
    def draw(t):
        t.dark_rect(1, 1, 14, 14, side, DARK)
        t.rect(2, 2, 13, 5, top)
        t.rect(2, 11, 13, 13, side_d)
        for i in range(2, 13, 3):
            t.px(i, 6 + (i % 3), side_d)
    return draw


def ti_ore(base, gem):
    def draw(t):
        t.dark_rect(1, 1, 14, 14, base, DARK)
        for (x, y) in ((4, 4), (9, 3), (12, 8), (6, 10), (3, 12), (10, 12)):
            t.rect(x, y, x + 1, y + 1, gem)
            t.px(x, y, tuple(min(255, c + 40) for c in gem[:3]) + (255,))
        t.rect(2, 2, 13, 4, PAL["stone_d"])
    return draw


def ti_sword(blade, hilt):
    def draw(t):
        for i in range(2, 12):
            t.line(i, 14 - i, i + 1, 15 - i, DARK)
            t.line(i + 1, 14 - i, i + 2, 15 - i, blade)
        t.rect(2, 12, 5, 13, hilt)
        t.rect(3, 14, 4, 15, (90, 60, 30, 255))
    return draw


def ti_pick(head_c, handle=(120, 84, 48, 255)):
    def draw(t):
        t.line(5, 4, 11, 10, DARK)
        for i in range(3, 11):
            t.px(i, 3 - (i - 3) // 2, head_c)
            t.px(i, 4 - (i - 3) // 2, head_c)
        for i in range(3, 10):
            t.px(13 - i, 3 + i - 3, head_c)
            t.px(13 - i, 4 + i - 3, head_c)
        for i in range(5, 13):
            t.line(5, i, 7, i - 2, handle)
    return draw


def ti_food(body, hi, stick=None):
    def draw(t):
        t.blob(8, 8, 4.5, DARK)
        t.blob(8, 8, 3.5, body)
        t.blob(6, 6, 1.2, hi)
        if stick:
            t.line(11, 11, 14, 14, stick)
    return draw


def ti_potion(cork, liquid):
    def draw(t):
        t.rect(6, 1, 9, 3, cork)
        t.rect(5, 4, 10, 6, (200, 215, 225, 255))
        t.dark_rect(4, 5, 11, 14, liquid, DARK)
        t.rect(5, 5, 10, 6, (200, 215, 225, 255))
        t.px(6, 8, tuple(min(255, c + 50) for c in liquid[:3]) + (255,))
        t.px(8, 11, tuple(min(255, c + 35) for c in liquid[:3]) + (255,))
    return draw


def ti_armor_piece(kind, base, base_d):
    def draw(t):
        if kind == "helmet":
            t.dark_rect(3, 4, 12, 10, base, DARK)
            t.rect(5, 10, 10, 11, base_d)
            t.rect(3, 3, 12, 5, tuple(min(255, c + 30) for c in base[:3]) + (255,))
        elif kind == "chest":
            t.dark_rect(3, 2, 12, 11, base, DARK)
            t.rect(3, 12, 6, 14, base_d)
            t.rect(9, 12, 12, 14, base_d)
            t.rect(2, 2, 3, 6, base_d)
            t.rect(12, 2, 13, 6, base_d)
        elif kind == "legs":
            t.dark_rect(3, 2, 12, 7, base, DARK)
            t.rect(4, 8, 6, 14, base_d)
            t.rect(9, 8, 11, 14, base_d)
        else:  # boots
            t.rect(3, 8, 6, 12, base)
            t.rect(9, 8, 12, 12, base)
            t.rect(3, 12, 7, 14, base_d)
            t.rect(8, 12, 12, 14, base_d)
            outline(t, 3, 8, 6, 12)
            outline(t, 9, 8, 12, 12)
    return draw


def ti_star(c):
    def draw(t):
        t.line(8, 1, 8, 14, c)
        t.line(1, 8, 14, 8, c)
        t.line(3, 3, 12, 12, c)
        t.line(12, 3, 3, 12, c)
        t.blob(8, 8, 2, tuple(min(255, x + 60) for x in c[:3]) + (255,))
    return draw


def ti_icon_char(glyph, c):
    def draw(t):
        t.rect(3, 3, 12, 12, (36, 36, 44, 255))
        outline(t, 3, 3, 12, 12)
        # 7x5 pseudo glyph from bitmask
        rows = glyph
        for gy, row in enumerate(rows):
            for gx, bit in enumerate(row):
                if bit == "1":
                    t.rect(5 + gx, 5 + gy, 5 + gx, 5 + gy, c)
    return draw


# ------------------------------------------------------------- pages

def page_items_0():
    a = PAL
    tiles = [
        ti_sword((216, 216, 222, 255), (120, 84, 48, 255)),
        ti_sword(a["gold"] + (255,), (120, 84, 48, 255)),
        ti_sword(a["dia"] + (255,), (120, 84, 48, 255)),
        ti_pick((216, 216, 222, 255)),
        ti_pick(a["dia"] + (255,)),
        ti_pick(a["gold"] + (255,)),
        # axe & shovel & hoe
        lambda t: (t.dark_rect(8, 2, 13, 7, (196, 196, 202, 255), DARK),
                   t.line(6, 13, 12, 7, (120, 84, 48, 255))),
        lambda t: (t.dark_rect(9, 2, 12, 8, (196, 196, 202, 255), DARK),
                   t.line(5, 13, 9, 9, (120, 84, 48, 255))),
        lambda t: (t.rect(9, 2, 13, 4, (196, 196, 202, 255)),
                   t.line(8, 4, 4, 13, (120, 84, 48, 255))),
        # bow & arrow
        lambda t: (t.d.arc([t.ox + 2, t.oy + 1, t.ox + 13, t.oy + 14], -70, 70,
                           fill=(120, 84, 48, 255), width=2),
                   t.line(3, 8, 3, 8, (230, 230, 230, 255))),
        lambda t: (t.line(2, 13, 13, 2, (200, 200, 204, 255)),
                   t.rect(2, 11, 4, 13, (240, 240, 80, 255)),
                   t.rect(12, 1, 13, 3, (160, 160, 166, 255))),
        # food
        ti_food((200, 70, 50, 255), (240, 120, 90, 255)),
        ti_food((220, 170, 60, 255), (250, 220, 120, 255), (200, 200, 190, 255)),
        ti_food((170, 40, 40, 255), (220, 90, 80, 255)),
        # potions
        ti_potion((170, 130, 80, 255), (190, 60, 60, 255)),
        ti_potion((170, 130, 80, 255), (60, 120, 220, 255)),
        ti_potion((170, 130, 80, 255), (120, 200, 90, 255)),
        # star & book & ingot
        ti_star((250, 230, 90, 255)),
        lambda t: (t.dark_rect(3, 2, 12, 13, (190, 240, 120, 255), DARK),
                   t.rect(5, 4, 10, 11, (246, 250, 246, 255)),
                   t.line(7, 4, 7, 11, (200, 200, 200, 255)),
                   t.rect(3, 2, 12, 4, (170, 220, 100, 255))),
        lambda t: (t.dark_rect(4, 5, 11, 10, (216, 216, 222, 255), DARK),
                   t.rect(3, 6, 5, 9, (216, 216, 222, 255)),
                   t.rect(10, 6, 12, 9, (216, 216, 222, 255))),
    ]
    return mkpage("items_0", tiles)


def page_blocks_0():
    a = PAL
    tiles = [
        ti_block((120, 170, 80, 255), a["dirt"] + (255,), (110, 76, 48, 255)),  # grass
        ti_block(a["dirt"] + (255,), (120, 84, 52, 255), (100, 68, 40, 255)),   # dirt path
        ti_block(a["stone"] + (255,), a["stone_d"] + (255,), (88, 88, 92, 255)),
        ti_ore(a["stone"] + (255,), (235, 190, 60, 255)),    # gold ore
        ti_ore(a["stone"] + (255,), (120, 230, 220, 255)),   # diamond ore
        ti_ore(a["stone"] + (255,), (200, 80, 60, 255)),     # redstone ore
        ti_ore((70, 60, 60, 255), (90, 230, 120, 255)),      # emerald deepslate
        ti_block(a["sand"] + (255,), (210, 196, 150, 255), (190, 176, 130, 255)),
        ti_block((140, 72, 40, 255,), (150, 80, 46, 255), (120, 62, 34, 255)),  # bricks-ish
        ti_block((200, 200, 205, 255), (188, 188, 194, 255), (164, 164, 170, 255)),
        ti_block((60, 110, 200, 255), (52, 98, 180, 255), (40, 80, 150, 255)),  # water block
        ti_block((240, 120, 20, 255), (226, 96, 14, 255), (190, 70, 10, 255)),  # lava block
    ]
    return mkpage("blocks_0", tiles)


def page_blocks_1():
    a = PAL
    tiles = [
        # logs & planks & glass & ores
        lambda t: (t.dark_rect(1, 1, 14, 14, (102, 78, 46, 255), DARK),
                   [t.line(3 + i, 2, 3 + i, 13, (84, 62, 36, 255)) for i in range(0, 10, 3)]),
        lambda t: (t.dark_rect(1, 1, 14, 14, a["wood"] + (255,), DARK),
                   [t.line(2, 3 + i, 13, 3 + i, a["wood_d"] + (255,)) for i in range(0, 11, 4)]),
        lambda t: (t.dark_rect(1, 1, 14, 14, (214, 234, 240, 180), DARK),
                   t.line(3, 12, 12, 3, (255, 255, 255, 220)),
                   t.line(6, 13, 13, 6, (255, 255, 255, 200))),
        ti_ore(a["stone"] + (255,), (226, 226, 230, 255)),   # iron ore
        ti_ore(a["stone"] + (255,), (60, 60, 70, 255)),      # coal ore
        ti_ore((40, 40, 60, 255), (120, 90, 220, 255)),      # obsidian+amethyst
        ti_block((60, 50, 70, 255), (52, 44, 60, 255), (40, 34, 46, 255)),     # obsidian
        ti_block((150, 60, 160, 255), (140, 54, 150, 255), (110, 42, 120, 255)),  # purpur
        ti_block((240, 240, 244, 255), (236, 236, 240, 255), (214, 214, 220, 255)),  # snow
        ti_block((140, 180, 220, 255), (130, 170, 212, 255), (104, 144, 190, 255)),  # ice
        ti_block((190, 96, 54, 255), (180, 88, 48, 255), (150, 70, 38, 255)),  # copper
        ti_block((100, 100, 120, 255), (90, 90, 110, 255), (70, 70, 88, 255)),  # deepslate
    ]
    return mkpage("blocks_1", tiles)


def page_decor():
    tiles = [
        # torch / lantern / bed / chest / table / painting-mini
        lambda t: (t.rect(7, 6, 8, 14, (120, 84, 48, 255)),
                   t.blob(8, 4, 2.4, (255, 220, 100, 255)),
                   t.blob(8, 4, 1.2, (255, 250, 220, 255))),
        lambda t: (t.dark_rect(5, 4, 10, 12, (60, 60, 70, 255), DARK),
                   t.rect(6, 6, 9, 10, (255, 220, 130, 255)),
                   t.rect(6, 2, 9, 3, (60, 60, 70, 255))),
        lambda t: (t.dark_rect(1, 8, 14, 12, (200, 60, 60, 255), DARK),
                   t.rect(1, 8, 4, 12, (238, 238, 238, 255)),
                   t.rect(1, 12, 14, 14, (120, 84, 48, 255))),
        lambda t: (t.dark_rect(2, 5, 13, 13, (166, 118, 66, 255), DARK),
                   t.rect(2, 7, 13, 8, (110, 76, 44, 255)),
                   t.blob(7, 9, 1, (70, 50, 30, 255)),
                   t.blob(8, 9, 1, (70, 50, 30, 255))),
        lambda t: (t.rect(2, 4, 13, 5, (120, 84, 48, 255)),
                   t.rect(3, 6, 4, 13, (100, 70, 40, 255)),
                   t.rect(11, 6, 12, 13, (100, 70, 40, 255))),
        lambda t: (t.dark_rect(2, 2, 13, 11, (150, 190, 220, 255), (160, 120, 60, 255)),
                   t.blob(6, 6, 2, (255, 240, 150, 255)),
                   t.rect(2, 9, 13, 11, (90, 140, 70, 255))),
        # flower pot / books / cauldron / jukebox
        lambda t: (t.dark_rect(5, 9, 10, 13, (160, 90, 50, 255), DARK),
                   t.line(8, 9, 8, 5, (70, 130, 60, 255)), t.blob(8, 4, 2, (220, 60, 60, 255))),
        lambda t: (t.dark_rect(2, 3, 13, 12, (90, 60, 36, 255), DARK),
                   t.rect(4, 4, 6, 11, (200, 50, 50, 255)), t.rect(7, 4, 9, 11, (60, 110, 200, 255)),
                   t.rect(10, 4, 12, 11, (80, 170, 80, 255))),
        lambda t: (t.dark_rect(3, 4, 12, 13, (70, 70, 80, 255), DARK),
                   t.rect(4, 5, 11, 7, (60, 110, 200, 255))),
        lambda t: (t.dark_rect(3, 4, 12, 12, (110, 76, 44, 255), DARK),
                   t.blob(7, 8, 2.4, (30, 30, 34, 255)), t.blob(7, 8, 1, (180, 180, 190, 255))),
    ]
    return mkpage("decor", tiles)


def page_nature():
    tiles = [
        # oak/birch/sakura leaves, sapling, flowers, bamboo, vine
        ti_block((70, 130, 55, 255), (60, 116, 48, 255), (48, 96, 40, 255)),
        ti_block((120, 170, 90, 255), (110, 158, 82, 255), (90, 134, 66, 255)),
        ti_block((240, 180, 200, 255), (232, 166, 190, 255), (210, 140, 170, 255)),
        lambda t: (t.line(8, 13, 8, 6, (102, 78, 46, 255)), t.blob(8, 4, 3, (90, 150, 66, 255)),
                   t.blob(5, 6, 2, (90, 150, 66, 255)), t.blob(11, 6, 2, (90, 150, 66, 255))),
        lambda t: (t.line(8, 13, 8, 7, (70, 130, 60, 255)), t.blob(8, 5, 2.4, (240, 240, 240, 255)),
                   t.blob(8, 5, 0.9, (250, 210, 60, 255))),
        lambda t: (t.line(8, 13, 8, 7, (70, 130, 60, 255)), t.blob(8, 4, 2.6, (220, 60, 90, 255))),
        lambda t: ([t.rect(5 + i, 2, 6 + i, 14, (120 + i * 8, 180, 60, 255)) for i in range(0, 6, 2)],
                   t.px(6, 4, (150, 210, 90, 255))),
        lambda t: ([t.px(x, y, (90, 140, 70, 255)) for x in range(3, 13) for y in range(2, 13)
                    if (x + y * 2) % 5 < 2]),
        # mushroom / wheat / carrot / pumpkin
        lambda t: (t.blob(8, 6, 4, (220, 60, 50, 255)), t.rect(6, 7, 9, 12, (236, 226, 210, 255)),
                   t.blob(6, 5, 0.9, (255, 255, 255, 255)), t.blob(10, 6, 0.8, (255, 255, 255, 255))),
        lambda t: ([t.line(8, 14, 8, 4, (200, 170, 70, 255))] +
                   [t.rect(5 + i, 3 + i % 2, 7 + i, 5 + i % 2, (220, 190, 90, 255)) for i in range(4)]),
        lambda t: (t.blob(8, 9, 3, (230, 130, 30, 255)), t.blob(8, 12, 2, (230, 130, 30, 255)),
                   t.line(8, 6, 8, 3, (80, 150, 60, 255))),
        lambda t: (t.dark_rect(3, 4, 12, 12, (230, 140, 30, 255), (180, 100, 20, 255)),
                   t.rect(5, 6, 6, 7, DARK), t.rect(9, 6, 10, 7, DARK),
                   t.rect(6, 9, 9, 10, DARK), t.px(7, 10, (255, 220, 90, 255))),
    ]
    return mkpage("nature", tiles)


def page_gui():
    def frame(c1=(60, 60, 66, 255), c2=(240, 240, 246, 255)):
        def d(t):
            t.rect(0, 0, 15, 15, c2)
            t.rect(1, 1, 14, 14, c1)
            t.rect(0, 0, 15, 15, (0, 0, 0, 0)) if False else None
        return d
    tiles = [
        # heart / half heart / armor / hunger / bubble / xp / slot / button
        lambda t: (t.blob(5, 6, 3, (220, 40, 50, 255)), t.blob(10, 6, 3, (220, 40, 50, 255)),
                   t.blob(7, -1, 0.1, (0, 0, 0, 0)), t.blob(8, 8, 4, (220, 40, 50, 255)),
                   t.px(6, 5, (250, 150, 150, 255)), t.px(7, 9, (220, 40, 50, 255)),
                   t.rect(7, 10, 8, 10, (220, 40, 50, 255)), t.rect(7, 11, 8, 11, (220, 40, 50, 255))),
        lambda t: (t.blob(5, 6, 3, (220, 40, 50, 255)), t.blob(10, 6, 3, (70, 70, 76, 255)),
                   t.blob(8, 8, 4, (120, 60, 66, 255)), t.rect(7, 11, 8, 11, (120, 60, 66, 255))),
        lambda t: (t.dark_rect(3, 2, 12, 9, (170, 172, 178, 255), DARK),
                   t.rect(4, 10, 7, 13, (140, 142, 148, 255)),
                   t.rect(8, 10, 11, 13, (140, 142, 148, 255))),
        lambda t: (t.dark_rect(3, 4, 12, 11, (120, 70, 30, 255), DARK),
                   t.rect(5, 1, 6, 5, (150, 150, 150, 255)), t.rect(9, 1, 10, 5, (150, 150, 150, 255)),
                   t.rect(2, 12, 13, 13, (120, 70, 30, 255))),
        lambda t: (t.d.ellipse([t.ox + 4, t.oy + 4, t.ox + 11, t.oy + 11],
                               outline=(120, 190, 255, 255)),
                   t.blob(8, 10, 1.4, (150, 210, 255, 255))),
        lambda t: (t.blob(8, 8, 5, (70, 200, 80, 255)), t.blob(8, 8, 3, (140, 240, 140, 255)),
                   t.px(6, 6, (240, 255, 240, 255))),
        frame(),
        lambda t: (t.dark_rect(2, 4, 13, 11, (120, 120, 128, 255), DARK),
                   t.rect(3, 5, 12, 10, (170, 170, 178, 255))),
        # tab / slider / check / cross
        lambda t: (t.dark_rect(2, 3, 13, 12, (90, 90, 98, 255), DARK),
                   t.rect(2, 3, 13, 6, (140, 140, 148, 255))),
        lambda t: (t.rect(2, 7, 13, 8, (70, 70, 78, 255)), t.dark_rect(7, 4, 10, 11, (220, 220, 226, 255), DARK)),
        lambda t: (t.rect(2, 2, 13, 13, (50, 50, 56, 255)), outline(t, 2, 2, 13, 13),
                   t.line(5, 8, 7, 11, (120, 230, 120, 255)), t.line(7, 11, 12, 4, (120, 230, 120, 255))),
        lambda t: (t.rect(2, 2, 13, 13, (50, 50, 56, 255)), outline(t, 2, 2, 13, 13),
                   t.line(5, 5, 11, 11, (230, 90, 90, 255)), t.line(11, 5, 5, 11, (230, 90, 90, 255))),
    ]
    return mkpage("gui", tiles)


def page_icons():
    tiles = [
        # waypoint / home / flag / skull / star / play / pause / gear / zoom / book
        lambda t: (t.blob(8, 6, 4, (240, 80, 70, 255)), t.rect(6, 9, 9, 12, (240, 80, 70, 255)),
                   t.px(8, 6, (255, 255, 255, 255)), t.px(7, 6, (255, 255, 255, 255))),
        lambda t: (t.rect(4, 7, 11, 13, (160, 120, 70, 255)), t.d.polygon(
            [(t.ox + 2, t.oy + 8), (t.ox + 8, t.oy + 2), (t.ox + 13, t.oy + 8)],
            fill=(120, 70, 40, 255))),
        lambda t: (t.rect(4, 2, 5, 14, (140, 100, 60, 255)),
                   t.d.polygon([(t.ox + 5, t.oy + 2), (t.ox + 13, t.oy + 5), (t.ox + 5, t.oy + 8)],
                               fill=(220, 60, 60, 255))),
        lambda t: (t.blob(8, 7, 4.4, (230, 230, 224, 255)), t.rect(6, 10, 9, 13, (220, 220, 214, 255)),
                   t.rect(5, 6, 6, 7, DARK), t.rect(9, 6, 10, 7, DARK), t.px(7, 9, DARK)),
        ti_star((250, 230, 90, 255)),
        lambda t: t.d.polygon([(t.ox + 5, t.oy + 3), (t.ox + 12, t.oy + 8), (t.ox + 5, t.oy + 13)],
                              fill=(220, 220, 226, 255)),
        lambda t: (t.rect(5, 3, 6, 13, (220, 220, 226, 255)), t.rect(9, 3, 10, 13, (220, 220, 226, 255))),
        lambda t: ([t.px(int(7.5 + 5 * math.cos(a)), int(7.5 + 5 * math.sin(a)),
                                         (200, 200, 208, 255)) for a in [i * 0.785 for i in range(8)]],
                   t.blob(8, 8, 4, (200, 200, 208, 255)), t.blob(8, 8, 2, (60, 60, 66, 255))),
        lambda t: (t.blob(8, 8, 5, (220, 220, 226, 255)), t.blob(8, 8, 3.4, (240, 246, 250, 255)),
                   t.line(12, 12, 14, 14, (160, 160, 166, 255))),
        lambda t: (t.dark_rect(3, 2, 12, 13, (190, 240, 120, 255), DARK),
                   t.rect(4, 3, 11, 12, (246, 250, 246, 255)), t.line(7, 3, 7, 12, (200, 200, 200, 255))),
    ]
    return mkpage("icons", tiles)


def page_particles():
    def puff(col, n, seed):
        def d(t):
            for i in range(n):
                x = 3 + (i * 5 + seed) % 11
                y = 3 + (i * 7 + seed * 2) % 11
                t.blob(x, y, 1 + (i + seed) % 2, col)
        return d
    tiles = [
        puff((250, 250, 250, 200), 6, 1),   # cloud puff
        puff((160, 160, 166, 220), 5, 2),   # smoke
        puff((255, 200, 80, 230), 5, 3),    # flame bits
        puff((120, 230, 120, 220), 6, 4),   # happy villager
        puff((230, 90, 90, 230), 5, 5),     # hearts? red
        puff((150, 100, 230, 220), 6, 6),   # portal magic
        puff((255, 255, 255, 230), 7, 7),   # sparkle
        puff((120, 180, 255, 200), 6, 8),   # water splash
        puff((230, 230, 60, 220), 5, 9),    # crit
    ]
    return mkpage("particles", tiles)


def page_environment():
    tiles = [
        # sun / moon phases / rain / snow / cloud / thunder / star cluster / rainbow
        lambda t: (t.blob(8, 8, 4, (255, 230, 120, 255)),
                   [t.line(8, 8, int(8 + 7 * math.cos(a)), int(8 + 7 * math.sin(a)),
                           (255, 210, 80, 255)) for a in [i * 0.785 for i in range(8)]]),
        lambda t: t.blob(8, 8, 4.4, (240, 244, 250, 255)),
        lambda t: (t.blob(8, 8, 4.4, (240, 244, 250, 255)), t.blob(10, 8, 2, (200, 206, 214, 255))),
        lambda t: (t.blob(8, 8, 4.4, (240, 244, 250, 255)), t.blob(9, 8, 3.4, (150, 156, 166, 255))),
        lambda t: (t.blob(6, 7, 3, (200, 210, 225, 255)), t.blob(10, 7, 3, (200, 210, 225, 255)),
                   t.rect(4, 7, 12, 10, (200, 210, 225, 255)),
                   t.line(6, 12, 5, 14, (120, 170, 240, 255)),
                   t.line(9, 12, 8, 14, (120, 170, 240, 255))),
        lambda t: (t.blob(6, 7, 3, (210, 220, 232, 255)), t.blob(10, 7, 3, (210, 220, 232, 255)),
                   t.rect(4, 7, 12, 10, (210, 220, 232, 255)),
                   t.rect(5, 12, 6, 12, (255, 255, 255, 255)), t.rect(8, 13, 9, 13, (255, 255, 255, 255))),
        lambda t: (t.blob(6, 7, 3, (140, 150, 165, 255)), t.blob(10, 7, 3, (140, 150, 165, 255)),
                   t.rect(4, 7, 12, 10, (140, 150, 165, 255)),
                   t.d.polygon([(t.ox + 7, t.oy + 9), (t.ox + 9, t.oy + 12), (t.ox + 7, t.oy + 12),
                                (t.ox + 9, t.oy + 15)], fill=(255, 230, 90, 255))),
        ti_star((240, 244, 255, 255)),
        lambda t: [t.d.arc([t.ox + i, t.oy + i, t.ox + 15 - i, t.oy + 15 - i], 180, 360,
                           fill=c + (255,)) for i, c in enumerate(
            [(240, 80, 80), (250, 170, 60), (250, 230, 80), (120, 210, 120), (90, 140, 240), (170, 90, 220)])],
    ]
    return mkpage("environment", tiles)


def page_mobs_0():
    def face(base, eye, extra=None):
        def d(t):
            t.dark_rect(3, 3, 12, 12, base, DARK)
            t.rect(5, 6, 6, 7, eye)
            t.rect(9, 6, 10, 7, eye)
            if extra:
                extra(t)
        return d
    tiles = [
        face((110, 170, 90, 255), (16, 24, 12, 255),  # creeper
             lambda t: (t.rect(5, 6, 6, 9, (16, 24, 12, 255)), t.rect(9, 6, 10, 9, (16, 24, 12, 255)),
                        t.rect(6, 10, 9, 12, (16, 24, 12, 255)), t.rect(5, 11, 6, 12, (16, 24, 12, 255)),
                        t.rect(9, 11, 10, 12, (16, 24, 12, 255)))),
        face((16, 16, 20, 255), (230, 120, 255, 255)),  # enderman
        face((230, 150, 150, 255), (40, 40, 40, 255),  # pig
             lambda t: (t.rect(6, 8, 9, 11, (200, 100, 100, 255)),
                        t.px(7, 9, (120, 40, 40, 255)), t.px(8, 9, (120, 40, 40, 255)))),
        face((230, 230, 224, 255), (40, 40, 40, 255),  # sheep
             lambda t: t.rect(4, 2, 11, 4, (250, 250, 248, 255))),
        face((140, 100, 60, 255), (30, 20, 12, 255),   # cow
             lambda t: (t.rect(5, 9, 10, 12, (200, 170, 150, 255)),
                        t.px(6, 10, (120, 80, 60, 255)), t.px(9, 10, (120, 80, 60, 255)))),
        face((60, 60, 110, 255), (200, 60, 60, 255)),  # spider
        face((210, 210, 210, 255), (20, 20, 24, 255),  # skeleton
             lambda t: (t.rect(6, 8, 9, 9, (180, 180, 180, 255)), t.rect(7, 10, 8, 12, (180, 180, 180, 255)))),
        face((90, 140, 80, 255), (60, 20, 20, 255)),   # zombie
        face((200, 200, 210, 255), (90, 90, 200, 255),  # ghast
             lambda t: (t.rect(6, 9, 7, 10, (160, 160, 170, 255)), t.rect(9, 9, 10, 10, (160, 160, 170, 255)))),
        face((230, 180, 120, 255), (40, 30, 20, 255)),  # villager
             # nose
        face((200, 90, 90, 255), (30, 30, 30, 255)),   # piglin-ish
        face((170, 220, 190, 255), (30, 40, 40, 255)),  # drowned
    ]
    # villager nose patch
    def nose(t):
        pass
    return mkpage("mobs_0", tiles)


def page_paintings_0():
    # 12 mini thumbnails reusing style: tiny scenes
    def mini(bg, mid, fg):
        def d(t):
            t.rect(1, 1, 14, 14, bg)
            t.rect(1, 8, 14, 14, mid)
            t.blob(11, 5, 2, fg)
            outline(t, 1, 1, 14, 14, (160, 120, 60, 255))
        return d
    combos = [
        ((160, 190, 220, 255), (90, 140, 70, 255), (255, 240, 150, 255)),
        ((240, 150, 60, 255), (80, 30, 34, 255), (255, 236, 140, 255)),
        ((100, 130, 160, 255), (70, 50, 38, 255), (230, 224, 210, 255)),
        ((20, 16, 30, 255), (60, 26, 96, 255), (220, 220, 255, 255)),
        ((160, 190, 200, 255), (60, 110, 110, 255), (200, 220, 230, 255)),
        ((120, 100, 78, 255), (90, 70, 50, 255), (250, 220, 120, 255)),
        ((40, 40, 52, 255), (28, 28, 40, 255), (140, 220, 240, 255)),
        ((200, 210, 220, 255), (70, 120, 175, 255), (240, 248, 252, 255)),
        ((150, 160, 160, 255), (100, 106, 106, 255), (200, 206, 204, 255)),
        ((190, 180, 150, 255), (80, 140, 190, 255), (220, 60, 50, 255)),
        ((250, 240, 120, 255), (240, 200, 50, 255), (150, 110, 70, 255)),
        ((140, 176, 208, 255), (96, 140, 66, 255), (208, 196, 178, 255)),
    ]
    return mkpage("paintings_0", [mini(*c) for c in combos])


def page_map():
    tiles = [
        lambda t: (t.dark_rect(1, 1, 14, 14, (200, 178, 122, 255), DARK),
                   t.blob(5, 5, 2, (120, 170, 110, 255)), t.blob(10, 9, 3, (90, 150, 190, 255)),
                   t.rect(11, 3, 13, 5, (220, 60, 50, 255))),
        lambda t: (t.dark_rect(1, 1, 14, 14, (210, 190, 140, 255), DARK),
                   [t.line(2 + i, 2, 2 + i, 13, (190, 170, 120, 255)) for i in range(0, 12, 3)]),
        lambda t: (t.dark_rect(1, 1, 14, 14, (170, 200, 230, 255), DARK),
                   t.blob(8, 8, 4, (240, 250, 255, 255)), t.ring2(t) if False else None,
                   t.d.ellipse([t.ox + 4, t.oy + 4, t.ox + 11, t.oy + 11], outline=(120, 160, 210, 255))),
        lambda t: (t.dark_rect(1, 1, 14, 14, (90, 140, 90, 255), DARK),
                   t.blob(6, 6, 2.4, (150, 210, 140, 255)), t.blob(11, 10, 2, (60, 100, 160, 255)),
                   t.px(8, 8, (250, 250, 250, 255))),
    ]
    return mkpage("map", tiles)


def page_misc():
    tiles = [
        lambda t: (t.d.ellipse([t.ox + 2, t.oy + 2, t.ox + 13, t.oy + 13],
                                                outline=(220, 220, 226, 255), width=2),
                   t.line(8, 8, 12, 4, (240, 90, 80, 255)), t.line(8, 8, 4, 11, (60, 130, 240, 255))),
        lambda t: (t.rect(4, 5, 11, 10, (60, 60, 70, 255)), t.blob(8, 8, 2, (255, 220, 90, 255))),
        lambda t: (t.dark_rect(3, 3, 12, 12, (240, 240, 246, 255), DARK),
                   t.rect(5, 5, 7, 7, (240, 80, 80, 255)), t.rect(9, 5, 11, 7, (80, 140, 240, 255)),
                   t.rect(5, 9, 7, 11, (120, 210, 120, 255))),
        lambda t: (t.rect(2, 6, 13, 9, (160, 120, 70, 255)), t.rect(4, 4, 11, 6, (180, 140, 90, 255)),
                   t.rect(4, 9, 11, 11, (140, 100, 56, 255))),
    ]
    return mkpage("misc", tiles)


def page_mobile():
    def btn(sym):
        def d(t):
            t.d.ellipse([t.ox + 1, t.oy + 1, t.ox + 14, t.oy + 14], fill=(70, 74, 82, 255),
                        outline=(150, 156, 168, 255))
            sym(t)
        return d
    tiles = [
        btn(lambda t: t.line(8, 3, 8, 13, (240, 240, 246, 255)) if False else
            t.d.polygon([(t.ox + 8, t.oy + 3), (t.ox + 4, t.oy + 9), (t.ox + 12, t.oy + 9)],
                        fill=(240, 240, 246, 255))),
        btn(lambda t: t.d.polygon([(t.ox + 8, t.oy + 13), (t.ox + 4, t.oy + 7), (t.ox + 12, t.oy + 7)],
                                  fill=(240, 240, 246, 255))),
        btn(lambda t: t.d.polygon([(t.ox + 3, t.oy + 8), (t.ox + 9, t.oy + 4), (t.ox + 9, t.oy + 12)],
                                  fill=(240, 240, 246, 255))),
        btn(lambda t: t.d.polygon([(t.ox + 13, t.oy + 8), (t.ox + 7, t.oy + 4), (t.ox + 7, t.oy + 12)],
                                  fill=(240, 240, 246, 255))),
        btn(lambda t: (t.rect(5, 5, 10, 10, (240, 240, 246, 255)))),
        btn(lambda t: (t.line(5, 5, 11, 11, (240, 240, 246, 255)),
                       t.line(11, 5, 5, 11, (240, 240, 246, 255)))),
    ]
    return mkpage("mobile", tiles)


def page_armor():
    mats = [("leather", (150, 90, 45, 255), (110, 64, 32, 255)),
            ("iron", (216, 216, 220, 255), (170, 172, 178, 255)),
            ("gold", (250, 210, 70, 255), (200, 160, 46, 255)),
            ("diamond", (120, 230, 220, 255), (90, 190, 190, 255))]
    tiles = [ti_armor_piece(k, base, edge) for _, base, edge in mats[:1]
             for k in ("helmet", "chest", "legs", "boots")]
    tiles += [ti_armor_piece(k, base, edge) for _, base, edge in mats[1:2]
              for k in ("helmet", "chest", "legs", "boots")]
    tiles += [ti_armor_piece(k, mats[2][1], mats[2][2]) for k in ("helmet", "chest")]
    tiles += [ti_armor_piece(k, mats[3][1], mats[3][2]) for k in ("helmet", "chest")]
    return mkpage("armor", tiles)


def page_font():
    glyphs = [" A ", " B ", " C ", " 1 ", " 2 ", " ? ", " ! ", " > ", " < ", " + ", " - ", " = "]
    tiles = []
    for g in glyphs:
        def make(gc=g):
            def d(t):
                t.rect(2, 2, 13, 13, (30, 30, 36, 255))
                outline(t, 2, 2, 13, 13, (90, 90, 100, 255))
                t.d.text((t.ox + 5, t.oy + 3), gc.strip() or " ", fill=(235, 235, 240, 255))
            return d
        tiles.append(make())
    return mkpage("font", tiles)


def page_models():
    def cube(top, side, sh):
        def d(t):
            t.d.polygon([(t.ox + 4, t.oy + 5), (t.ox + 8, t.oy + 3), (t.ox + 12, t.oy + 5),
                         (t.ox + 8, t.oy + 7)], fill=top)
            t.d.polygon([(t.ox + 4, t.oy + 5), (t.ox + 8, t.oy + 7), (t.ox + 8, t.oy + 13),
                         (t.ox + 4, t.oy + 11)], fill=side)
            t.d.polygon([(t.ox + 12, t.oy + 5), (t.ox + 8, t.oy + 7), (t.ox + 8, t.oy + 13),
                         (t.ox + 12, t.oy + 11)], fill=sh)
        return d
    a = PAL
    tiles = [
        cube((120, 170, 80, 255), a["dirt"] + (255,), (110, 76, 48, 255)),
        cube((110, 76, 44, 255), (150, 108, 64, 255), (120, 84, 48, 255)),
        cube((136, 136, 140, 255), (104, 104, 108, 255), (84, 84, 88, 255)),
        cube((214, 234, 240, 230), (200, 224, 232, 200), (180, 204, 214, 200)),
        cube((60, 110, 200, 255), (52, 98, 180, 255), (40, 80, 150, 255)),
        cube((220, 205, 160, 255), (210, 196, 150, 255), (190, 176, 130, 255)),
    ]
    return mkpage("models", tiles)


PAGES = [page_items_0, page_blocks_0, page_blocks_1, page_decor, page_nature,
         page_gui, page_icons, page_particles, page_environment, page_mobs_0,
         page_paintings_0, page_map, page_misc, page_mobile, page_armor,
         page_font, page_models]

if __name__ == "__main__":
    names = [p() for p in PAGES]
    print(f"generated {len(names)} sprite pages -> {OUT}")
