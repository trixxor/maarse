# MeshCore gateway firmware

This project turns a Heltec V4 into a remote LoRa radio for the Må:rse backbone. For first boot, Wi-Fi recovery, button behavior and TLS configuration, see [gateway setup](firmware/kiss_modem_wifi/BACKBONE.md). The [host guide](../../docs/GUIDE.md#connect-a-heltec-v4-radio) describes the listener and radio worker.

## Build on Windows

Install Git and PlatformIO CLI and make both available on `PATH`. From this directory, run the following in PowerShell to build both display variants.

```powershell
.\build.ps1
```

Use `-Environment heltec_v4_kiss_backbone` for OLED or `-Environment heltec_v4_kiss_backbone_tft` for TFT. The script fetches the pinned upstream MeshCore revision into the local `src` checkout on first run. The gateway configuration lives in `platformio.gateway.ini`. The application binaries are written to `src/.pio/build/<environment>/firmware.bin`.

The gateway can be configured through the setup portal after flashing. If a private `firmware/kiss_modem_wifi/backbone_config.h` is used for build-time defaults, keep it outside version control.
