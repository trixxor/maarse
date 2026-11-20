# MeshCore gateway firmware

This project turns a Heltec V4 into a remote LoRa radio for the Må:rse backbone. For first boot, Wi-Fi recovery, button behavior and TLS configuration, see [gateway setup](firmware/kiss_modem_wifi/BACKBONE.md). The [host guide](../../docs/GUIDE.md#connect-a-heltec-v4-radio) describes the listener and radio worker.

## Flash from the browser

Releases from v0.7.6 build both display variants. Open the web flasher (`flash.html`, published with each release), choose **Heltec V4 MeshCore gateway (OLED)** or **(TFT)** and press **Flash**. The file is `maarse-heltec-v4-gateway.bin` or `maarse-heltec-v4-tft-gateway.bin`: one merged image with bootloader, partition table, `boot_app0` and the app, written from 0x0.

The flasher first reads the partition table on the board. When it matches the new image, the NVS partition is skipped, so the gateway keeps its Wi-Fi and backbone settings. Otherwise, for example when the board ran Må:rse firmware before, everything is written and the gateway starts in setup mode. The MeshCore LoRa identity lives in a separate partition beyond the written part. **Erase the whole flash first** removes the settings and the identity. MeshCore's MIT license is published next to the images as `meshcore-license.txt`.

## Build on Linux

`build.sh <environment> <output.bin>` builds one variant and writes the merged image; the release workflow uses it. It needs Git, PlatformIO and esptool 4.x.

```sh
firmware/meshcore-gateway/build.sh heltec_v4_kiss_backbone dist/maarse-heltec-v4-gateway.bin
```

## Build on Windows

Install Git and PlatformIO CLI and make both available on `PATH`. From this directory, run the following in PowerShell to build both display variants.

```powershell
.\build.ps1
```

Use `-Environment heltec_v4_kiss_backbone` for OLED or `-Environment heltec_v4_kiss_backbone_tft` for TFT. Both scripts fetch the upstream MeshCore revision pinned in `MESHCORE_REVISION` into the local `src` checkout on first run. The gateway configuration lives in `platformio.gateway.ini`. The application binaries are written to `src/.pio/build/<environment>/firmware.bin`.

The gateway can be configured through the setup portal after flashing. If a private `firmware/kiss_modem_wifi/backbone_config.h` is used for build-time defaults, keep it outside version control.
