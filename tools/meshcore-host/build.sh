#!/usr/bin/env bash
# Builds meshcore-host, the radio worker for the backbone's MeshCore gateway.
#
#   tools/meshcore-host/build.sh [output-file]
#
# Needs git, cmake and a C++17 compiler (on Debian: apt install git cmake g++).
# The official MeshCore sources are fetched at the revision pinned in
# firmware/meshcore-gateway/MESHCORE_REVISION and compiled unmodified.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REVISION="$(tr -d '[:space:]' < "$HERE/../../firmware/meshcore-gateway/MESHCORE_REVISION")"
SRC="$HERE/meshcore-src"
OUT="${1:-$HERE/build/meshcore-host}"

if [ ! -d "$SRC/.git" ]; then
  git init -q "$SRC"
  git -C "$SRC" fetch -q --depth 1 https://github.com/meshcore-dev/meshcore "$REVISION"
  git -C "$SRC" checkout -q --detach FETCH_HEAD
fi
if [ "$(git -C "$SRC" rev-parse HEAD)" != "$REVISION" ]; then
  echo "Expected MeshCore revision $REVISION in $SRC." >&2
  exit 1
fi

cmake -S "$HERE" -B "$HERE/build" -DCMAKE_BUILD_TYPE=Release
cmake --build "$HERE/build" --parallel
mkdir -p "$(dirname "$OUT")"
[ "$HERE/build/meshcore-host" = "$OUT" ] || cp "$HERE/build/meshcore-host" "$OUT"
echo "Built $OUT"
