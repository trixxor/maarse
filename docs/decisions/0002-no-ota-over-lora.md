# 0002: No firmware updates over LoRa

- **Status:** shelved
- **Date:** 2026-11-14

## Context

Nodes in the field are hard to reach. Updating them over the radio would avoid a
trip.

## Options

1. Distribute signed firmware over LoRa, hop by hop.
2. Update only over USB, Bluetooth, Wi-Fi or the network (host pushes signed
   firmware).

## Decision

Option 2. The OTA code over LoRa is kept in the repository but not pursued.
Do not propose compression, delta updates or hardware tests of it unless the
decision is reopened.

## Consequences

- A node in a hard-to-reach place needs a visit, or a Wi-Fi/network link to a host.
- USB, Bluetooth and network updates stay supported and signed
  ([spec 6.7](../protocol-spec.md#67-firmware-update-signing)).

## Evidence

Firmware is 323 KB (nRF) and 602 KB (Heltec V4). At SF7 / 250 kHz with 10 % duty
cycle that is about 1.6 h (nRF) and 3 h (Heltec) **per hop**, and days at SF9.
The README and [compare.md](../../compare.md) mark it as shelved.
