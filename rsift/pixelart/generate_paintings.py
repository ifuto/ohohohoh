#!/usr/bin/env python3
"""RSift 絵画アトラス・ジェネレータ (vanilla 47 paintings, MC best-practice).

仕様:
- 1 ブロック = 16px のネイティブ解像度 (1x1 → 16x16, 4x4 → 64x64)
- モチーフは書き割り (整数ピクセル) で配置、滲み/AA 禁止
- stipple (ハッシュ密度描点) / gridretch (縦横交互色) / filtlumi (16px セル毎の
  輝度クランプ) / threedci (左上=光・右下=影の擬似3Dシェーディング) を全適用
- 出力: pixelart/paintings/<name>.png と 1 枚の paintings_atlas.png (縦断図)
"""
import math
import os
from PIL import Image, ImageDraw

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "paintings")
os.makedirs(OUT, exist_ok=True)

U = 16  # 1 block unit in px


# ---------------------------------------------------------------- primitives

def hx(x, y, s=0):
    """deterministic pixel hash 0..255"""
    v = (x * 374761393 + y * 668265263 + s * 2246822519) & 0xFFFFFFFF
    v = (v ^ (v >> 13)) * 1274126177 & 0xFFFFFFFF
    return (v ^ (v >> 16)) & 255


class Cnv:
    def __init__(self, wu, hu, bg=(12, 10, 14)):
        self.w = wu * U
        self.h = hu * U
        self.img = Image.new("RGB", (self.w, self.h), bg)
        self.d = ImageDraw.Draw(self.img)

    def px(self, x, y, c):
        if 0 <= x < self.w and 0 <= y < self.h:
            self.d.point((x, y), fill=c)

    def rect(self, x0, y0, x1, y1, c):
        self.d.rectangle([x0, y0, x1, y1], fill=c)

    def vgrad(self, top, bot, y0=None, y1=None, x0=0, x1=None, seed=7):
        """dithered vertical gradient"""
        y0 = 0 if y0 is None else y0
        y1 = self.h - 1 if y1 is None else y1
        x1 = self.w - 1 if x1 is None else x1
        span = max(1, y1 - y0)
        for y in range(y0, y1 + 1):
            t = (y - y0) / span
            base = tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3))
            for x in range(x0, x1 + 1):
                n = (hx(x, y, seed) - 128) >> 4  # subtle dither
                self.px(x, y, tuple(min(255, max(0, base[i] + n)) for i in range(3)))

    def stipple(self, x0, y0, x1, y1, c, density=128, seed=0):
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                if hx(x, y, seed) < density:
                    self.px(x, y, c)

    def gridretch(self, x0, y0, x1, y1, c1, c2, vertical=True):
        """grid+stretch: alternating column/row pairs between two colors"""
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                k = x if vertical else y
                self.px(x, y, c1 if (k >> 1) % 2 == 0 else c2)

    def blob(self, cx, cy, rx, ry, c, wobble=0.0, seed=3):
        """filled noisy ellipse (写割 silhouette)"""
        for y in range(int(cy - ry) - 2, int(cy + ry) + 3):
            for x in range(int(cx - rx) - 2, int(cx + rx) + 3):
                d = ((x - cx) / max(rx, 0.5)) ** 2 + ((y - cy) / max(ry, 0.5)) ** 2
                d += wobble * (hx(x, y, seed) - 128) / 128.0
                if d <= 1.0:
                    self.px(x, y, c)

    def ring(self, cx, cy, r, c, thick=1):
        for a in range(360):
            for t in range(thick):
                rad = math.radians(a)
                x = int(cx + (r - t) * math.cos(rad))
                y = int(cy + (r - t) * math.sin(rad))
                self.px(x, y, c)

    def rays(self, cx, cy, n, r0, r1, c, width=2, phase=0.0):
        """radial light rays"""
        for i in range(n):
            a = phase + 2 * math.pi * i / n
            for rr in range(r0, r1):
                for w in range(-(width // 2), width // 2 + 1):
                    aa = a + w / max(rr, 1)
                    x = int(cx + rr * math.cos(aa))
                    y = int(cy + rr * math.sin(aa))
                    self.px(x, y, c)

    def line(self, x0, y0, x1, y1, c):
        self.d.line([x0, y0, x1, y1], fill=c)

    def edge3d(self, x0, y0, x1, y1, hi, lo, depth=1):
        """threedci: top/left highlight, bottom/right shadow"""
        for t in range(depth):
            for x in range(x0, x1 + 1):
                self.px(x, y0 + t, hi)
                self.px(x, y1 - t, lo)
            for y in range(y0, y1 + 1):
                self.px(x0 + t, y, hi)
                self.px(x1 - t, y, lo)

    def filtlumi(self, seed=11):
        """filtlumi: per-16px-cell symmetric luminance clamp (+/-10 around cell avg)"""
        for cy in range(0, self.h, U):
            for cx in range(0, self.w, U):
                px = self.img.load()
                lum = []
                for y in range(cy, min(cy + U, self.h)):
                    for x in range(cx, min(cx + U, self.w)):
                        r, g, b = px[x, y]
                        lum.append((r * 299 + g * 587 + b * 114) // 1000)
                if not lum:
                    continue
                avg = sum(lum) // len(lum)
                for y in range(cy, min(cy + U, self.h)):
                    for x in range(cx, min(cx + U, self.w)):
                        r, g, b = px[x, y]
                        l = (r * 299 + g * 587 + b * 114) // 1000
                        dl = l - avg
                        cap = 14 if abs(dl) <= 28 else 8
                        if dl > cap or dl < -cap:
                            k = (l - (dl - max(-cap, min(cap, dl)))) / max(l, 1e-6)
                            px[x, y] = (min(255, int(r * k)), min(255, int(g * k)),
                                        min(255, int(b * k)))

    def save(self, name):
        self.filtlumi()
        self.img.save(os.path.join(OUT, name + ".png"))
        return name


# ------------------------------------------------------------- classic 26

def p_kebab():
    c = Cnv(1, 1, (168, 60, 48))
    c.vgrad((176, 62, 44), (120, 40, 34))
    # skewer: golden kebab on white plate
    c.rect(2, 11, 13, 13, (214, 210, 200))           # plate
    for i, y in enumerate((5, 7, 9)):
        c.blob(7, y, 3 - i * 0.4, 1.4, (188, 132, 60))  # meat slices
    for x in (5, 8, 11):
        c.rect(x, 3, x + 1, 4, (88, 168, 48))       # 3 green peppers
    c.line(8, 2, 8, 12, (120, 92, 48))              # stick
    c.edge3d(2, 11, 13, 13, (238, 234, 228), (150, 144, 132))
    return c.save("kebab")


def p_aztec():
    c = Cnv(1, 1, (74, 98, 60))
    for y in range(16):
        for x in range(16):                          # top-down map + path
            base = (74, 98, 60) if hx(x, y) > 60 else (60, 84, 50)
            c.px(x, y, base)
    c.rect(2, 2, 13, 13, (178, 168, 128))            # temple square
    c.rect(4, 4, 11, 11, (74, 98, 60))
    c.rect(7, 2, 8, 13, (178, 168, 128))             # main path
    c.stipple(4, 4, 11, 11, (128, 190, 60), 90)      # green moss face
    c.rect(7, 7, 9, 9, (128, 190, 60))
    return c.save("aztec")


def p_aztec2():
    c = Cnv(1, 1, (120, 90, 62))
    c.vgrad((132, 96, 64), (92, 64, 44))
    c.rect(3, 3, 12, 12, (60, 110, 70))              # inner map
    c.rect(5, 5, 10, 10, (168, 150, 110))
    c.blob(8, 8, 2, 2, (128, 190, 60))               # face
    c.px(7, 7, (30, 40, 30)); c.px(9, 7, (30, 40, 30))
    c.stipple(3, 3, 12, 12, (128, 190, 60), 40, 9)
    return c.save("aztec2")


def p_alban():
    c = Cnv(1, 1, (140, 170, 190))
    c.vgrad((118, 158, 200), (90, 110, 120), 0, 9)   # sky
    c.vgrad((76, 66, 52), (54, 46, 38), 10, 15)      # ground
    c.rect(0, 10, 15, 15, (70, 96, 54))              # field
    c.blob(8, 11, 2.4, 3.4, (196, 60, 44))           # red jacket
    c.rect(7, 4, 9, 7, (234, 224, 216))              # pointy white hat
    c.px(8, 3, (234, 224, 216))
    c.rect(7, 8, 9, 9, (226, 190, 160))              # face
    c.edge3d(7, 4, 9, 11, (255, 250, 244), (140, 40, 32))
    return c.save("alban")


def p_bomb():
    c = Cnv(1, 1, (36, 48, 72))
    for y in range(16):                              # target wheel
        for x in range(16):
            d = math.hypot(x - 7.5, y - 7.5)
            ring = int(d) % 4
            col = [(200, 60, 40), (230, 220, 200), (40, 60, 110), (230, 220, 200)][ring]
            if d > 8:
                col = (24, 30, 44)
            c.px(x, y, col)
    c.rect(6, 6, 9, 9, (255, 210, 40))               # explosion core
    c.rays(7.5, 7.5, 8, 3, 7, (255, 160, 40), 1, 0.3)
    return c.save("bomb")


def p_plant():
    c = Cnv(1, 1, (166, 196, 214))
    c.vgrad((176, 206, 224), (140, 168, 186))
    c.rect(4, 11, 11, 14, (150, 84, 50))             # pot
    c.edge3d(4, 11, 11, 14, (180, 104, 62), (110, 60, 36))
    for i in range(7):                               # leafy top
        a = -0.4 + i * 0.42
        x = 7 + int(4.5 * math.sin(a))
        y = 6 + int(4 * math.cos(a * 1.7))
        c.blob(x, y, 1.8, 1.8, (56 + (i % 3) * 18, 130, 44))
    c.line(8, 7, 8, 11, (66, 100, 40))
    return c.save("plant")


def p_wasteland():
    c = Cnv(1, 1, (188, 170, 120))
    c.rect(1, 1, 14, 14, (120, 110, 90))             # window frame
    c.rect(2, 2, 13, 13, (210, 190, 130))            # desert sky
    c.vgrad((214, 196, 138), (188, 156, 100), 2, 13, 2, 13)
    c.rect(2, 10, 13, 13, (172, 132, 74))            # dunes
    c.blob(6, 8, 3, 2, (200, 200, 196))              # rabbit body
    c.blob(9, 7, 1.3, 1.3, (214, 214, 208))          # head
    c.rect(9, 3, 10, 5, (214, 214, 208))             # ears
    c.px(9, 7, (40, 40, 40))
    return c.save("wasteland")


def p_pool():
    c = Cnv(2, 1, (96, 148, 190))
    c.vgrad((120, 170, 206), (70, 110, 150))
    # inflatable pool with two people
    c.ring(16, 9, 10, (230, 70, 60), 3)              # red pool ring
    c.ring(16, 9, 7, (70, 140, 210), 2)
    c.blob(11, 8, 2, 2.6, (228, 190, 160))           # person A
    c.blob(21, 8, 2, 2.6, (160, 110, 76))            # person B
    c.blob(11, 5.4, 1.2, 1.2, (60, 40, 30))
    c.blob(21, 5.4, 1.2, 1.2, (30, 20, 16))
    c.stipple(2, 2, 29, 13, (190, 230, 250), 26, 5)  # sparkles
    return c.save("pool")


def p_courbet():
    c = Cnv(2, 1, (70, 96, 130))
    c.vgrad((60, 88, 120), (38, 60, 86))
    c.blob(16, 9, 8, 4.6, (90, 66, 50), 0.15)        # hillside (Courbet color)
    c.blob(16, 10, 9, 3.4, (70, 50, 38), 0.12, 8)
    c.stipple(6, 4, 26, 12, (120, 150, 120), 42, 1)  # trees silhouette
    c.rect(15, 7, 17, 9, (230, 224, 210))            # small house
    return c.save("courbet")


def p_sea():
    c = Cnv(2, 1, (170, 200, 214))
    c.vgrad((196, 214, 224), (150, 178, 198), 0, 6)  # sky
    c.gridretch(0, 7, 31, 15, (58, 110, 150), (46, 94, 136), False)  # sea layers
    for k in range(3):                               # wave crests
        y = 8 + k * 3
        for x in range(0, 32, 4):
            c.px(x + k, y, (210, 230, 240))
    c.blob(8, 12, 3, 1.6, (120, 90, 52))             # head in water
    return c.save("sea")


def p_sunset():
    c = Cnv(2, 1, (240, 150, 60))
    c.vgrad((250, 178, 70), (200, 84, 44), 0, 10)
    c.vgrad((120, 50, 40), (80, 30, 34), 11, 15)
    c.blob(22, 9, 4, 4, (255, 236, 140))             # sun
    c.rays(22, 9, 6, 5, 9, (255, 210, 110), 1, 0.5)
    c.rect(0, 11, 8, 13, (60, 44, 38))               # mountain silhouette
    c.rect(2, 10, 6, 13, (60, 44, 38))
    return c.save("sunset")


def p_creebet():
    c = Cnv(2, 1, (150, 180, 200))
    c.vgrad((160, 190, 208), (110, 146, 170), 0, 8)
    c.gridretch(0, 9, 31, 15, (66, 118, 152), (54, 102, 136), False)
    for k in range(2):
        for x in range(0, 32, 4):
            c.px(x + k, 10 + k * 2, (214, 234, 244))  # foam
    c.blob(24, 12, 3, 2, (110, 160, 90))             # creeper peeking
    c.rect(22, 10, 23, 11, (16, 24, 12))             # eyes
    c.rect(25, 10, 26, 11, (16, 24, 12))
    c.rect(23, 12, 25, 14, (16, 24, 12))             # mouth
    return c.save("creebet")


def p_wanderer():
    c = Cnv(1, 2, (110, 130, 120))
    c.vgrad((104, 128, 118), (70, 88, 80), 0, 20)
    c.vgrad((78, 70, 54), (56, 50, 40), 21, 31)      # misty ground (Caspar David)
    c.stipple(0, 12, 15, 24, (200, 204, 200), 60, 4) # fog
    c.rect(7, 14, 8, 24, (32, 30, 36))               # wanderer (back view)
    c.blob(7.5, 12.6, 1.4, 1.4, (60, 50, 40))        # head/hat
    c.line(9, 16, 12, 23, (40, 30, 24))              # cane
    c.rect(0, 22, 5, 31, (52, 58, 44))               # rock
    c.rect(11, 24, 15, 31, (60, 66, 50))
    return c.save("wanderer")


def p_graham():
    c = Cnv(1, 2, (206, 190, 160))
    c.vgrad((214, 198, 168), (178, 160, 128))
    c.blob(8, 10, 4.4, 4.8, (212, 182, 150), 0.06)   # graham creature
    c.blob(8, 5.4, 3, 3.2, (220, 192, 160))
    c.rect(5, 4, 6, 5, (255, 255, 255)); c.px(6, 5, (26, 26, 26))
    c.rect(10, 4, 11, 5, (255, 255, 255)); c.px(10, 5, (26, 26, 26))
    c.rect(6, 8, 10, 8, (140, 92, 70))               # mouth
    c.rect(3, 12, 4, 17, (212, 182, 150))            # arms
    c.rect(12, 12, 13, 17, (212, 182, 150))
    c.stipple(2, 2, 13, 29, (236, 226, 200), 24, 6)
    return c.save("graham")


def p_match():
    c = Cnv(2, 2, (30, 28, 34))
    c.rect(4, 22, 27, 29, (44, 40, 46))              # table
    # two hands holding a match
    c.rect(9, 14, 14, 21, (216, 178, 140))
    c.rect(17, 14, 22, 21, (216, 178, 140))
    c.line(14, 18, 18, 18, (124, 84, 40))            # matchstick
    c.blob(16, 15, 1.6, 2.4, (255, 220, 90))         # flame
    c.blob(16, 14, 0.9, 1.2, (255, 245, 200))
    c.rays(16, 15, 10, 4, 12, (120, 84, 40), 1, 0.2) # glow in dark
    return c.save("match")


def p_bust():
    c = Cnv(2, 2, (120, 96, 70))
    c.vgrad((132, 106, 76), (92, 70, 50))
    c.rect(10, 22, 21, 27, (164, 150, 130))          # column base
    c.edge3d(10, 22, 21, 27, (196, 184, 164), (110, 98, 82))
    c.blob(16, 17, 6.5, 5, (196, 188, 176))          # shoulders (bust)
    c.blob(16, 9, 4.6, 5.2, (210, 202, 190))         # head
    c.rect(13, 8, 14, 9, (60, 56, 52))               # eyes (carved)
    c.rect(18, 8, 19, 9, (60, 56, 52))
    c.rect(15, 12, 17, 13, (150, 140, 128))          # mouth
    c.stipple(3, 3, 28, 20, (150, 120, 90), 40, 2)
    return c.save("bust")


def p_stage():
    c = Cnv(2, 2, (18, 16, 22))
    c.rect(0, 24, 31, 31, (52, 34, 24))              # stage floor
    c.rect(0, 0, 3, 31, (120, 24, 24))               # curtain L
    c.rect(28, 0, 31, 31, (120, 24, 24))             # curtain R
    c.gridretch(0, 0, 3, 31, (120, 24, 24), (96, 18, 18))
    c.gridretch(28, 0, 31, 31, (120, 24, 24), (96, 18, 18))
    c.rect(14, 10, 17, 23, (196, 170, 140))          # actor (spider-king pose)
    c.blob(15.5, 8, 2, 2.2, (210, 180, 150))
    for dx in (-5, -3, 3, 5):                        # spider legs
        c.line(15, 14, 15 + dx, 22, (40, 32, 28))
    c.rays(16, 4, 1, 0, 0, (0, 0, 0))  # noop
    c.stipple(10, 2, 22, 6, (255, 240, 180), 70, 3)  # spotlight
    return c.save("stage")


def p_void():
    c = Cnv(2, 2, (10, 8, 16))
    # swirling void portal
    for y in range(32):
        for x in range(32):
            d = math.hypot(x - 15.5, y - 15.5)
            a = math.atan2(y - 15.5, x - 15.5)
            sw = (d * 6 + a * 5) % 22
            if d < 12:
                col = (10, 8, 16) if sw > 11 else (58, 26, 96)
            elif d < 15:
                col = (120, 70, 180)
            else:
                col = (16, 12, 26) if hx(x, y) > 40 else (28, 20, 44)
            c.px(x, y, col)
    c.stipple(0, 0, 31, 31, (220, 220, 255), 14, 9)  # stars
    return c.save("void")


def p_skull_and_roses():
    c = Cnv(2, 2, (96, 30, 36))
    c.vgrad((110, 36, 42), (70, 20, 26))
    c.blob(16, 13, 6, 5.6, (226, 220, 206))          # skull
    c.rect(12, 11, 14, 13, (24, 18, 20))             # eyes
    c.rect(18, 11, 20, 13, (24, 18, 20))
    c.rect(15, 15, 17, 18, (226, 220, 206))          # jaw
    for x in (15, 16, 17):
        c.line(x, 15, x, 18, (160, 150, 136))
    for i in range(5):                               # roses
        a = i * 1.256
        x = 16 + int(11 * math.cos(a))
        y = 24 + int(4 * math.sin(a))
        c.blob(x, y, 2, 2, (190, 40, 40))
        c.px(x, y, (240, 90, 80))
        c.line(x, y + 2, x, y + 4, (60, 110, 50))
    return c.save("skull_and_roses")


def p_wither():
    c = Cnv(2, 2, (16, 14, 18))
    for y in range(32):                              # dark soul-sand field
        for x in range(32):
            c.px(x, y, (16, 14, 18) if hx(x, y) > 90 else (24, 20, 26))
    for dx, dy in ((16, 11), (10, 14), (22, 14)):    # 3 skulls
        c.blob(dx, dy, 3.4, 3.6, (40, 36, 44))
        c.rect(dx - 2, dy - 1, dx - 1, dy + 1, (10, 10, 12))
        c.rect(dx + 1, dy - 1, dx + 2, dy + 1, (10, 10, 12))
        c.rect(dx - 1, dy + 2, dx + 1, dy + 3, (10, 10, 12))
    c.rect(14, 19, 17, 28, (36, 32, 40))             # spine body
    c.rect(10, 19, 13, 22, (36, 32, 40))
    c.rect(19, 19, 22, 22, (36, 32, 40))
    c.stipple(6, 6, 26, 26, (150, 90, 190), 50, 7)   # wither particles
    return c.save("wither")


def p_fighters():
    c = Cnv(4, 2, (120, 140, 120))
    c.vgrad((128, 148, 128), (88, 106, 92))
    c.rect(0, 26, 63, 31, (96, 84, 64))              # dojo floor
    for i in range(2, 60, 8):                        # crowd silhouettes
        c.blob(i, 4 + (i % 3), 2, 2.6, (52, 56, 48))
    # kens: two martial artists
    c.rect(14, 12, 18, 24, (230, 220, 200)); c.blob(16, 10, 2, 2, (220, 180, 140))
    c.line(18, 16, 26, 14, (230, 220, 200))          # punch > hadouken arm
    c.rect(44, 12, 48, 24, (220, 60, 50)); c.blob(46, 10, 2, 2, (220, 180, 140))
    c.line(44, 16, 36, 14, (220, 60, 50))
    c.blob(30, 15, 2.6, 2.6, (120, 200, 255))        # hadouken!
    c.rays(30, 15, 6, 3, 6, (200, 240, 255), 1, 0.4)
    return c.save("fighters")


def p_pointer():
    c = Cnv(4, 4, (96, 22, 20))
    # red monochrome club scene
    for y in range(64):
        for x in range(64):
            n = (hx(x, y) - 128) >> 5
            c.px(x, y, (96 + n, 20, 18))
    c.vgrad((120, 30, 26), (60, 12, 12), 0, 63)
    for i, xx in enumerate(range(8, 58, 10)):        # crowd silhouettes
        h = 18 + (i % 3) * 5
        c.rect(xx, 63 - h, xx + 5, 63, (30, 8, 8))
        c.blob(xx + 2, 62 - h, 2.4, 2.6, (40, 10, 10))
    cx, cy = 20, 30
    c.rect(cx - 4, cy, cx + 4, 54, (210, 60, 50))    # main figure
    c.blob(cx, cy - 4, 3.4, 3.6, (220, 170, 130))
    c.line(cx + 2, cy + 6, cx + 18, cy - 10, (210, 60, 50))  # pointing arm
    c.blob(cx + 19, cy - 11, 1.6, 1.6, (220, 170, 130))
    c.edge3d(cx - 4, cy - 1, cx + 4, 54, (240, 120, 100), (120, 30, 26))
    return c.save("pointer")


def p_pigscene():
    c = Cnv(4, 4, (140, 170, 190))
    c.vgrad((170, 190, 205), (120, 140, 160), 0, 34)  # sky window
    c.rect(0, 35, 63, 63, (110, 84, 60))              # gallery floor
    c.gridretch(0, 35, 63, 63, (110, 84, 60), (96, 72, 52), True)
    c.rect(10, 8, 54, 38, (226, 222, 206))            # big canvas: the pig painting
    c.edge3d(10, 8, 54, 38, (255, 252, 240), (150, 146, 130), 2)
    c.rect(13, 11, 51, 35, (150, 190, 220))           # sky in painting
    c.rect(13, 26, 51, 35, (90, 140, 70))             # grass in painting
    c.blob(26, 24, 7, 4.4, (238, 150, 150))           # pig body
    c.blob(32, 22, 3.6, 3.6, (242, 158, 158))
    c.rect(33, 22, 35, 24, (200, 90, 90))             # snout
    c.px(32, 21, (40, 40, 40))
    for lx in (21, 24, 28, 31):
        c.rect(lx, 27, lx + 1, 31, (228, 138, 138))
    c.stipple(13, 12, 51, 25, (255, 255, 255), 18, 3) # clouds
    c.rect(44, 40, 47, 63, (70, 54, 40))              # viewer silhouette
    c.blob(45, 37, 2, 2.2, (50, 40, 32))
    return c.save("pigscene")


def p_burning_skull():
    c = Cnv(4, 4, (90, 30, 8))
    c.vgrad((120, 40, 10), (60, 16, 6))
    for i in range(14):                               # flames ring
        a = i * 0.448
        x = 32 + int(22 * math.cos(a))
        y = 34 + int(14 * math.sin(a))
        for s in range(3):
            c.blob(x, y - s * 3, 2.4 - s * 0.6, 2.4 - s * 0.6,
                   (255, 210 - s * 70, 30 + s * 20))
    c.blob(32, 32, 12, 13, (228, 222, 208))           # skull
    c.rect(25, 28, 29, 32, (14, 10, 10))
    c.rect(35, 28, 39, 32, (14, 10, 10))
    c.rect(30, 35, 34, 40, (200, 196, 184))           # teeth area
    for x in range(30, 35):
        c.line(x, 36, x, 40, (140, 134, 120))
    c.px(32, 33, (14, 10, 10))                        # nose hole
    c.rays(32, 32, 12, 14, 20, (255, 160, 40), 1, 0.26)
    return c.save("burning_skull")


def p_skeleton():
    c = Cnv(4, 3, (60, 56, 70))
    c.vgrad((70, 66, 82), (40, 36, 50))
    c.rect(0, 38, 63, 47, (48, 44, 52))               # ground
    c.stipple(0, 0, 63, 47, (120, 116, 130), 20, 5)   # cold night
    # skeleton seated with dog (mean midget)
    x0, y0 = 40, 14
    c.blob(x0, y0, 4, 4.6, (226, 226, 220))           # skull
    c.rect(x0 - 2, y0 - 1, x0 - 1, y0 + 1, (30, 30, 34))
    c.rect(x0 + 1, y0 - 1, x0 + 2, y0 + 1, (30, 30, 34))
    for r in range(5):                                # ribs
        c.rect(x0 - 4, y0 + 5 + r * 2, x0 + 4, y0 + 5 + r * 2, (216, 216, 210))
    c.rect(x0 - 1, y0 + 5, x0, y0 + 18, (216, 216, 210))
    c.line(x0 - 4, y0 + 7, x0 - 12, y0 + 16, (216, 216, 210))  # arms
    c.line(x0 + 4, y0 + 7, x0 + 12, y0 + 16, (216, 216, 210))
    c.line(x0 - 2, y0 + 18, x0 - 8, y0 + 30, (216, 216, 210))  # legs
    c.line(x0 + 2, y0 + 18, x0 + 8, y0 + 30, (216, 216, 210))
    # loyal dog
    c.blob(20, 36, 6, 3, (150, 120, 88))
    c.blob(26, 33, 2.6, 2.6, (160, 130, 96))
    c.rect(25, 30, 26, 31, (130, 100, 74))
    c.line(14, 36, 11, 33, (140, 110, 80))
    c.px(26, 32, (30, 24, 20))
    return c.save("skeleton")


def p_donkey_kong():
    c = Cnv(4, 3, (24, 24, 30))
    c.vgrad((30, 30, 38), (18, 18, 26))
    for gy in (8, 20, 32):                            # girders
        c.rect(0, gy, 63, gy + 2, (180, 60, 60))
        c.gridretch(0, gy, 63, gy + 2, (180, 60, 60), (150, 46, 46))
    for i in (12, 36, 52):                            # ladders
        c.rect(i, 2, i + 2, 34, (200, 170, 90))
    c.blob(48, 6, 6, 5, (110, 74, 44))                # DK body
    c.blob(48, 1.4, 3.6, 3.2, (120, 82, 50))          # head
    c.rect(45, 0, 51, 2, (60, 38, 22))
    c.rect(8, 24, 11, 32, (60, 110, 220))             # jumpman (overalls)
    c.blob(9, 22, 1.6, 1.6, (240, 190, 150))
    c.rect(8, 21, 11, 22, (220, 40, 40))              # red cap
    c.blob(27, 27, 2.4, 3, (240, 180, 60))            # barrel
    return c.save("donkey_kong")


# ------------------------------------------------------------- 1.21 new (5)

def p_meditative():
    c = Cnv(1, 1, (192, 170, 140))
    c.vgrad((202, 182, 152), (160, 136, 106))
    c.blob(8, 11, 4, 3.2, (150, 110, 70))             # seated lotus silhouette
    c.blob(8, 6.6, 1.6, 1.8, (170, 130, 90))
    c.ring(8, 6.6, 4, (230, 210, 150), 1)             # halo
    c.stipple(2, 2, 13, 13, (240, 226, 190), 26, 8)
    return c.save("meditative")


def p_prairie_ride():
    c = Cnv(1, 2, (150, 180, 210))
    c.vgrad((170, 196, 222), (120, 150, 190), 0, 14)
    c.vgrad((196, 178, 96), (160, 140, 70), 15, 31)   # golden prairie
    c.stipple(0, 16, 15, 31, (220, 200, 110), 90, 2)  # wheat
    c.rect(6, 18, 10, 27, (90, 60, 40))               # horse body
    c.rect(9, 14, 11, 21, (100, 68, 46))              # neck+head
    for lx in (6, 9):
        c.rect(lx, 27, lx, 31, (80, 52, 34))
    c.rect(7, 12, 8, 18, (150, 40, 40))               # rider
    c.blob(7.5, 10.6, 1, 1.1, (230, 190, 150))
    return c.save("prairie_ride")


def p_baroque():
    c = Cnv(2, 2, (60, 44, 70))
    c.vgrad((70, 52, 80), (44, 32, 56))
    # ornate golden frame in frame
    c.edge3d(4, 4, 27, 27, (220, 190, 90), (120, 96, 40), 2)
    c.rect(6, 6, 25, 25, (90, 120, 150))
    c.vgrad((100, 130, 160), (70, 96, 130), 6, 25, 6, 25)
    c.blob(16, 18, 6, 5, (60, 80, 60), 0.1)           # still-life fruit pile
    c.blob(13, 15, 2, 2, (220, 90, 60))
    c.blob(17, 14, 2, 2, (240, 170, 50))
    c.blob(20, 16, 2, 2, (160, 190, 60))
    c.stipple(6, 6, 25, 25, (255, 244, 214), 30, 4)
    return c.save("baroque")


def p_humble():
    c = Cnv(2, 2, (120, 100, 78))
    c.vgrad((132, 112, 88), (98, 80, 62))
    c.rect(9, 16, 23, 27, (140, 110, 80))             # humble house
    c.rect(8, 13, 24, 16, (110, 70, 44))              # roof
    c.rect(14, 21, 17, 27, (80, 56, 38))              # door
    c.rect(11, 18, 13, 20, (250, 220, 120))           # lit window
    c.rect(20, 18, 22, 20, (250, 220, 120))
    c.stipple(2, 26, 29, 31, (90, 130, 60), 100, 5)   # grass line
    return c.save("humble")


def p_unpacked():
    c = Cnv(4, 4, (150, 140, 120))
    c.vgrad((166, 154, 132), (120, 108, 88))
    c.rect(0, 44, 63, 63, (104, 84, 64))              # floor
    c.gridretch(0, 44, 63, 63, (104, 84, 64), (92, 74, 56), True)
    # unzipped world: giant open box with landscape inside
    c.rect(16, 14, 48, 46, (150, 110, 70))            # box outer
    c.edge3d(16, 14, 48, 46, (186, 140, 90), (104, 74, 46), 2)
    c.rect(20, 18, 44, 42, (140, 190, 230))           # sky inside box
    c.blob(28, 24, 3, 1.6, (245, 245, 250))
    c.blob(37, 22, 2.4, 1.4, (245, 245, 250))
    c.rect(20, 34, 44, 42, (90, 150, 70))             # grass inside
    c.blob(40, 33, 2, 2.4, (238, 150, 150))           # pig inside
    c.rect(10, 8, 24, 13, (170, 130, 84))             # flaps open
    c.rect(40, 8, 54, 13, (170, 130, 84))
    c.rect(10, 14, 15, 24, (160, 120, 76))
    c.rect(49, 14, 54, 24, (160, 120, 76))
    return c.save("unpacked")


# ------------------------------------------------------------- 1.21.4+ (15) + dennis

def p_backyard():
    c = Cnv(3, 4, (110, 150, 190))
    c.vgrad((140, 176, 208), (90, 126, 166), 0, 30)
    c.rect(0, 31, 47, 63, (96, 140, 66))              # lawn
    c.stipple(0, 31, 47, 63, (120, 168, 80), 110, 3)
    c.rect(4, 12, 20, 30, (150, 100, 66))             # fence-ish shed
    c.edge3d(4, 12, 20, 30, (176, 122, 80), (110, 72, 46))
    c.blob(36, 14, 5, 8, (60, 100, 50))               # tree
    c.rect(35, 22, 37, 40, (110, 76, 48))
    c.blob(25, 44, 6, 3, (200, 60, 50))               # dog house
    c.blob(40, 46, 3, 2.4, (180, 150, 110))           # own dog
    c.px(40, 45, (40, 30, 24))
    return c.save("backyard")


def p_bouquet():
    c = Cnv(1, 1, (200, 190, 200))
    c.vgrad((212, 204, 212), (170, 160, 172))
    c.rect(6, 10, 10, 15, (130, 90, 60))              # vase wrap
    cols = [(230, 80, 80), (240, 180, 60), (200, 100, 220), (244, 244, 244), (240, 120, 150)]
    for i, col in enumerate(cols):
        a = -0.5 + i * 0.5
        x = 8 + int(4.5 * math.sin(a))
        y = 5 + int(2.5 * math.cos(a * 2))
        c.blob(x, y, 1.5, 1.5, col)
        c.line(8, 10, x, y + 1, (70, 120, 56))
    return c.save("bouquet")


def p_cavebird():
    c = Cnv(2, 1, (40, 40, 52))
    c.vgrad((48, 48, 62), (28, 28, 40))
    c.stipple(0, 0, 31, 15, (90, 90, 110), 60, 6)     # cave stone specks
    c.stipple(0, 0, 31, 15, (120, 200, 220), 10, 1)   # glow lichen
    c.blob(16, 8, 3, 3.4, (210, 220, 230))            # cave bird
    c.blob(18, 5, 1.4, 1.4, (222, 232, 240))
    c.px(19, 5, (20, 20, 26))
    c.line(15, 11, 15, 15, (140, 150, 160))
    c.line(17, 11, 17, 15, (140, 150, 160))
    c.blob(9, 6, 1.6, 1.2, (140, 220, 240))           # second bird far
    return c.save("cavebird")


def p_changing():
    c = Cnv(4, 2, (170, 150, 120))
    # seasons strip: 4 panels changing
    panels = [((190, 220, 240), (240, 245, 250)),   # winter
              ((150, 210, 140), (240, 160, 180)),   # spring
              ((110, 190, 120), (90, 150, 220)),    # summer
              ((200, 130, 60), (150, 90, 50))]      # autumn
    for pi in range(4):
        x0 = pi * 16
        top, leaf = panels[pi]
        c.vgrad(top, [max(0, v - 40) for v in top], 0, 20, x0, x0 + 15)
        c.rect(x0, 21, x0 + 15, 31, (110, 86, 60))
        c.rect(x0 + 6, 8, x0 + 8, 24, (110, 76, 48))
        c.blob(x0 + 7, 6, 4.4, 3.4, leaf)
        c.stipple(x0, 2, x0 + 15, 19, leaf, 30, pi)
    return c.save("changing")


def p_cotan():
    c = Cnv(2, 2, (24, 20, 18))
    # bodegón: hanging produce, Cotán style (black bg)
    for i, (x, col) in enumerate(((8, (220, 200, 60)), (15, (200, 90, 60)),
                                  (22, (90, 140, 70)), (26, (230, 230, 220)))):
        c.line(x, 0, x, 8 + i * 4, (140, 130, 120))
        c.blob(x, 12 + i * 4, 2.6, 3.2, col)
        c.edge3d(x - 3, 9 + i * 4, x + 3, 15 + i * 4, (255, 255, 240), (20, 16, 14))
    c.stipple(2, 26, 29, 31, (70, 60, 50), 40, 7)
    return c.save("cotan")


def p_endboss():
    c = Cnv(3, 3, (12, 8, 20))
    c.vgrad((20, 14, 30), (10, 6, 16))
    c.stipple(0, 0, 47, 47, (200, 180, 255), 20, 3)   # end stars
    c.blob(24, 20, 12, 8, (16, 12, 26))               # dragon silhouette
    c.blob(36, 14, 3, 3, (20, 16, 30))                # head
    c.rect(37, 13, 38, 14, (230, 120, 255))           # eyes glow
    for i in range(2):                                # wings
        c.line(16, 18, 6 + i * 4, 8, (26, 20, 40))
        c.line(32, 18, 44 - i * 4, 8, (26, 20, 40))
    c.rays(38, 14, 4, 4, 8, (160, 60, 220), 1, 0.9)
    c.rect(20, 40, 28, 42, (60, 50, 90))              # obsidian spikes
    c.rect(22, 36, 26, 42, (46, 38, 70))
    return c.save("endboss")


def p_fern():
    c = Cnv(3, 3, (30, 60, 40))
    c.vgrad((36, 70, 46), (22, 44, 30))
    for fr in range(7):                               # fern fronds
        a = -0.6 + fr * 0.5
        x0, y0 = 24, 44
        for s in range(16):
            x = x0 + int(s * 1.3 * math.cos(a + s * 0.02))
            y = y0 - int(s * 2.2)
            w = max(0, 3 - s // 5)
            c.rect(x - w, y, x + w, y, (90 + s * 6, 170, 90 - s * 3))
    c.stipple(2, 2, 45, 20, (160, 220, 160), 16, 9)
    return c.save("fern")


def p_finding():
    c = Cnv(4, 2, (190, 180, 150))
    c.vgrad((200, 190, 160), (150, 138, 110))
    c.rect(10, 10, 30, 22, (170, 140, 96))            # map paper
    c.edge3d(10, 10, 30, 22, (200, 172, 126), (120, 96, 66))
    c.rect(13, 13, 27, 19, (80, 140, 190))            # ocean on map
    c.blob(17, 16, 2.4, 2, (90, 150, 80))             # island
    c.rect(24, 14, 26, 16, (220, 60, 50))             # X mark
    c.blob(44, 12, 4, 4, (250, 220, 130))             # magnifier glimpse light
    return c.save("finding")


def p_lowmist():
    c = Cnv(4, 4, (140, 150, 150))
    c.vgrad((150, 160, 160), (110, 118, 118), 0, 40)
    for layer in range(4):                            # receding misty ridges
        y0 = 18 + layer * 9
        tone = 120 - layer * 18
        for x in range(64):
            h = int(5 * math.sin(x * 0.22 + layer * 1.7) + 3 * math.sin(x * 0.71 + layer))
            top = min(y0 + h, 62 - layer * 4)
            c.rect(x, top, x, 63 - layer * 4, (tone - 20, tone - 12, tone - 10))
        c.stipple(0, y0 - 6, 63, y0 + 2, (200, 206, 204), 50, layer)  # mist band
    return c.save("lowmist")


def p_orb():
    c = Cnv(4, 4, (18, 14, 30))
    c.vgrad((26, 20, 40), (10, 8, 18))
    c.stipple(0, 0, 63, 63, (220, 220, 255), 14, 5)
    c.blob(32, 30, 13, 13, (90, 130, 220))            # the orb (planet)
    c.blob(28, 26, 7, 6, (130, 170, 240))             # lit quadrant
    c.blob(37, 35, 4, 3, (60, 90, 170))               # maria
    c.ring(32, 30, 18, (200, 190, 160), 1)            # ring
    for t in range(20):                               # ring trailing arc
        a = 0.5 + t * 0.31
        x = int(32 + 18 * math.cos(a)); y = int(30 + 6 * math.sin(a))
        c.px(x, y, (220, 210, 180))
    c.rays(24, 22, 3, 16, 24, (60, 70, 120), 1, 0.2)
    return c.save("orb")


def p_owlemons():
    c = Cnv(3, 3, (250, 240, 120))
    c.vgrad((252, 244, 140), (236, 214, 80))
    c.blob(24, 22, 15, 15, (250, 214, 60))            # giant lemon
    c.ring(24, 22, 12, (240, 200, 50), 2)
    # owl in the lemon
    c.blob(24, 20, 7, 7.4, (150, 110, 70))
    c.ring(21, 18, 3, (250, 250, 250), 2)
    c.ring(27, 18, 3, (250, 250, 250), 2)
    c.px(21, 18, (30, 24, 20)); c.px(27, 18, (30, 24, 20))
    c.rect(23, 22, 24, 23, (240, 170, 40))            # beak
    c.line(16, 30, 32, 30, (110, 80, 50))             # branch
    return c.save("owlemons")


def p_passage():
    c = Cnv(4, 2, (60, 66, 80))
    c.vgrad((70, 76, 92), (40, 44, 58))
    # corridor passage with light at the end
    for d in range(6):
        x0, y0 = 2 + d * 5, 2 + d * 2
        x1, y1 = 61 - d * 5, 29 - d * 2
        tone = 60 + d * 26
        c.edge3d(x0, y0, x1, y1, (tone, tone + 6, tone + 14), (tone // 2, tone // 2, tone // 2 + 6))
    c.rect(28, 12, 35, 19, (250, 244, 214))           # the light door
    c.edge3d(28, 12, 35, 19, (255, 252, 240), (190, 184, 160))
    c.stipple(0, 28, 63, 31, (90, 96, 110), 60, 2)
    return c.save("passage")


def p_pond():
    c = Cnv(3, 4, (90, 140, 120))
    c.vgrad((130, 170, 120), (70, 110, 80), 0, 20)
    c.blob(24, 42, 20, 16, (60, 110, 110))            # pond
    c.gridretch(6, 34, 42, 56, (60, 110, 110), (50, 96, 100), False)
    for lx in (14, 26, 38):                           # lily pads
        c.blob(lx, 40 + lx % 6, 3, 2, (80, 150, 70))
    c.blob(26, 38, 1.4, 1.4, (240, 120, 160))         # lotus
    c.ring(30, 44, 5, (170, 210, 210), 1)             # ripple
    c.ring(12, 48, 4, (170, 210, 210), 1)
    return c.save("pond")


def p_sunflowers():
    c = Cnv(3, 3, (120, 180, 230))
    c.vgrad((140, 196, 238), (100, 158, 208), 0, 24)
    c.rect(0, 25, 47, 47, (70, 120, 56))
    for fx, fy, fs in ((12, 16, 5), (30, 12, 6), (40, 22, 4)):
        for ptl in range(12):
            a = ptl * 0.523
            x = fx + int((fs + 1) * math.cos(a)); y = fy + int((fs + 1) * math.sin(a))
            c.blob(x, y, 1.6, 1.6, (250, 200, 30))
        c.blob(fx, fy, fs * 0.6, fs * 0.6, (120, 76, 30))
        c.line(fx, fy + 2, fx, 47, (60, 110, 50))
    c.stipple(0, 26, 47, 47, (250, 210, 60), 30, 8)   # field sparkle
    return c.save("sunflowers")


def p_tides():
    c = Cnv(3, 3, (200, 210, 220))
    c.vgrad((210, 222, 232), (160, 180, 200), 0, 12)
    for k in range(5):                                # stylized tide bands
        y0 = 13 + k * 7
        col = [(120, 170, 210), (90, 145, 195), (140, 185, 215),
               (70, 120, 175), (110, 160, 200)][k]
        for x in range(48):
            wv = int(2.2 * math.sin(x * 0.3 + k * 1.3))
            c.rect(x, y0 + wv, x, y0 + 3 + wv, col)
            if x % 6 == k % 3:
                c.px(x, y0 - 1 + wv, (240, 248, 252))  # foam caps
    return c.save("tides")


def p_dennis():
    c = Cnv(3, 3, (150, 190, 220))
    c.vgrad((160, 200, 230), (110, 150, 190), 0, 26)
    c.rect(0, 27, 47, 47, (96, 140, 66))
    c.stipple(0, 27, 47, 47, (120, 168, 80), 90, 4)
    # Dennis the famous shaggy wolf
    c.blob(24, 30, 9, 6.4, (208, 196, 178))
    c.stipple(14, 24, 34, 34, (188, 176, 158), 140, 1)  # shag texture
    c.blob(32, 24, 4.4, 4.4, (214, 202, 184))
    c.rect(29, 20, 30, 23, (214, 202, 184))           # ears
    c.rect(34, 20, 35, 23, (214, 202, 184))
    c.px(30, 23, (30, 26, 22)); c.px(34, 23, (30, 26, 22))
    c.rect(32, 26, 33, 27, (60, 44, 36))              # nose
    c.line(15, 30, 10, 26, (198, 186, 168))           # tail up!
    return c.save("dennis")


PAINTINGS = [
    p_kebab, p_aztec, p_alban, p_aztec2, p_bomb, p_plant, p_wasteland,
    p_pool, p_courbet, p_sea, p_sunset, p_creebet,
    p_wanderer, p_graham,
    p_match, p_bust, p_stage, p_void, p_skull_and_roses, p_wither,
    p_fighters, p_pointer, p_pigscene, p_burning_skull, p_skeleton, p_donkey_kong,
    p_meditative, p_prairie_ride, p_baroque, p_humble, p_unpacked,
    p_backyard, p_bouquet, p_cavebird, p_changing, p_cotan, p_endboss, p_fern,
    p_finding, p_lowmist, p_orb, p_owlemons, p_passage, p_pond, p_sunflowers,
    p_tides, p_dennis,
]

if __name__ == "__main__":
    names = []
    for fn in PAINTINGS:
        names.append(fn())
    print(f"rendered {len(names)} paintings -> {OUT}")
    # atlas (横断図): 8-column grid, 8px gutter, name strip
    from PIL import ImageFont
    cols = 8
    cell_w, cell_h = 64 + 8, 64 + 20
    rows = (len(names) + cols - 1) // cols
    atlas = Image.new("RGB", (cols * cell_w, rows * cell_h), (24, 22, 28))
    ad = ImageDraw.Draw(atlas)
    for i, n in enumerate(names):
        im = Image.open(os.path.join(OUT, n + ".png"))
        x = (i % cols) * cell_w + 4
        y = (i // cols) * cell_h + 4
        ox = x + (64 - im.width) // 2
        oy = y + (64 - im.height) // 2
        ad.rectangle([x + im.width + 3, y - 1, ox - 2, oy + im.height + 2],
                     outline=(70, 66, 80)) if False else None
        atlas.paste(im, (ox, oy))
        ad.rectangle([x, y, x + 63, y + 63 + 12], outline=(60, 56, 70))
        ad.text((x + 2, y + 65), n[:11], fill=(190, 186, 200))
    atlas.save(os.path.join(OUT, "paintings_atlas.png"))
    print("atlas:", os.path.join(OUT, "paintings_atlas.png"))
