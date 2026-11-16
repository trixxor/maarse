#!/usr/bin/env sh
# Builds Må:rse firmware.
#
# nRF52840 (makes a UF2 file that is dragged onto the board's USB drive —
# double-click the reset button to bring up the drive):
#   ./scripts/build-firmware.sh wismesh-tag
#   ./scripts/build-firmware.sh xiao-nrf52840 [flash <port>]
#
# ESP32-S3 (makes a merged .bin, and flashes with "flash" as the 2nd argument):
#   ./scripts/build-firmware.sh heltec-v4 [flash]
#   ./scripts/build-firmware.sh xiao-esp32s3 [flash]
#   The first time, Espressif's toolchain (espup) and espflash are installed.
#
# Signed package for over-the-air updates (dist/maarse-<board>-ota.bin +
# .manifest) when MAARSE_SIGN_KEY points to a key from `maarse-sign keygen`
# (or MAARSE_SIGN_KEY=dev for the development key). The version is taken from
# MAARSE_FW_VERSION (must also be set during the build, so the firmware knows
# its own version), and the public key from MAARSE_RELEASE_PUBKEY.
set -e
cd "$(dirname "$0")/.."
BOARD="$1"
ACTION="$2"
PORT="$3"

# Signs an image with maarse-sign if MAARSE_SIGN_KEY is set.
sign() {
  [ -n "$MAARSE_SIGN_KEY" ] || return 0
  [ -n "$MAARSE_FW_VERSION" ] || { echo "Set MAARSE_FW_VERSION (e.g. 0.2.0) to sign"; exit 1; }
  if [ "$MAARSE_SIGN_KEY" = "dev" ]; then KEYARG="--dev-key"; else KEYARG="--key $MAARSE_SIGN_KEY"; fi
  # shellcheck disable=SC2086
  cargo run --release -q -p maarse-uf2 --bin maarse-sign -- sign --board "$BOARD" \
    --version "$MAARSE_FW_VERSION" $KEYARG "$1" "dist/maarse-$BOARD-ota"
}
command -v cargo >/dev/null 2>&1 || { echo "Rust is missing: https://rustup.rs"; exit 1; }
mkdir -p dist

case "$BOARD" in
  wismesh-tag|xiao-nrf52840)
    rustup target add thumbv7em-none-eabihf
    # The Bluetooth part (nrf-sdc) uses bindgen, which needs libclang
    # (the package libclang-dev / clang). Set LIBCLANG_PATH if it is not found.
    (cd firmware/nrf && cargo build --release --features "board-$BOARD")
    cargo run --release -q -p maarse-uf2 -- \
      firmware/nrf/target/thumbv7em-none-eabihf/release/maarse-fw-nrf "dist/maarse-$BOARD.uf2"
    echo "Done: dist/maarse-$BOARD.uf2"
    sign "dist/maarse-$BOARD.uf2"
    if [ "$ACTION" = "flash" ]; then
      [ -n "$PORT" ] || { echo "Give the port, e.g.: $0 $BOARD flash /dev/ttyACM0"; exit 1; }
      python3 scripts/flash-nrf.py "$PORT" "dist/maarse-$BOARD.uf2"
    else
      echo "Flash with:  $0 $BOARD flash <port>  (or double-click reset and drag the file over)"
    fi
    ;;
  heltec-v4|xiao-esp32s3)
    if ! rustup toolchain list | grep -q '^esp'; then
      echo "Installing Espressif's Rust toolchain (espup) — takes a few minutes the first time."
      command -v espup >/dev/null 2>&1 || cargo install espup --locked
      espup install --targets esp32s3
    fi
    # espup writes environment variables (Xtensa linker) to ~/export-esp.sh
    [ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
    command -v espflash >/dev/null 2>&1 || cargo install espflash --locked
    (cd firmware/esp && cargo +esp build --release --features "board-$BOARD")
    ELF=firmware/esp/target/xtensa-esp32s3-none-elf/release/maarse-fw-esp
    # Flash size: Heltec V4 16 MB, XIAO ESP32-S3 8 MB (the table needs more than 4 MB).
    if [ "$BOARD" = heltec-v4 ]; then FLASH=16mb; else FLASH=8mb; fi
    rm -f "dist/maarse-$BOARD.bin"
    espflash save-image --chip esp32s3 --merge --flash-size "$FLASH" \
      --partition-table firmware/esp/partitions.csv "$ELF" "dist/maarse-$BOARD.bin"
    echo "Done: dist/maarse-$BOARD.bin (merged image, flashed from address 0x0)"
    # The app image alone (for over-the-air updates).
    espflash save-image --chip esp32s3 "$ELF" "dist/maarse-$BOARD-app.bin"
    sign "dist/maarse-$BOARD-app.bin"
    if [ "$ACTION" = "flash" ]; then
      # otadata is erased, so the bootloader starts ota_0 (the one we flash),
      # even if an over-the-air update has chosen ota_1.
      espflash flash ${PORT:+--port "$PORT"} --chip esp32s3 --partition-table firmware/esp/partitions.csv \
        --erase-parts otadata "$ELF"
    else
      echo "Flash with:  ./scripts/build-firmware.sh $BOARD flash"
    fi
    ;;
  *)
    echo "Usage: $0 <wismesh-tag | xiao-nrf52840 | heltec-v4 | xiao-esp32s3> [flash]"
    exit 1 ;;
esac
