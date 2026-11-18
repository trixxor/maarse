"""Draws the pictures for the backbone demo pages (docs/sample-pages/backbone/demo).

Flat poster-style illustrations in 3:2, made from shapes only, so they belong
to the project (no outside pictures, no licences). The host makes the small
copies for LoRa itself.

    python tools/make_demo_pictures.py docs/sample-pages/backbone/demo
"""
import math
import random
import sys
from PIL import Image, ImageDraw, ImageFilter

W, H = 1200, 800
random.seed(7)


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def sky(img, stops):
    """Vertical gradient through (position 0..1, colour) stops."""
    d = ImageDraw.Draw(img)
    for y in range(img.height):
        t = y / (img.height - 1)
        for (p0, c0), (p1, c1) in zip(stops, stops[1:]):
            if p0 <= t <= p1:
                d.line([(0, y), (img.width, y)], fill=lerp(c0, c1, (t - p0) / (p1 - p0 or 1)))
                break


def glow(img, x, y, r, colour, strength=0.8):
    """A soft round glow (sun, moon, lamp)."""
    layer = Image.new("RGB", img.size, (0, 0, 0))
    d = ImageDraw.Draw(layer)
    for k in range(12, 0, -1):
        rr = r * k / 4
        a = strength * (1 - k / 12.5) ** 2
        d.ellipse([x - rr, y - rr, x + rr, y + rr], fill=tuple(int(c * a) for c in colour))
    layer = layer.filter(ImageFilter.GaussianBlur(r / 2))
    img.paste(Image.blend(img, Image.composite(layer, img, layer.convert("L")), 1), (0, 0))
    base = img.load()
    lp = layer.load()
    for yy in range(max(0, int(y - r * 3)), min(img.height, int(y + r * 3))):
        for xx in range(max(0, int(x - r * 3)), min(img.width, int(x + r * 3))):
            b = base[xx, yy]
            g = lp[xx, yy]
            base[xx, yy] = (min(255, b[0] + g[0]), min(255, b[1] + g[1]), min(255, b[2] + g[2]))


def waves(d, y0, y1, colour, step=26, amp=7, width=3):
    for y in range(y0, y1, step):
        off = random.randint(0, 60)
        for x in range(-80 + off, W, 120):
            d.arc([x, y, x + 70, y + amp * 3], 200, 340, fill=colour, width=width)


def boat(d, x, y, s, hull, sail=None, mast=(30, 30, 40), flag=None):
    """A small boat: hull, mast, optional sail."""
    d.polygon([(x - 60 * s, y), (x + 60 * s, y), (x + 45 * s, y + 22 * s), (x - 45 * s, y + 22 * s)], fill=hull)
    d.line([(x, y), (x, y - 120 * s)], fill=mast, width=max(2, int(4 * s)))
    if sail:
        d.polygon([(x + 4 * s, y - 115 * s), (x + 4 * s, y - 8 * s), (x + 55 * s, y - 8 * s)], fill=sail)
        d.polygon([(x - 4 * s, y - 100 * s), (x - 4 * s, y - 8 * s), (x - 38 * s, y - 8 * s)], fill=tuple(int(c * 0.85) for c in sail))
    if flag:
        d.polygon([(x, y - 120 * s), (x + 22 * s, y - 113 * s), (x, y - 106 * s)], fill=flag)


def storm(out):
    img = Image.new("RGB", (W, H))
    sky(img, [(0, (28, 34, 48)), (0.45, (62, 72, 92)), (0.62, (110, 118, 128)), (1, (40, 52, 66))])
    d = ImageDraw.Draw(img)
    # Heavy clouds.
    for i in range(14):
        cx, cy, r = random.randint(-100, W + 100), random.randint(0, 260), random.randint(90, 200)
        d.ellipse([cx - r * 1.6, cy - r * 0.6, cx + r * 1.6, cy + r * 0.6], fill=(36 + i, 42 + i, 56 + i))
    img = img.filter(ImageFilter.GaussianBlur(6))
    d = ImageDraw.Draw(img)
    # Sea with white caps.
    d.rectangle([0, 470, W, H], fill=(32, 54, 70))
    waves(d, 480, H, (170, 190, 200), step=22, amp=9, width=4)
    # Harbour mole and lighthouse with a beam.
    d.polygon([(0, 560), (520, 540), (560, 600), (0, 640)], fill=(26, 28, 32))
    lx = 470
    d.polygon([(lx - 22, 545), (lx + 22, 545), (lx + 14, 400), (lx - 14, 400)], fill=(236, 236, 230))
    for yb in (440, 490):
        d.polygon([(lx - 18, yb + 18), (lx + 18, yb + 18), (lx + 16, yb), (lx - 16, yb)], fill=(200, 40, 46))
    d.rectangle([lx - 12, 380, lx + 12, 400], fill=(255, 222, 120))
    beam = Image.new("L", (W, H), 0)
    ImageDraw.Draw(beam).polygon([(lx, 390), (W, 260), (W, 420)], fill=90)
    img.paste((255, 236, 170), (0, 0), beam.filter(ImageFilter.GaussianBlur(14)))
    d = ImageDraw.Draw(img)
    # Boats tied in, leaning.
    for i, (x, c) in enumerate([(150, (180, 60, 60)), (290, (40, 90, 150)), (390, (220, 220, 210))]):
        boat(d, x, 560 + (i % 2) * 6, 0.7, c)
    # Rain.
    for _ in range(900):
        x, y = random.randint(-200, W), random.randint(-50, H)
        d.line([(x, y), (x + 16, y + 34)], fill=(190, 200, 215), width=1)
    img.save(out)


def fuelpier(out):
    img = Image.new("RGB", (W, H))
    sky(img, [(0, (120, 182, 230)), (0.55, (250, 214, 170)), (0.62, (255, 236, 200)), (1, (70, 140, 170))])
    glow(img, 860, 420, 60, (255, 230, 160), 0.9)
    d = ImageDraw.Draw(img)
    d.ellipse([820, 380, 900, 460], fill=(255, 244, 210))
    d.rectangle([0, 470, W, H], fill=(64, 150, 178))
    waves(d, 480, H, (150, 210, 225), step=30, amp=5, width=3)
    # The pier on posts.
    d.rectangle([0, 500, 760, 524], fill=(120, 84, 56))
    for x in range(20, 760, 70):
        d.rectangle([x, 524, x + 12, 640], fill=(86, 60, 40))
    # Fuel pump.
    px = 520
    d.rounded_rectangle([px, 380, px + 70, 500], 10, fill=(214, 40, 46))
    d.rectangle([px + 12, 396, px + 58, 430], fill=(240, 240, 232))
    d.line([(px + 70, 440), (px + 110, 470), (px + 120, 520)], fill=(30, 30, 30), width=6)
    d.text((px + 18, 440), "FUEL", fill=(255, 255, 255))
    # Sign "OPEN".
    d.rectangle([180, 400, 330, 450], fill=(40, 140, 80))
    d.line([(255, 450), (255, 500)], fill=(60, 50, 40), width=6)
    d.text((228, 418), "OPEN", fill=(255, 255, 255))
    # A boat alongside.
    boat(d, 860, 560, 1.0, (250, 250, 245), sail=None, flag=(200, 40, 46))
    d.polygon([(780, 560), (940, 560), (930, 548), (790, 548)], fill=(40, 90, 150))
    # Gulls.
    for gx, gy in [(300, 180), (360, 150), (980, 220)]:
        d.arc([gx, gy, gx + 30, gy + 20], 200, 340, fill=(60, 60, 70), width=3)
        d.arc([gx + 28, gy, gx + 58, gy + 20], 200, 340, fill=(60, 60, 70), width=3)
    img.save(out)


def dusk(out):
    img = Image.new("RGB", (W, H))
    sky(img, [(0, (22, 30, 60)), (0.35, (92, 60, 110)), (0.55, (236, 126, 96)), (0.6, (252, 190, 120)), (1, (18, 26, 48))])
    d = ImageDraw.Draw(img)
    for _ in range(80):
        x, y = random.randint(0, W), random.randint(0, 220)
        d.point((x, y), fill=(255, 255, 255))
    glow(img, 600, 470, 50, (255, 170, 110), 0.9)
    d = ImageDraw.Draw(img)
    # Water with the sun's path.
    d.rectangle([0, 480, W, H], fill=(28, 40, 70))
    for y in range(486, H, 9):
        w = 30 + (y - 480) * 0.9
        d.line([(600 - w + random.randint(-12, 12), y), (600 + w + random.randint(-12, 12), y)], fill=(250, 170, 110), width=2)
    # Far shore.
    d.polygon([(0, 470), (200, 455), (420, 466), (700, 452), (1000, 462), (W, 450), (W, 482), (0, 482)], fill=(40, 34, 60))
    # Marina silhouettes: masts and hulls, a quay with lamps.
    d.rectangle([0, 600, W, 640], fill=(14, 16, 26))
    for i, x in enumerate(range(60, W, 95)):
        h = 140 + (i * 37) % 90
        d.line([(x, 600), (x, 600 - h)], fill=(14, 16, 26), width=4)
        d.polygon([(x - 38, 600), (x + 38, 600), (x + 28, 616), (x - 28, 616)], fill=(14, 16, 26))
        d.line([(x, 600 - h), (x + 30, 598)], fill=(14, 16, 26), width=1)
    for x in range(120, W, 260):
        d.line([(x, 640), (x, 560)], fill=(10, 10, 16), width=4)
        glow(img, x, 556, 10, (255, 210, 140), 1.0)
        d = ImageDraw.Draw(img)
        d.ellipse([x - 7, 549, x + 7, 563], fill=(255, 226, 170))
    img.save(out)


def regatta(out):
    img = Image.new("RGB", (W, H), (255, 228, 92))
    d = ImageDraw.Draw(img)
    # Bold zine colours: yellow sky with halftone dots (the photocopied look), pink sun, blue sea.
    for yy in range(0, 430, 14):
        for xx in range((yy // 14 % 2) * 7, W, 14):
            r = 1 + 2.2 * (1 - yy / 430)
            d.ellipse([xx - r, yy - r, xx + r, yy + r], fill=(235, 200, 60))
    d.ellipse([820, 80, 1080, 340], fill=(255, 59, 107))
    d.rectangle([0, 430, W, H], fill=(40, 120, 210))
    for y in range(450, H, 34):
        for x in range(-40, W, 110):
            d.arc([x + (y % 68), y, x + 70 + (y % 68), y + 18], 200, 340, fill=(160, 210, 255), width=5)
    # Three racing yachts, heeling.
    for (x, y, s, sail, hull) in [(260, 470, 1.6, (255, 255, 255), (20, 20, 20)), (620, 500, 1.2, (255, 59, 107), (250, 250, 250)),
                                 (930, 480, 1.0, (17, 17, 17), (255, 228, 92))]:
        layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        boat(ImageDraw.Draw(layer), x, y, s, hull, sail=sail, mast=(17, 17, 17))
        layer = layer.rotate(-9, center=(x, y))
        img.paste(layer, (0, 0), layer)
    d = ImageDraw.Draw(img)
    img.save(out)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    storm(f"{out}/storm.png")
    fuelpier(f"{out}/fuelpier.png")
    dusk(f"{out}/dusk.png")
    regatta(f"{out}/regatta.png")
    print("ok")
