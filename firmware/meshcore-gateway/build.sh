#!/usr/bin/env bash
# Builds the gateway firmware on Linux (used by the release workflow) and
# writes one merged image for the web flasher: bootloader at 0x0, partition
# table at 0x8000, boot_app0 at 0xe000 and the app at 0x10000.
#
#   firmware/meshcore-gateway/build.sh heltec_v4_kiss_backbone dist/maarse-heltec-v4-gateway.bin
#
# Needs git, PlatformIO (pio) and esptool 4.x. Set ESPTOOL to another command
# to run esptool differently (default: python3 -m esptool).
set -euo pipefail

ENVIRONMENT="$1"
OUT="$2"
HERE="$(cd "$(dirname "$0")" && pwd)"
REVISION="$(tr -d '[:space:]' < "$HERE/MESHCORE_REVISION")"
SRC="$HERE/src"
ESPTOOL="${ESPTOOL:-python3 -m esptool}"

if [ ! -d "$SRC/.git" ]; then
  git init -q "$SRC"
  git -C "$SRC" fetch -q --depth 1 https://github.com/meshcore-dev/meshcore "$REVISION"
  git -C "$SRC" checkout -q --detach FETCH_HEAD
fi
if [ "$(git -C "$SRC" rev-parse HEAD)" != "$REVISION" ]; then
  echo "Expected MeshCore revision $REVISION in $SRC." >&2
  exit 1
fi

cp "$HERE/platformio.gateway.ini" "$SRC/platformio.local.ini"
pio run -d "$SRC" -e "$ENVIRONMENT"

BUILD="$SRC/.pio/build/$ENVIRONMENT"
BOOT_APP0="$(find "${PLATFORMIO_CORE_DIR:-$HOME/.platformio}/packages/framework-arduinoespressif32/tools/partitions" -name boot_app0.bin | head -1)"
[ -f "$BOOT_APP0" ] || { echo "boot_app0.bin not found" >&2; exit 1; }

mkdir -p "$(dirname "$OUT")"
$ESPTOOL --chip esp32s3 merge_bin -o "$OUT" \
  --flash_mode keep --flash_freq keep --flash_size keep \
  0x0 "$BUILD/bootloader.bin" \
  0x8000 "$BUILD/partitions.bin" \
  0xe000 "$BOOT_APP0" \
  0x10000 "$BUILD/firmware.bin"
cp "$SRC/license.txt" "$(dirname "$OUT")/meshcore-license.txt"
echo "Wrote $OUT ($(wc -c < "$OUT") bytes)"
