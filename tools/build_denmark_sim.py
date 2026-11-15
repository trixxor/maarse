#!/usr/bin/env python3
"""Build the Denmark simulator's WebAssembly engine.

Compiles maarse-sim-wasm for wasm32-unknown-unknown (release) and copies the
module to docs/denmark/maarse_sim_wasm.wasm. No wasm-bindgen or wasm-pack is
needed: the module exports a small C ABI that docs/denmark/maarse-sim.js uses.
If `wasm-opt` (binaryen) is on PATH it is used to shrink the module.

Usage:
    rustup target add wasm32-unknown-unknown   # once
    python tools/build_denmark_sim.py
Then serve docs/ over HTTP (browsers do not load .wasm from file://), e.g.
    python -m http.server -d docs 8000   ->  http://localhost:8000/denmark/
"""
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "denmark" / "maarse_sim_wasm.wasm"


def main() -> int:
    cmd = ["cargo", "build", "--release", "--target", "wasm32-unknown-unknown", "-p", "maarse-sim-wasm"]
    print("+", " ".join(cmd))
    if subprocess.call(cmd, cwd=ROOT) != 0:
        return 1
    built = ROOT / "target" / "wasm32-unknown-unknown" / "release" / "maarse_sim_wasm.wasm"
    opt = shutil.which("wasm-opt")
    if opt:
        print("+ wasm-opt -O3")
        subprocess.check_call([opt, "-O3", "--enable-bulk-memory", "-o", str(OUT), str(built)])
    else:
        shutil.copyfile(built, OUT)
    print(f"wrote {OUT.relative_to(ROOT)} ({OUT.stat().st_size // 1024} KiB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
