#!/bin/sh
# Serves docs/ for the Denmark simulator: open http://127.0.0.1:8000/denmark/
cd "$(dirname "$0")/.." && exec python3 -m http.server -d docs 8000
