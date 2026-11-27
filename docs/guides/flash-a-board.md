# How to flash a board

> **Type:** how-to · **Goal:** put Må:rse firmware on a LoRa board · **Assumes:** you know which board you have. If you are new, start with the [tutorial](../tutorials/first-node.md).
>
> **Status:** `working copy`. The commands are taken from [GUIDE.md](../GUIDE.md) and [FIELD-TEST.md](../FIELD-TEST.md). The web flasher is live at <https://trixxor.github.io/maarse/flash.html> but has not been tested on every board. `bench-tested`: Heltec V4 and XIAO nRF52840 by cable. WisMesh Tag and XIAO ESP32-S3 build, but are not flashed.

Supported boards: `wismesh-tag`, `xiao-nrf52840`, `heltec-v4`, `xiao-esp32s3`.

## Option A: the web flasher (no tools to install)

1. Use Chrome or Edge on a computer.
2. Open `flash.html` (from the host at `/flash.html`, or from the project's Pages site).
3. Choose your board and follow the page.

- **ESP32-S3 (Heltec V4, XIAO ESP32-S3):** connect by USB. If it will not connect, hold BOOT and press RST. Your identity, contacts and mailbox are kept unless you tick *erase the whole flash*.
- **nRF52840 (XIAO nRF52840, WisMesh Tag):** press *Start bootloader*, then *Flash over USB*. Fallback: double-press the reset button so a USB drive appears and copy the `.uf2` file onto it.

**Check:** the board reconnects and shows up in the web client (USB or Bluetooth).

## Option B: build and flash yourself

Windows:

```text
.\scripts\build-firmware.ps1 <board> -Flash
```

Linux:

```text
./scripts/build-firmware.sh <board> flash
```

- **nRF52840:** the script makes `dist/maarse-<board>.uf2`. Double-click the board's reset button and drag the file onto the USB drive that appears. USB comes back after about a minute: the first start after a USB flash hangs, and the watchdog restarts the board (from firmware 0.9.12). If it has not after two minutes, or with older firmware, press reset once.
- **ESP32-S3:** the first time, the script installs Espressif's toolchain (`espup`) and `espflash`, which takes several minutes. The port must be free: stop any host that uses the board as its modem first.

**Check:** connect with the web client over **USB / serial** (Chrome or Edge) or **Bluetooth**; the status page shows the firmware version.

## Update a board that already runs Må:rse

Use the web client's *Firmware* page (USB, Bluetooth or the network). Updates are
signed: a node installs only an image signed with its built-in release key, for
its own board, with a newer version. Firmware updates over LoRa are
`shelved` ([decision 0002](../decisions/0002-no-ota-over-lora.md)).

## If something goes wrong

| Symptom | What to try |
| --- | --- |
| ESP32 will not connect | Hold BOOT, press RST, retry |
| nRF52840 shows no USB drive | Double-press reset quickly; try another cable (some are charge-only) |
| COM port busy | Stop the host or any serial monitor using it |
| Nothing on the Heltec display | See the firmware status section of [GUIDE.md](../GUIDE.md#firmware-status-and-what-is-missing) |

Related: [Verification](../verification/README.md) · [Settings reference](../reference/settings.md)
