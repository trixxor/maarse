#!/usr/bin/env sh
# Bygger Må:rse-firmware.
#
# nRF52840 (laver en UF2-fil, som trækkes over på boardets USB-drev —
# dobbeltklik på reset-knappen for at få drevet frem):
#   ./scripts/build-firmware.sh wismesh-tag
#   ./scripts/build-firmware.sh xiao-nrf52840 [flash <port>]
#
# ESP32-S3 (laver en samlet .bin, og flasher med "flash" som 2. argument):
#   ./scripts/build-firmware.sh heltec-v4 [flash]
#   ./scripts/build-firmware.sh xiao-esp32s3 [flash]
#   Første gang installeres Espressifs værktøjskæde (espup) og espflash.
#
# Signeret pakke til opdatering over luften (dist/maarse-<board>-ota.bin +
# .manifest), når MAARSE_SIGN_KEY peger på en nøgle fra `maarse-sign keygen`
# (eller MAARSE_SIGN_KEY=dev for udviklingsnøglen). Versionen tages fra
# MAARSE_FW_VERSION (skal også være sat under byggeriet, så firmwaren kender
# sin egen version), og den offentlige nøgle fra MAARSE_RELEASE_PUBKEY.
set -e
cd "$(dirname "$0")/.."
BOARD="$1"
ACTION="$2"
PORT="$3"

# Signerer et image med maarse-sign, hvis MAARSE_SIGN_KEY er sat.
sign() {
  [ -n "$MAARSE_SIGN_KEY" ] || return 0
  [ -n "$MAARSE_FW_VERSION" ] || { echo "Sæt MAARSE_FW_VERSION (fx 0.2.0) for at signere"; exit 1; }
  if [ "$MAARSE_SIGN_KEY" = "dev" ]; then KEYARG="--dev-key"; else KEYARG="--key $MAARSE_SIGN_KEY"; fi
  # shellcheck disable=SC2086
  cargo run --release -q -p maarse-uf2 --bin maarse-sign -- sign --board "$BOARD" \
    --version "$MAARSE_FW_VERSION" $KEYARG "$1" "dist/maarse-$BOARD-ota"
}
command -v cargo >/dev/null 2>&1 || { echo "Rust mangler: https://rustup.rs"; exit 1; }
mkdir -p dist

case "$BOARD" in
  wismesh-tag|xiao-nrf52840)
    rustup target add thumbv7em-none-eabihf
    # Bluetooth-delen (nrf-sdc) bruger bindgen, som skal bruge libclang
    # (pakken libclang-dev / clang). Sæt LIBCLANG_PATH, hvis den ikke findes.
    (cd firmware/nrf && cargo build --release --features "board-$BOARD")
    cargo run --release -q -p maarse-uf2 -- \
      firmware/nrf/target/thumbv7em-none-eabihf/release/maarse-fw-nrf "dist/maarse-$BOARD.uf2"
    echo "Færdig: dist/maarse-$BOARD.uf2"
    sign "dist/maarse-$BOARD.uf2"
    if [ "$ACTION" = "flash" ]; then
      [ -n "$PORT" ] || { echo "Angiv porten, fx: $0 $BOARD flash /dev/ttyACM0"; exit 1; }
      python3 scripts/flash-nrf.py "$PORT" "dist/maarse-$BOARD.uf2"
    else
      echo "Flash med:  $0 $BOARD flash <port>  (eller dobbeltklik på reset og træk filen over)"
    fi
    ;;
  heltec-v4|xiao-esp32s3)
    if ! rustup toolchain list | grep -q '^esp'; then
      echo "Installerer Espressifs Rust-værktøjskæde (espup) — tager et par minutter første gang."
      command -v espup >/dev/null 2>&1 || cargo install espup --locked
      espup install --targets esp32s3
    fi
    # espup skriver miljøvariabler (Xtensa-linker) til ~/export-esp.sh
    [ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
    command -v espflash >/dev/null 2>&1 || cargo install espflash --locked
    (cd firmware/esp && cargo +esp build --release --features "board-$BOARD")
    ELF=firmware/esp/target/xtensa-esp32s3-none-elf/release/maarse-fw-esp
    # Flash-størrelse: Heltec V4 16 MB, XIAO ESP32-S3 8 MB (tabellen kræver over 4 MB).
    if [ "$BOARD" = heltec-v4 ]; then FLASH=16mb; else FLASH=8mb; fi
    rm -f "dist/maarse-$BOARD.bin"
    espflash save-image --chip esp32s3 --merge --flash-size "$FLASH" \
      --partition-table firmware/esp/partitions.csv "$ELF" "dist/maarse-$BOARD.bin"
    echo "Færdig: dist/maarse-$BOARD.bin (samlet billede, flashes fra adresse 0x0)"
    # App-imaget alene (til opdatering over luften).
    espflash save-image --chip esp32s3 "$ELF" "dist/maarse-$BOARD-app.bin"
    sign "dist/maarse-$BOARD-app.bin"
    if [ "$ACTION" = "flash" ]; then
      # otadata slettes, så bootloaderen starter ota_0 (det, vi flasher),
      # også selvom en opdatering over luften har valgt ota_1.
      espflash flash ${PORT:+--port "$PORT"} --chip esp32s3 --partition-table firmware/esp/partitions.csv \
        --erase-parts otadata "$ELF"
    else
      echo "Flash med:  ./scripts/build-firmware.sh $BOARD flash"
    fi
    ;;
  *)
    echo "Brug: $0 <wismesh-tag | xiao-nrf52840 | heltec-v4 | xiao-esp32s3> [flash]"
    exit 1 ;;
esac
