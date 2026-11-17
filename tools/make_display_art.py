"""Draws the Heltec OLED artwork and writes firmware/esp/src/art.rs.

- BOOT: the 128x64 boot screen (mast with lightning and waves, "MÅ:RSE",
  "HELTEC V4 / LORA MESH")
- HEX: heavy hex digits for the big ID on the home page
- ICON: a small mast for the page headers

Uses DejaVu Sans Bold / DejaVu Sans Mono Bold (free licence; shipped with
matplotlib). 1 bit per pixel, rows padded to whole bytes, most significant
bit first (embedded-graphics ImageRaw<BinaryColor>).

    python tools/make_display_art.py [--png preview.png]
"""
import os
import pathlib
import sys

from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "firmware" / "esp" / "src" / "art.rs"


def font_path(name):
    import matplotlib
    return os.path.join(os.path.dirname(matplotlib.__file__), "mpl-data", "fonts", "ttf", name)


SANS_BOLD = font_path("DejaVuSans-Bold.ttf")
MONO_BOLD = font_path("DejaVuSansMono-Bold.ttf")


def crop(img):
    box = img.getbbox()
    return img.crop(box) if box else img


def text_img(text, path, size, spacing=0):
    """Text rendered big, then scaled down to `size` px cap height (crisper edges)."""
    big = 8
    f = ImageFont.truetype(path, size * big)
    w = int(sum(f.getlength(c) for c in text) + spacing * big * len(text)) + 40
    img = Image.new("L", (w, size * big * 2), 0)
    d = ImageDraw.Draw(img)
    x = 10
    for c in text:
        d.text((x, size * big // 3), c, font=f, fill=255)
        x += f.getlength(c) + spacing * big
    img = crop(img)
    h = size
    wn = max(1, round(img.width * h / img.height))
    return img.resize((wn, h), Image.LANCZOS).point(lambda v: 255 if v > 110 else 0)


def mast(h):
    """A lattice mast with a lightning bolt on its left and waves on its
    right, drawn pixel by pixel at its real size (scaling a big drawing
    down turned the lattice into a blob). `h` = 21 (boot) or 9 (headers)."""
    if h <= 10:
        # Header icon, 11 x 9: mast with one wave each side.
        img = Image.new("L", (11, 9), 0)
        d = ImageDraw.Draw(img)
        d.line([(5, 2), (2, 8)], fill=255)
        d.line([(5, 2), (8, 8)], fill=255)
        d.line([(3, 6), (7, 6)], fill=255)
        d.point((5, 1), fill=255)
        d.arc([2, -2, 8, 4], -70, 70, fill=255)
        d.arc([2, -2, 8, 4], 110, 250, fill=255)
        return img
    W, H = 40, 21
    img = Image.new("L", (W, H), 0)
    d = ImageDraw.Draw(img)
    cx = 24
    # Mast: two-pixel legs, bars and cross bracing.
    for dx in (0, 1):
        d.line([(cx + dx, 6), (cx - 6 + dx, 20)], fill=255)
        d.line([(cx + dx, 6), (cx + 6 + dx, 20)], fill=255)
    d.line([(cx - 2, 11), (cx + 3, 11)], fill=255)
    d.line([(cx - 4, 15), (cx + 5, 15)], fill=255)
    d.line([(cx - 6, 20), (cx + 7, 20)], fill=255)
    d.line([(cx - 2, 11), (cx + 5, 15)], fill=255)
    d.line([(cx + 3, 11), (cx - 4, 15)], fill=255)
    d.line([(cx - 4, 15), (cx + 6, 20)], fill=255)
    d.line([(cx + 5, 15), (cx - 5, 20)], fill=255)
    # Top: a dot with waves on the right.
    d.ellipse([cx - 1, 2, cx + 2, 5], fill=255)
    d.arc([cx - 4, -1, cx + 5, 8], -60, 60, fill=255, width=2)
    d.arc([cx - 8, -5, cx + 9, 12], -55, 55, fill=255, width=2)
    # Lightning bolt on the left.
    d.polygon([(14, 0), (7, 10), (12, 10), (8, 20), (18, 7), (13, 7), (17, 0)], fill=255)
    return crop(img)


def paste_center(canvas, img, y):
    canvas.paste(img, ((canvas.width - img.width) // 2, y))


def pack(img):
    """1 bpp, MSB first, rows padded to bytes."""
    img = img.convert("L")
    out = bytearray()
    for y in range(img.height):
        byte, n = 0, 0
        for x in range(img.width):
            byte = (byte << 1) | (1 if img.getpixel((x, y)) > 127 else 0)
            n += 1
            if n == 8:
                out.append(byte)
                byte, n = 0, 0
        if n:
            out.append(byte << (8 - n))
    return bytes(out)


def rust_bytes(name, data, comment):
    rows = [", ".join(f"0x{b:02x}" for b in data[i:i + 16]) for i in range(0, len(data), 16)]
    body = ",\n    ".join(rows)
    return f"/// {comment}\npub const {name}: [u8; {len(data)}] = [\n    {body},\n];\n"


def main():
    # Boot screen.
    boot = Image.new("L", (128, 64), 0)
    icon = mast(21)
    paste_center(boot, icon, 0)
    logo = text_img("MÅ:RSE", SANS_BOLD, 22, spacing=0.5)
    if logo.width > 124:
        logo = logo.resize((124, logo.height), Image.LANCZOS).point(lambda v: 255 if v > 110 else 0)
    paste_center(boot, logo, 23)
    sub1 = text_img("HELTEC V4", SANS_BOLD, 7, spacing=0.6)
    sub2 = text_img("LORA MESH", SANS_BOLD, 7, spacing=0.6)
    paste_center(boot, sub1, 48)
    paste_center(boot, sub2, 57)

    # Heavy hex digits, all in one cell size.
    digits = "0123456789abcdef"
    gh = 18
    glyphs = []
    for c in digits:
        g = text_img(c, MONO_BOLD, gh)
        # Condensed: eight digits must fit with room to spare (8 x 13 = 104 px).
        w = max(1, round(g.width * 0.72))
        glyphs.append(g.resize((w, g.height), Image.LANCZOS).point(lambda v: 255 if v > 110 else 0))
    gw = max(g.width for g in glyphs) + 1
    cells = []
    for g in glyphs:
        cell = Image.new("L", (gw, gh), 0)
        cell.paste(g, ((gw - g.width) // 2, gh - g.height))
        cells.append(cell)

    # Small header icon.
    small = mast(9)

    rs = ["//! Generated by tools/make_display_art.py - do not edit by hand.",
          "//! 1 bit per pixel, rows padded to bytes, most significant bit first.", ""]
    rs.append(rust_bytes("BOOT", pack(boot), "Boot screen, 128 x 64."))
    rs.append(f"pub const HEX_W: u32 = {gw};\npub const HEX_H: u32 = {gh};\n")
    hex_data = b"".join(pack(c) for c in cells)
    per = len(pack(cells[0]))
    rs.append(f"/// Bytes per hex digit.\npub const HEX_BYTES: usize = {per};\n")
    rs.append(rust_bytes("HEX", hex_data, "Hex digits 0-9, a-f (HEX_W x HEX_H each, one after the other)."))
    rs.append(f"pub const ICON_W: u32 = {small.width};\npub const ICON_H: u32 = {small.height};\n")
    rs.append(rust_bytes("ICON", pack(small), "Small mast for the page headers."))
    OUT.write_text("\n".join(rs), encoding="utf-8", newline="\n")
    print(f"wrote {OUT} (hex digit {gw}x{gh}, icon {small.width}x{small.height})")

    if "--png" in sys.argv:
        p = pathlib.Path(sys.argv[sys.argv.index("--png") + 1])
        sheet = Image.new("L", (128 * 4, 64 * 4 + 30 * 4), 0)
        sheet.paste(boot.resize((512, 256), Image.NEAREST), (0, 0))
        x = 0
        for c in cells:
            sheet.paste(c.resize((c.width * 4, c.height * 4), Image.NEAREST), (x, 260))
            x += c.width * 4 + 4
        sheet.save(p)
        print(p)


if __name__ == "__main__":
    main()
