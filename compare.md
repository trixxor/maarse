LoRa mesh · status November 2026

# Meshtastic, MeshCore, Reticulum and Må:rse

Four ways to send messages over LoRa without a mobile network. This page compares them side by side, gives pros and cons, and ends with what Må:rse is missing and what it should have.

## In short

GPL-3.0 · C++

### Meshtastic

The largest and easiest. All nodes repeat all packets (managed flood), at most 7 hops. The stable version is 2.7; 2.8 with signed packets is in alpha.

MIT · C++

### MeshCore

Only repeaters forward. The first message is flooded, after that it goes along a learned path. Up to 64 hops, regions and remote control of repeaters.

Own licence · Python

### Reticulum

A whole network stack that runs over anything from LoRa to the internet. Strongest on cryptography and global reach, but needs a computer or phone.

MIT/Apache-2.0 · Rust

### Må:rse

Our prototype (v0.5). Reticulum's identity and global reach, MeshCore's repeater routing, run directly on the microcontroller. Runs on hardware with a gateway to MeshCore, an Android app and remote access over VPN; range and power have not been measured.

## Side by side

Må:rse is judged on what is built and tested in v0.4, not on what is in the specification.

yesworkspartlywith limitationsnodoes not exist

|  | Meshtastic | MeshCore | Reticulum | Må:rse |
| --- | --- | --- | --- | --- |
| Routing |  |  |  |  |
| Principle | Managed flood: everyone repeats, with a delay by signal strength | Flood the first time, then a source-routed path via repeaters | Announces spread; every transport node remembers the next hop | Flood with a path request, then a source-routed path; the best path is chosen by SNR |
| Max hops | 7 (default 3) | 64 | 128 | 64 in total; over 12 with anchor repeaters |
| Who forwards | Everyone (roles like ROUTER, ROUTER_LATE, CLIENT) | Only repeaters (clients can turn on "repeat") | Nodes with transport enabled | Repeaters; forest mode lets clients help for up to 4 hops |
| Acknowledgement | yesfor direct messages | yesend to end | yesproof of delivery | yesper hop and end to end, with route repair |
| Duty cycle (EU) | yes | yestoken bucket from v1.14.1 | yesper interface | yestoken bucket + listen before talk (CAD) |
| Moving nodes | yesflood does not care about topology | partlypaths must be learned again | partlypaths are updated via announces | yesmobile flag and route updates; 100 % in the motorway simulation |
| Reach beyond LoRa |  |  |  |  |
| Internet / TCP | partlyMQTT bridge via a server | noonly via third-party bridges | yesTCP, UDP, I2P, serial etc. in the same net | yesTCP backbone with DHT, MQTT bridge and gateway to MeshCore (tried on hardware) |
| Finding someone in another country | Via a shared MQTT server | No | Announces over public transport nodes | Lookup in the DHT, then a direct connection to the recipient's gateway |
| Security |  |  |  |  |
| Identity | Node number + key (PKC since 2.5) | Ed25519 key; short prefix as address | Key pair; address = 16-byte hash | Key pair; address = 16-byte hash; no sender in clear text |
| Direct messages | yesX25519 + AES | yesencrypted, compact | yesX25519 + AES-256, forward secrecy on links | yesX25519 + ChaCha20-Poly1305, new key per message; forward secrecy (draft) |
| Signed packets | partlycoming in 2.8 (alpha) | partlyadverts are signed | yesannounces and proofs | partlyannounces and firmware are signed |
| Spam protection | partlytraffic limits | partlycap on flood hops | yes"stamps" (proof of work) from 1.6 | yesproof of work on announces and path requests |
| Features |  |  |  |  |
| Public channels | yes | yesalso #hashtag and regions | partlygroups via apps | yespublic, #hashtag, private channels and regions |
| Messages for offline users | partlyStore & Forward module | yesroom servers | yesLXMF propagation nodes | yesmailbox on the host and in flash on nodes; the phone can use the home host over VPN |
| Files and pictures | no | no | yesresources over links | yesup to 8 KB over LoRa (also on firmware), 1 MB over the internet |
| Pages / services | no | partlyroom servers, sensors | yesNomadNet pages | yes`maarse://` pages from maarse-host, up to 8 KB compressed over LoRa |
| Voice | no | no | yescalls in Sideband | yesvoice messages (Codec2) and calls (Opus over TCP) |
| Remote administration | yesadmin keys | yeslogin on repeaters | partlyrnstatus, remote control | yesstatus and radio on other nodes, administrators only |
| Hardware and maturity |  |  |  |  |
| Runs on a microcontroller | yesESP32, nRF52, RP2040 and more | yesESP32, nRF52, RP2040 and more | noRNode is only a modem; Rust versions are young | yesnRF52840 and ESP32-S3 (4 boards) |
| Apps | Android, iOS, web, CLI | Official app (closed source) + web, third party | Sideband, NomadNet, MeshChat | Web client (PWA) and Android app (Bluetooth to the node or maarse-host over VPN with an access key) |
| Maturity | Large user base, many years | Growing fast, frequent releases | 1.0 in 2025, now 1.6 | Prototype; 187 tests, browser test, fuzzing, simulator and test lab; signed releases; tried on 2 boards |
| Licence | GPL-3.0 | MIT | MIT with added clauses since 2025; an AGPL fork exists | MIT or Apache-2.0; spec under CC-BY |

## Pros and cons

### Meshtastic

#### Pros

- By far the most users and nodes on the air.
- Many apps and boards, easy setup.
- Flood works without planning, also when everyone moves.
- Store & Forward, telemetry, GPS and MQTT are built in.

#### Cons

- Flood uses a lot of airtime and scales badly in large nets.
- At most 7 hops.
- Channels are based on a shared key; packets are only signed from 2.8.
- Internet only via MQTT servers, not as part of the net.

### MeshCore

#### Pros

- Efficient routing: only repeaters send, and fixed paths save airtime.
- Up to 64 hops; regions keep channels local.
- Good tools for repeaters: remote control, neighbours, noise floor.
- MIT licence and active development.

#### Cons

- No built-in connection beyond LoRa.
- Short address prefixes can collide (hence paths with 1–3 bytes now).
- Depends on someone having set up repeaters.
- The official app is not open source.

### Reticulum

#### Pros

- The strongest cryptography and identity model of the four.
- The same net over LoRa, TCP, I2P and more: reaches globally.
- The most services: messages, files, pages, voice.
- Spam protection with proof of work from 1.6.

#### Cons

- Needs Python on a computer or phone; does not run on a LoRa chip by itself.
- Announces and path lookups take a lot of slow LoRa airtime.
- The licence got extra clauses in 2025, and the founder stepped back the same year, so the future is unclear.
- More technical to set up.

### Må:rse

#### Pros

- Runs entirely on the microcontroller, without heap, in Rust.
- Repeater routing with the best path, per-hop acknowledgement and route updates. In simulation it uses about a third of the flood's airtime.
- Global reach: TCP backbone and DHT, without a central server, gateway to MeshCore and IP tunnel.
- No sender in clear text, 16-byte addresses and modern encryption.
- Mailbox, files, private channels, signed over-the-air updates and gateway to MeshCore already in the prototype.

#### Cons

- Only a first test on hardware; range, power and many features have not been tried on the air.
- No users yet; the Android app is new and only tried in an emulator and over Wi-Fi.
- The cryptography has not been reviewed by others.
- Some planned parts are missing (map, gateway to Reticulum), and the phone cannot be its own node yet.

## What Må:rse is missing, and what it should have

"Missing" are things we have decided on but not built. "Should be added" are new proposals, mostly borrowed from the other three.

### Missing (decided, not built)

Sorted by importance.

HIGH**More testing on hardware**The first test has passed (Heltec V4.3 and XIAO nRF52840 + Wio-SX1262). Missing: range, power consumption, Bluetooth on the ESP32-S3, files and updates over Bluetooth in practice (updating over LoRa is shelved: too slow), plus the WisMesh Tag and XIAO ESP32-S3.

HIGH**Gateway to Reticulum**The gateway to MeshCore is built and tried on hardware. Reticulum is missing.

MEDIUM**Node and coverage map**A map of nodes and measured coverage, like MeshMapper.


### Should be added (open proposals)

HIGH**External security review**Fuzzing has been done with no findings; the crypto, especially forward secrecy (`maarse-crypto/src/ratchet.rs`), must be looked at by others before v1.0.

MEDIUM**The phone as its own node**The Android app exists (Bluetooth and maarse-host over VPN). Next step: maarse-host in the app, so the phone has its own identity on the TCP backbone; then the Bluetooth radio as a modem. iOS is missing.

LOW**Sensors**Battery, uptime, energy status and chip temperature (nRF52840) are sent; the MQTT bridge has Home Assistant discovery. External sensors (weather) and temperature on the ESP32-S3 are missing.

### Done since the first version

- **Hardware:** Heltec V4.3 and XIAO nRF52840 send messages with acknowledgement both ways; the PA chip on the Heltec V4 is detected automatically.
- **Spam protection:** proof of work on announces (12 bits) and path requests (8 bits).
- **Regions for channels:** `name@region`, a region list per node, filtering on LoRa and the backbone.
- **Key verification:** safety number (60 digits) and QR code.
- **Private channels:** random key, invitation as a link and QR code.
- **Fuzzing** of all decoders and `forbid(unsafe_code)`.
- **Forward secrecy (draft):** a ratchet per contact, 5 extra bytes per message.
- **Fewer lost acknowledgements** in large, sparse nets.
- **Bluetooth on the ESP32-S3** (Nordic UART Service).
- **Battery measurement and energy-driven routing** on firmware.
- **Files and pictures on firmware** (up to 8 KB) and a file mailbox on the host.
- **Diagnostics:** trace route with SNR, neighbour list and noise floor.
- **Position private by default**, rounded to 10 km or 1 km.
- **MQTT bridge** with Home Assistant discovery.
- **Gateway to MeshCore** in maarse-host, tried on hardware (Heltec V4.3 with `companion_radio_usb`).
- **Display names** in announces, also shown in the MeshCore bridge as `[Må Dennis]`.
- **`maarse://` pages:** compressed markup from `<data>/pages/`, a browser with a cache in the web client. Not HTML, so foreign pages cannot run code.
- **Signed firmware updates** over USB, Bluetooth and the net, with anti-rollback.
- **Web client as a PWA**, history on several screens, recovery phrase (24 words) and encrypted backup.
- **maarse-lab:** test lab in the browser with a live simulator.
- **Voice messages** (Codec2, 1200 bit/s) and **calls** (Opus, TCP only).
- **Anchor repeaters:** paths over 12 hops.
- **IP tunnel** (IPv6 over TCP, Wintun/TUN).
- **Signed releases:** firmware, host and APK with a release key and SHA256SUMS.sig.
- **New design of the web client** and an **Android app** (Bluetooth to the node).
- **Remote access with an access key:** the app and browsers can connect to maarse-host over VPN or the home network; tried from an Android phone.
- **Chip temperature** in the telemetry (nRF52840).
- **Role** (client, repeater, relay) stored in flash.

## Where does Må:rse stand?

On paper Må:rse has the best combination: Reticulum's identity and global reach, MeshCore's efficient routing, and everything runs on the chip. But the other three have what counts most in practice: users, apps and years of operation.

Since the first version the firmware has gone on the air, the gateway to MeshCore works, and the phone can use the net via Bluetooth or via the home host over VPN. The next steps are to measure range and power, try updates, files and Bluetooth on real boards, let the phone become its own node and get the crypto reviewed by others.

Sources (fetched November 2026):

- [The Meshtastic blog](https://meshtastic.org/blog/) (2.7 and 2.8-alpha)
- [Meshtastic 2.7 preview](https://meshtastic.org/blog/meshtastic-2-7-preview/) (key verification)
- [MeshCore v1.16.0](https://blog.meshcore.io/2026/06/06/release-1-16-0)
- [MeshCore v1.14.1](https://meshcoreeuropeeu.pages.dev/en/blog/2026-04-04-meshcore-v1-14-1-adds-duty-cycle-control-and-device-support) (duty cycle, paths)
- [Reticulum RNS 1.6.0](https://newreleases.io/project/github/markqvist/Reticulum/release/1.6.0)
- [FOSDEM 2026: Reticulum, what's next](https://fosdem.org/2026/events/attachments/9NCWUR-reticulum_community_meetup_implementations_migration_and_future/slides/262533/reticulum_dimz1j8.pdf) (licence and forks)

Må:rse numbers come from the project's own tests and simulator (v0.5).
