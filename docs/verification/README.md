# Verification: what is proven, and how

> **Type:** reference · **Status:** written 2026-11-27 from the repository, the test tools and the project's own notes. Update it whenever a test is run or a claim changes. Honest labels are defined on the [index page](../index.md#status-labels).

## The short version

- **Nothing is `field-tested`.** No range, loss or power measurement has been made
  outdoors yet. The procedure exists: [FIELD-TEST.md](../FIELD-TEST.md).
- Behavior is checked by **automated tests**, **fuzzing**, **simulators**, and
  **bench tests** on two boards on a desk (Heltec V4, XIAO nRF52840).
- Simulator results use a log-distance path-loss model (exponent 3); real radio is
  messier. Comparisons with Meshtastic, MeshCore and Reticulum come from their
  documentation and from the simulator, not from measurements.
- **The cryptography has not been reviewed externally.**

## Kinds of evidence

| Kind | What it is | Where |
| --- | --- | --- |
| **tested (automated)** | Unit, integration and end-to-end tests run on one computer, plus JavaScript-equals-Rust parity tests | `cargo test --workspace`, `tools/e2e_test.py`, `tools/dht_test.py`, `tools/*_test.*` ([How to build and test](../guides/build-and-test.md)) |
| **fuzzed** | Deterministic fuzzing of parsers and crypto, reproducible by seed | `maarse-fuzz`, `maarse-host/src/fuzz_tests.rs`; compact frame: 3 million iterations |
| **simulated** | The real `Router` in many virtual nodes | `maarse-sim`, the [Denmark simulator](../denmark/README.md), `tools/sim-experiments/` |
| **bench-tested** | Real boards, cable on a desk | notes in the project; flashed Heltec V4 and XIAO nRF52840 |
| **reviewed** | Written review | [security-audit-2026-11-20.txt](../security-audit-2026-11-20.txt) (internal), `docs/maybe/*review*.html` |
| **field-tested** | Outdoors over real distances | none yet |

## Status by feature

`working copy` unless stated. Entries marked *not on hardware* are only verified by tests and simulation.

| Feature | Status | Notes |
| --- | --- | --- |
| Direct messages with delivery mark | tested (automated), bench-tested | e2e test; two boards |
| Forward-secrecy ratchet | tested (automated), fuzzed | **draft**, not externally reviewed |
| Compact FS frame (`0xF2`) | tested (automated), fuzzed, simulated | firmware change *not on hardware*; stack probe needed before flashing |
| Routing (path discovery, per-hop ack, repair) | simulated, bench-tested on two boards | not field-tested |
| Carry and delivered notice | tested (automated), simulated | *not on hardware* |
| Host post office | tested (automated) | `tools/post_office_test.py`, e2e |
| LoRa trunks between post offices | simulated | |
| Backbone DHT | tested (automated) | `tools/dht_test.py`; DHT check tool in the working copy |
| Expiry of published addresses and moved nodes | working copy, unit tests | `maarse-host/src/node/reach.rs`, `dht.rs`; *not field-tested* |
| Files over LoRa (pull/batch transfer, progress) | bench-tested | measured 157 to 66.6 s in one case; not field-tested |
| Pages (`maarse://`), alerts | unit tests; page limits covered by regression tests ([audit](../security-audit-2026-11-20.txt)) | not on hardware |
| Linked devices | tested (automated) | `tools/linked_devices_test.py`; *not flashed, not hardware-tested* |
| Sensors catalog and designer | working copy | `tools/sensor_catalog.py` |
| Spectrum recordings | bench-tested (Heltec over USB) | remote recording over LoRa untested |
| Heltec display and button settings | working copy | preview tool `tools/display_preview.py`; confirm on the board |
| Firmware update from the host (USB, Bluetooth, Wi-Fi) | working copy | *not flashed* |
| Web flasher | live on the Pages site | not tested on every board |
| Android app (native) | emulator-verified | Bluetooth mode on a real phone not tested; some features missing (see [decision 0009](../decisions/0009-native-android-app.md)) |
| Windows service, tray, installer | working copy | signed updates; SmartScreen warning accepted |
| ESP32 Wi-Fi hotspot gateway | working copy | `tools/hotspot_hw_test.py` exists; the feature is in progress |
| Network test tools (ping, crawl, DHT check) | working copy | `tools/ping_test.mjs`, `crawl_test.mjs`, `dht_check_test.mjs`; Android pages not run on devices |

## Simulator results worth knowing

- The simulator was optimistic about the old frame. With an honest overhead model
  the car scenario went from 954 s and 97 % to 2426 s and 81 %
  ([decision 0011](../decisions/0011-airtime-round.md)).
- Load control and quotas were measured and rejected
  ([decision 0004](../decisions/0004-load-control-rejected.md)).
- A drop in national and outage scenarios was bisected to a host mail change and is
  a likely model gap; it has not been fixed.

## Documentation checks

`tools/docs_check.py` runs in CI (`.github/workflows/docs.yml`) and locally:

```text
python tools/docs_check.py
```

It checks that

1. every relative Markdown link and anchor in `docs/` and the root pages resolves;
2. every page in the new structure starts with a title and a `Type` line;
3. the generated [settings reference](../reference/settings.md) is up to date when a
   `maarse-host` binary is available (`tools/gen_settings_reference.py --check`).

Still missing: a check that constants in the specification equal the constants in
`maarse-core`, and that code examples in the pages run.

## When you change something

Update this page when a test is added, run on hardware, or a claim changes label.
Do not write "works" about something that has only run in the simulator.
