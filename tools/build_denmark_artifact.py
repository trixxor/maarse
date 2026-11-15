#!/usr/bin/env python3
"""Make the claude.ai artifact version of the Denmark simulator page.

The artifact platform wraps the published page in its own document skeleton
(doctype, head with charset and viewport), so docs/denmark/artifact.html is
docs/denmark/index.html without those parts and without links to pages that
are not published with it (the Mesh Lab). The supporting files (scripts,
styles, data, the WebAssembly engine) are published next to it unchanged.

Usage:
    python tools/build_denmark_sim.py        # engine first, if it changed
    python tools/build_denmark_artifact.py
Then publish docs/denmark/artifact.html with the files listed in FILES.
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "docs" / "denmark" / "index.html"
OUT = ROOT / "docs" / "denmark" / "artifact.html"

# Published next to the page (path in the artifact = path under docs/denmark).
FILES = [
    "style.css", "vendor/leaflet-lite.css", "dk-land.js", "dk-towns.js", "scenarios.js",
    "maarse-sim.js", "app.js", "ask.js", "maarse_sim_wasm.wasm", "README.md", "AREA-ALERTS.md",
]


def main() -> int:
    s = SRC.read_text(encoding="utf-8")
    for pat in [r"<!doctype html>\s*", r"<html[^>]*>\s*", r"</html>\s*", r"<head>\s*", r"</head>\s*",
                r"<body>\s*", r"</body>\s*", r'<meta charset="utf-8">\s*', r'<meta name="viewport"[^>]*>\s*']:
        s = re.sub(pat, "", s, flags=re.I)
    s = re.sub(r'\s*<a href="\.\./simulator/"[^>]*>Mesh Lab</a>', "", s)
    OUT.write_text(s, encoding="utf-8", newline="\n")
    print(f"wrote {OUT.relative_to(ROOT)}; publish it with: {', '.join(FILES)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
