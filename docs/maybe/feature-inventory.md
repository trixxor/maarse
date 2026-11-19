# Må:rse feature inventory

The outline for the full system description, and the checklist for the
Denmark simulator (docs/maybe/denmark-sim-v3-plan.md). Each feature says how
the simulator runs it: **real** (the real code runs in the simulator),
**shared** (rules shared with maarse-host through `maarse-hostrules`),
**model** (the simulator models it), or **not yet**.

Status as of 2026-11-19.

## Messages

| Feature | Code | Simulator |
|---|---|---|
| Direct messages, acks, textz compression | maarse-core router, content | real |
| Sequence numbers, "2 messages missing" | maarse-hostrules seq, maarse-host node/seq.rs | shared rule: numbered between hosts/app users, gaps shown on phones, Results |
| Public and private channels | router, companion | real (public + hashtag channels from phones; private: not yet) |
| Store-carry-forward, carried notice | router carry | real |
| Post offices: deposit, sync, replicas, quotas, expiry | maarse-core mail.rs, maarse-host node.rs | model on core rules; host-to-host over radio/trunks without TCP; retries per the host's rules |
| Hold messages from unknown senders | router | real |
| History shared by several screens | maarse-host history.rs | not yet |

## Files, voice and pictures

| Feature | Code | Simulator |
|---|---|---|
| File offer + pull/batch transfer, progress, cancel | maarse-host node.rs, maarse-crypto files | real/shared (step 4B, done) |
| Size limit per route (8 KB LoRa, 1 MB TCP), file mailbox | maarse-hostrules filebox | real/shared (step 4B, done) |
| Voice messages (Codec2 1200 bit/s, audio/x-codec2) | maarse-voice, Media.kt, web client | real/shared (step 4B, done) |
| Pictures (dots / grey / colour copies) | maarse-hostrules pages | real/shared (step 4B, done) |

## Calls

| Feature | Code | Simulator |
|---|---|---|
| Opus calls as datagrams, internet all the way | Calls.kt, web client, maarse-host | real/shared (step 4B, done) |
| Refusal when the path is not TCP all the way | maarse-hostrules calls | real/shared (step 4B, done) |
| Call test service (music, echo, speed) | maarse-host node/calltest.rs, maarse-hostrules calls::test | shared constants; music, echo and speed modes (Copenhagen host in generated maps) |

## Pages

| Feature | Code | Simulator |
|---|---|---|
| Page requests, deflated page files, MU markup | maarse-host node/pages.rs, Mu.kt, web client | real/shared (step 4B, done) |
| Locks, boards, notes | maarse-hostrules pagelock | shared: @allow/@locked per page and folder, groups per site, boards with notes (one a minute, 50 kept), PostBoard from phones |
| Pictures on demand, burst limits | maarse-hostrules pages | real/shared (step 4B, done) |

## Alerts

| Feature | Code | Simulator |
|---|---|---|
| Signed bulletins on #bulletin, priority | maarse-crypto bulletin, qos | real |
| Alerts v2: issuer key, certificate, replay drop | maarse-crypto alert, maarse-host node/alerts.rs | real (alert::sign/verify + ReplayGuard per node); forged and replayed alerts in Break |
| Area + region scope, heat map | sim layer | model |

## IP tunnel (VPN: SSH, mosh, ping)

| Feature | Code | Simulator |
|---|---|---|
| Session key, datagrams, header compression, MSS clamp, pieces + NACK | maarse-hostrules tunnel, maarse-host tunnel.rs | real/shared (step 4B, done) |
| Over TCP always, over LoRa with --tun-lora | maarse-host | real/shared (step 4B, done) |

## Identity and people

| Feature | Code | Simulator |
|---|---|---|
| Announces, contacts, pinned contacts | router | real |
| Key requests, host key cache | router, maarse-host | real |
| Linked devices (one person, many devices) | maarse-host node/person.rs | model (sync) |
| Recovery phrase, identity restore | maarse-crypto recovery | real derivation; lost device restored after N min with empty state, mail from post offices (Break, phone ↺) |
| Safety numbers | web client, app | not yet (phones get no identity yet) |

## Routing and radio

| Feature | Code | Simulator |
|---|---|---|
| Flooding, path requests, routes, local repair | router | real |
| Hello, regions, bordercast, area scope | router, maarse-host | real / model (bordercast, area scope) |
| Forest mode (auto relay), edge bridges, hop limit | router + sim | real / model (switching) |
| Mobile mode, anchors, host anchor | router | real |
| Load control, QoS priorities, SF slot | router, qos | real |
| Duty cycle, CAD, collisions, half duplex | maarse-core duty + sim | real duty, model radio |
| Path loss, height, horizon, clutter, frequency, bandwidth | maarse-sim radio | model; calibratable (radio_cal: path_n, offset_db) from field data with tools/calibrate_radio.py — not calibrated yet |
| Live change of SF / frequency / bandwidth / power | sim `radio` event | model |
| Duty-cycle sub-bands | maarse-core duty::eu_band_permille | real rule (sim caps duty by the channel's sub-band; firmware does not use it yet) |

## Backbone and links

| Feature | Code | Simulator |
|---|---|---|
| TCP backbone, region hello | maarse-host links.rs | model |
| DHT lookup | maarse-host dht.rs | model |
| Wi-Fi point to point, LoRa trunks, Bloom summaries | maarse-host, sim | model |
| Servers (VPS) without radio, NAT relay | maarse-host server | model (server kind; NAT relay not yet) |
| Self-healing: min-peers, DHT discovery, seed DNS, reconnect | maarse-host | model: min-peers reconnect (servers default 6); seed DNS not yet |
| Serial, KISS, UDP links | maarse-host extra_links.rs | not yet |
| Guard: rate limits, bans, block list | maarse-hostrules guard | shared: LinkRate per (host, sender) on internet links, bans; flood attack in Break (block list not yet) |

## Power

| Feature | Code | Simulator |
|---|---|---|
| Batteries, mains with battery fallback, power cuts | sim | model |
| Energy level drives flood delays | router | real |

## Clients and devices

| Feature | Code | Simulator |
|---|---|---|
| Android app (native Kotlin) | android/ | phones: Android look in progress |
| Web client | maarse-client | real (in the phones) |
| Firmware: Heltec V4, XIAO nRF52840, Heltec display | firmware/ | router real; display not shown |
| Phone ↔ radio over Bluetooth / USB, link drops | app, firmware | model: link drops (see below) |
| Range test, trace route, neighbours, remote admin | router, companion | neighbours real; others not yet |
| Position and telemetry sharing | appdata | not yet |
| Firmware update (USB/BT only) | firmware, web flasher | not simulated (shown as reboot) |
| MeshCore gateway, MQTT bridge | maarse-host | not yet |

## The simulator itself

| Feature | Status |
|---|---|
| Generator per region with connection mix (LoRa / both / app), cars, relays | done |
| Devices: handheld, relay, repeater, anchor, host, server, Wi-Fi, car, app phone, person with route | done |
| Routes with waypoints, pauses, start time, end mode; mobile / stationary | done |
| Follow a message live (status, where it is now, journey) | done |
| Track nodes on a watch list (contact, battery, messages) | done |
| Normal-day traffic curve | done |
| Save runs, snapshots, branches, share links | done |
| CPU limits + benchmark | done |
| Presets with expected results, checked in the test | done (3 presets, 1 known gap) |
| Phones running the real web client | done |
| Results: delivery time, busiest nodes, loss analysis, compare runs, packet log | done |
| Rule profiles "Standard (as built)" | waiting for step 4B |

| Phone ↔ radio Bluetooth link drop | firmware ble.rs (old frames thrown away on connect) | model: events while down are lost to the phone, counted; phone window ᛒ, Results |
