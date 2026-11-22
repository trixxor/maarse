#!/usr/bin/env python3
"""Makes the Windows icons from the app icon (the same artwork as the Android launcher icon).

  packaging/windows/maarse.ico            multi-size icon for the exe files, shortcuts and installer
  maarse-desktop/assets/icon-64.rgba      64x64 RGBA, drawn into the tray icon (with a status dot)

Run from the repository root: python tools/make_windows_icon.py
"""
from pathlib import Path

from PIL import Image

SRC = Path("maarse-client/icon-512.png")
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

img = Image.open(SRC).convert("RGBA")
ico = Path("packaging/windows/maarse.ico")
ico.parent.mkdir(parents=True, exist_ok=True)
img.save(ico, format="ICO", sizes=[(s, s) for s in SIZES])

raw = Path("maarse-desktop/assets/icon-64.rgba")
raw.parent.mkdir(parents=True, exist_ok=True)
raw.write_bytes(img.resize((64, 64), Image.LANCZOS).tobytes())
print(f"wrote {ico} ({ico.stat().st_size} bytes) and {raw} ({raw.stat().st_size} bytes)")
