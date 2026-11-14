# Laver app-ikonerne til webklienten (PWA): icon.svg, icon-192.png og icon-512.png.
#   python tools/make_icons.py
# Kun standardbiblioteket (zlib/struct). Mærket er et stiliseret "M:" på grøn baggrund.
import os, struct, zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "maarse-client")
BG, FG = (0x2F, 0x7D, 0x5B), (0xFF, 0xFF, 0xFF)
# Alt er angivet på et 512-gitter.
R = 112                                     # hjørneradius
STROKES = [((136, 372), (136, 140)),        # venstre ben
           ((136, 140), (232, 284)),        # skrå ned
           ((232, 284), (328, 140)),        # skrå op
           ((328, 140), (328, 372))]        # højre ben
W = 52                                      # stregtykkelse
DOTS = [(408, 196), (408, 336)]             # kolon
DR = 32


def seg_dist2(px, py, a, b):
    (ax, ay), (bx, by) = a, b
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    cx, cy = ax + t * dx - px, ay + t * dy - py
    return cx * cx + cy * cy


def inside_bg(x, y):
    cx, cy = min(max(x, R), 512 - R), min(max(y, R), 512 - R)
    return (x - cx) ** 2 + (y - cy) ** 2 <= R * R


def inside_fg(x, y):
    if any(seg_dist2(x, y, a, b) <= (W / 2) ** 2 for a, b in STROKES):
        return True
    return any((x - dx) ** 2 + (y - dy) ** 2 <= DR * DR for dx, dy in DOTS)


def png(size):
    ss, rows = 3, []
    for j in range(size):
        row = bytearray([0])
        for i in range(size):
            bg = fg = 0
            for sj in range(ss):
                for si in range(ss):
                    x = (i + (si + .5) / ss) * 512 / size
                    y = (j + (sj + .5) / ss) * 512 / size
                    if inside_bg(x, y):
                        bg += 1
                        if inside_fg(x, y):
                            fg += 1
            if bg == 0:
                row += bytes(4)
                continue
            t = fg / bg
            col = [round(BG[k] * (1 - t) + FG[k] * t) for k in range(3)]
            row += bytes(col + [round(255 * bg / (ss * ss))])
        rows.append(bytes(row))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b""))


def svg():
    pts = " ".join(f"{x},{y}" for x, y in [STROKES[0][0]] + [s[1] for s in STROKES])
    dots = "".join(f'<circle cx="{x}" cy="{y}" r="{DR}"/>' for x, y in DOTS)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">'
            f'<rect width="512" height="512" rx="{R}" fill="#2f7d5b"/>'
            f'<polyline points="{pts}" fill="none" stroke="#fff" stroke-width="{W}" '
            f'stroke-linecap="round" stroke-linejoin="round"/>'
            f'<g fill="#fff">{dots}</g></svg>\n')


if __name__ == "__main__":
    with open(os.path.join(OUT, "icon.svg"), "w", newline="\n") as f:
        f.write(svg())
    for s in (192, 512):
        with open(os.path.join(OUT, f"icon-{s}.png"), "wb") as f:
            f.write(png(s))
    print("ikoner skrevet til", os.path.abspath(OUT))
