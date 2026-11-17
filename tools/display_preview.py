"""Renders the Heltec OLED pages to one PNG sheet.

Builds and runs tools/display-preview (the firmware's own drawing code with
sample data) and lays the pages out at 3x scale in OLED colours.

    python tools/display_preview.py [out.png]
"""
import pathlib
import subprocess
import sys
import tempfile

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCALE = 3
FG, BG, FRAME = (140, 200, 255), (8, 10, 16), (60, 60, 70)


def main():
    out = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "dist" / "display-pages.png"
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(["cargo", "run", "-q", "--release", "--manifest-path",
                        str(ROOT / "tools" / "display-preview" / "Cargo.toml"), "--", tmp], check=True)
        files = sorted(pathlib.Path(tmp).glob("*.pbm"))
        cols = 3
        rows = (len(files) + cols - 1) // cols
        cw, ch = 128 * SCALE + 24, 64 * SCALE + 40
        sheet = Image.new("RGB", (cols * cw + 12, rows * ch + 12), (30, 30, 36))
        draw = ImageDraw.Draw(sheet)
        for i, f in enumerate(files):
            lines = f.read_text().split("\n")[2:]
            img = Image.new("RGB", (128, 64), BG)
            for y, line in enumerate(lines[:64]):
                for x, b in enumerate(line.split()):
                    if b == "1":
                        img.putpixel((x, y), FG)
            img = img.resize((128 * SCALE, 64 * SCALE), Image.NEAREST)
            x0 = 12 + (i % cols) * cw
            y0 = 12 + (i // cols) * ch
            draw.rectangle([x0 - 4, y0 - 4, x0 + 128 * SCALE + 3, y0 + 64 * SCALE + 3], outline=FRAME, width=2)
            sheet.paste(img, (x0, y0))
            draw.text((x0, y0 + 64 * SCALE + 8), f.stem, fill=(200, 200, 210))
        out.parent.mkdir(parents=True, exist_ok=True)
        sheet.save(out)
        print(out)


if __name__ == "__main__":
    main()
