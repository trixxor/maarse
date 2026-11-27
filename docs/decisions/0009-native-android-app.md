# 0009: Native Android app with the host embedded

- **Status:** accepted
- **Date:** 2026-11-17

## Context

The first Android app was a WebView around the web client. It had no
notifications, no foreground service and could not run a node of its own, and it
lagged behind features (post offices, alerts v2, many host settings).

## Options

1. Keep the WebView app.
2. A native Kotlin app that talks to a Må:rse host, with the full `maarse-host`
   running inside the app as a library through JNI.

## Decision

Option 2. The host runs in a foreground service in a process of its own (`:host`),
so a fatal error or a settings restart ends only that process. The UI talks to it
as the web client does: the companion protocol over WebSocket on `127.0.0.1:47080`,
with an access key (`--require-key`, because every app on a phone can reach
loopback). The phone is its own node with its own identity. Modes: radio over
Bluetooth only, own node, and server mode (only on Wi-Fi and charger; mobile data
has CGNAT).

## Consequences

- The Android app and the PC web client must stay level: every user-facing change
  is made in both (project rule, see `CLAUDE.md`).
- Gaps remain on Android: calls, parts of the map, TUN (VpnService), radio as modem
  for the phone node (see the status in [Verification](../verification/README.md)).
- Needs the NDK, `cargo-ndk` and the Android Rust targets to build
  (`tools/build_apk.py`).

## Evidence

Verified on an emulator 2026-11-17: phone node and PC host exchanged messages in
both directions with forward secrecy, receipts, notifications and host restart
via settings. Bluetooth mode and server mode were not tested on a real phone then.
Crate documentation: `maarse-android/src/lib.rs`.
