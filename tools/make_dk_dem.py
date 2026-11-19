"""Builds docs/denmark/dk-dem.bin: a ground height grid of Denmark for the
simulator (maarse-sim-wasm/src/terrain.rs).

Source: the public Terrarium elevation tiles (AWS open data, mostly SRTM).
Output: zlib-deflated bytes, one per cell, whole metres (0..255), row 0 at
the south edge, plus docs/denmark/dk-dem.json with the grid layout.

Usage: python tools/make_dk_dem.py   (needs internet and Pillow)
"""
import io
import json
import math
import os
import urllib.request
import zlib

from PIL import Image

LAT0, LAT1 = 54.50, 57.80
LON0, LON1 = 7.90, 15.30
DLAT = 0.0025          # ~280 m
DLON = 0.0040          # ~250 m at 56 N
Z = 10                 # source tiles, ~95 m per pixel here
URL = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png"
OUT = os.path.join(os.path.dirname(__file__), "..", "docs", "denmark")

tiles = {}


def tile(x, y):
    if (x, y) not in tiles:
        req = urllib.request.Request(URL.format(z=Z, x=x, y=y), headers={"User-Agent": "maarse-dem-builder"})
        with urllib.request.urlopen(req, timeout=60) as r:
            im = Image.open(io.BytesIO(r.read())).convert("RGB")
        tiles[(x, y)] = im.load()
    return tiles[(x, y)]


def height(lat, lon):
    n = 2 ** Z
    px = (lon + 180) / 360 * n * 256
    r = math.radians(lat)
    py = (1 - math.log(math.tan(r) + 1 / math.cos(r)) / math.pi) / 2 * n * 256
    tx, ty = int(px // 256), int(py // 256)
    R, G, B = tile(tx, ty)[int(px) - tx * 256, int(py) - ty * 256]
    return R * 256 + G + B / 256 - 32768


def main():
    w = int(round((LON1 - LON0) / DLON)) + 1
    h = int(round((LAT1 - LAT0) / DLAT)) + 1
    out = bytearray(w * h)
    for y in range(h):
        lat = LAT0 + y * DLAT
        for x in range(w):
            out[y * w + x] = max(0, min(255, round(height(lat, LON0 + x * DLON))))
        if y % 100 == 0:
            print(f"row {y}/{h}, {len(tiles)} tiles")
    with open(os.path.join(OUT, "dk-dem.bin"), "wb") as f:
        f.write(zlib.compress(bytes(out), 9))
    meta = {"lat0": LAT0, "lon0": LON0, "dlat": DLAT, "dlon": DLON, "w": w, "h": h,
            "source": "Terrarium elevation tiles (AWS open data, SRTM), zoom %d" % Z}
    with open(os.path.join(OUT, "dk-dem.json"), "w") as f:
        json.dump(meta, f, indent=1)
    print(meta, "max", max(out))


if __name__ == "__main__":
    main()
