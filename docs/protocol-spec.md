# Må:rse protocol specification

Version: describes the code on the `main` branch as of November 2026
(packet format version 0, stamp version 1, companion protocol version 2).

Licence: this specification is licensed under
[Creative Commons Attribution 4.0 International (CC-BY 4.0)](https://creativecommons.org/licenses/by/4.0/).
The code it describes is under MIT or Apache-2.0.

The code is the source of truth. Every constant below names the file it is
defined in; if this document and the code disagree, the code wins and the
document is wrong. Places where the code and `docs/GUIDE.md` disagree are
listed in [Notes for maintainers](#notes-for-maintainers).

> **Security status.** The cryptography (sections 6 and 7) has **not been
> reviewed by an external cryptographer**. The forward-secrecy ratchet is
> marked as a draft in its own source (`maarse-crypto/src/ratchet.rs`). Do not
> rely on it for anything where a break would cause harm.

## Contents

1. [Overview and terminology](#1-overview-and-terminology)
2. [Addresses and path IDs](#2-addresses-and-path-ids)
3. [Packet format](#3-packet-format)
4. [Announces, app data and proof of work](#4-announces-app-data-and-proof-of-work)
5. [Routing](#5-routing)
6. [Cryptography](#6-cryptography)
7. [Content of direct messages, files and fragments](#7-content-of-direct-messages-files-and-fragments)
8. [Mailboxes and the backbone](#8-mailboxes-and-the-backbone)
9. [Companion protocol](#9-companion-protocol)
10. [Versioning and compatibility](#10-versioning-and-compatibility)
11. [Notes for maintainers](#notes-for-maintainers)

Conventions: byte counts are in parentheses, `|` separates consecutive
fields, `BE` is big endian and `LE` is little endian. Packet header fields on
the air are big endian; most payload integers and all companion frame
integers are little endian. Each layout states which.

---

## 1. Overview and terminology

Må:rse is a mesh network for LoRa radios with an optional internet backbone.
Every participant has one 16-byte address that is the same over LoRa, TCP and
the backbone. Direct messages are source-routed along a path found by a
flooded path request; channel messages, announces and path requests are
flooded.

| Term | Meaning |
| --- | --- |
| **node** | Any participant with an identity: a firmware radio node or a `maarse-host`. |
| **client** | (1) The router role `Client` (0): a node that does not forward for others (mobile, or on battery without charging). (2) A user interface (web client, Android app) that talks to a node over the companion protocol (section 9). The meaning is clear from context. |
| **repeater** | Role `Repeater` (1): a stationary node with a positive energy budget that forwards floods and source-routed packets. |
| **relay** | Role `Relay` (2): a handheld node in "group mode" that forwards like a repeater, but waits longer and only up to `relay_hops` hops. Relays carry undeliverable messages (store-carry-forward). |
| **anchor** | Role `Anchor` (3): a fixed repeater that also splits paths longer than 12 hops into legs (section 5.8). |
| **host** | `maarse-host`, a node program for PCs. It routes like a node, connects to other hosts over TCP (the backbone), can use a radio node as a LoRa modem, keeps mailboxes and serves the web client. |
| **backbone** | TCP links between hosts (and Wi-Fi-connected ESP32 nodes), plus a DHT for finding which host can reach an address. |
| **region** | A named area (e.g. `dk`) carried as a 16-bit ID on floods, so channel traffic can be kept local (section 5.12). |
| **channel** | A group conversation identified by the hash of a 32-byte channel key. Anyone with the key can read and write (section 6.4). |
| **path ID** | A 2-byte identifier of a forwarding node inside a source path. |

Role numbers are defined by `Role::as_u8` in `maarse-core/src/router.rs`.

---

## 2. Addresses and path IDs

Source: `maarse-core/src/addr.rs`, `maarse-crypto/src/lib.rs`.

```text
Address  = 16 bytes
PathId   = u16 = first two bytes of the address, read big endian
```

* A **node address** is the first 16 bytes of
  `SHA-256(Ed25519 public key (32) | X25519 public key (32))`
  (`PublicIdentity::address`).
* A **channel address** is the first 16 bytes of `SHA-256(channel key)`
  (`channel_address`).
* A **token address** is any 16-byte value used as a destination for replies
  so that the replier's target is not named: path-request reply tokens
  (random), acknowledgement tokens (`ack_token`, section 5.5) and trace tokens.

A path ID only has to be unique among a forwarding node's neighbours, not
globally. A collision makes two neighbours both believe they are the next hop;
the duplicate filter limits the damage.

---

## 3. Packet format

Source: `maarse-core/src/packet.rs`, `maarse-core/src/lib.rs`.

### 3.1 Layout

```text
+-------+------+-------------+----------------------------+-------------+---------+
| flags | hops | destination | path (optional)            | region (opt)| payload |
| 1     | 1    | 16          | count (1) + 2 × count (BE) | 2 (BE)      | rest    |
+-------+------+-------------+----------------------------+-------------+---------+
```

Flags byte:

| Bits | Field | Value |
| --- | --- | --- |
| 7–6 | version | `VERSION = 0` |
| 5–2 | packet type | 0–11, see 3.2 |
| 1 | path present | `FLAG_HAS_PATH = 0b10` |
| 0 | region present | `FLAG_REGION = 0b01` |

* `hops`: number of hops taken so far. For source-routed packets it is also
  the index of the next forwarder in the path (section 5.4).
* `destination`: a node address, channel address or token address. The
  sender is **never** in the header.
* `path`: present when bit 1 is set. A count byte (at most `MAX_PATH` = 12)
  followed by that many path IDs, each u16 BE. A count of 0 is valid (used by
  a fresh path request).
* `region`: present when bit 0 is set; u16 BE, placed after the path, or after
  the destination if there is no path. A region of 0 means "no region" and is
  never encoded (the flag is cleared).
* `payload`: the rest of the packet. Any AEAD tag is part of the payload.

Fixed header length `FIXED_HEADER = 18`. The full header length is
`18 + (path ? 1 + 2·n : 0) + (region ? 2 : 0)`.

### 3.2 Packet types

| Type | Name | Header path | Payload | Notes |
| --- | --- | --- | --- | --- |
| 0 | `Data` | source path | sealed direct message (section 6) | acknowledged end to end |
| 1 | `Ack` | source path (reverse) | empty | destination = `ack_token(msg_id)` |
| 2 | `Announce` | none | signed announce + stamp (section 4) | destination = announcing address; flooded |
| 3 | `PathRequest` | record route | reply token (16) \| optional `Data` payload \| stamp (5) | flooded |
| 4 | `PathResponse` | source path (reverse of record) | count (1) \| count × PathId (BE) | destination = reply token |
| 5 | `Control` | – | – | defined, not sent by current code (see Notes) |
| 6 | `Fragment` | source path | file chunk (section 7.3) | acknowledged end to end |
| 7 | `Probe` | source path | trace route (section 5.13) | |
| 8 | `Channel` | none | sealed channel message (section 6.4) | flooded, optional region |
| 9 | `Deposit` | source path | recipient address (16) \| sealed message | to a mailbox; acknowledged end to end |
| 10 | `Datagram` | source path | IP tunnel datagram | per-hop acks only, no end-to-end ack |
| 11 | `Carry` | one-element path, or none | carry handoff or delivered notice (section 5.10) | never flooded |
| 12 | `Hello` | none, `dest` = sender | `role (1) \| flags (1) \| chosen bridge (PathId u16 BE, 0xFFFF = none)` | never forwarded (`hops` 0) |
| 13 | `KeyRequest` | none, `dest` = the node asked for | `requester (PathId u16 BE) \| nonce (u16 BE)` | local flood (`LOCAL_SCOPE`) |
| 14 | `MailSync` | none, `dest` = the asker | `home post office (PathId u16 BE) \| have IDs (u32 BE, at most 8) \| nonce (u16 BE)` \| stamp (5) | flood of `MAIL_SCOPE` = 2 hops (section 8.1) |
| 15 | `Summary` | none, `dest` = the sender | Bloom filter (128 bytes, `bloom::BYTES`) | trunk summary; never forwarded (`hops` 0) |

**Hello** is sent on a Trickle timer (RFC 6206): 10 s after something it
says changed (role, "hears a repeater", chosen bridge, motion, or for a moving
node the repeater it hears), then at doubling intervals up to 4 min for
repeaters and anchors, 10 min for relays and nodes without coverage, and
30 min for covered clients. Receivers update the neighbor table (role,
coverage, bridge choice, mobile flag); bridge rules count a neighbor's hello
information for 35 min (`INFO_WINDOW_MS`). It carries no identity and no
proof of work, so neighbors learn about role changes in seconds instead of
from announces 12 h apart.

**KeyRequest** asks "who is `dest`?". Every node delivers it to its app: the
node asked for announces (at most every 30 s), and a host answers with the
announce it keeps for `dest` when the request came over its radio, sending it
on the radio only. Repeaters and relays forward it like a channel message.

**Summary** is sent by a host every 10 minutes on each long-distance LoRa
trunk (`--trunk`): a Bloom filter (`maarse-core/src/bloom.rs`, 1024 bits,
3 hashes from a mixed FNV-1a of the address) of the nodes heard on its side
by radio. The far end lets a path or key request across the trunk only for a
destination in the filter (or on a second request within the repeat window),
so requests for nodes on the near side stay off the trunk. A summary older
than 30 minutes is ignored. About 2 % false positives at 100 addresses; a
false positive only lets one request cross.

All 16 packet types (0–15) are now assigned. A `KeyRequest` payload now ends
with a stamp too (section 4.3).

### 3.3 Limits

| Constant | Value | Source |
| --- | --- | --- |
| `MAX_PACKET` | 255 bytes (largest LoRa packet) | `lib.rs` |
| `MAX_HOPS` | 64 (decoder rejects `hops > 64`) | `lib.rs` |
| `MAX_PATH` | 12 hops (24 bytes of path) | `lib.rs` |
| `LOCAL_SCOPE` | 8 hops for flooded channels, announces and probes | `lib.rs` |
| `MAX_DIRECT_PAYLOAD` | 255 − 18 − 1 − 24 = **212** bytes | `router.rs` |

`MAX_DIRECT_PAYLOAD` is the payload budget for any source-routed packet, so a
message still fits when the path is full. A channel message without a region
may carry up to 255 − 18 = 237 bytes of payload (235 with a region).

### 3.4 Decoding rules

A receiver rejects a packet (and counts it as invalid) if: it is longer than
255 bytes or shorter than 18; version ≠ 0; the type is unknown; `hops > 64`;
the path count is > 12; or the path or region runs past the end of the buffer.

### 3.5 Packet ID and duplicate filter

Source: `maarse-core/src/dedup.rs`.

```text
packet_id = FNV-1a-64(type (1) | destination (16) | payload)
            offset 0xcbf29ce484222325, prime 0x00000100000001b3
```

Hops, path and region are excluded so that copies along different routes have
the same ID. For a `Probe` trace, only the fixed part of the payload is hashed
(section 5.13). The router keeps the last `D` IDs in a ring buffer
(`DedupCache`). The **message ID** returned to applications is the packet ID
of the `Data` (or `Fragment`, `Deposit`) packet.

---

## 4. Announces, app data and proof of work

### 4.1 Announce payload

After verifying the signature, the host checks the signed Unix timestamp
against its clock and its persistent per-destination timestamp. The window
runs from seven days in the past to ten minutes in the future. A zero
timestamp remains available for clockless firmware when no recent dated
announcement from that identity is known. Equal timestamps can recur. This
is a receiver policy and preserves the wire encoding.

Source: `maarse-crypto/src/lib.rs` (`Identity::announce`, `verify_announce`).

Sent as packet type 2 with destination = the announcing node's address, no
path, flooded within `LOCAL_SCOPE`.

```text
offset 0   Ed25519 public key (32)
       32  X25519 public key (32)
       64  timestamp (u32 BE, Unix seconds; 0 if the node has no clock)
       68  app data length n (1)
       69  app data (n)
   69+n    Ed25519 signature (64)
  133+n    stamp (5)                       -- section 4.3, stripped before verification
```

Signature input: `"maarse/announce/v0" | address (16) | bytes 0 .. 69+n`.

Verification (`verify_announce`): the payload (stamp removed) must be exactly
`69 + n + 64` bytes; the address computed from the keys must equal the packet
destination; the signature must verify.

| Constant | Value | Source |
| --- | --- | --- |
| `PUBLIC_IDENTITY_LEN` | 64 | `maarse-crypto/src/lib.rs` |
| `SIGNATURE_LEN` | 64 | same |
| `ANNOUNCE_FIXED` | 69 (keys + timestamp + length byte) | same |
| `MAX_APP_DATA` | 255 − 18 − 69 − 64 − 5 = **99** | same |

Announce intervals are implementation choices. `maarse-host` announces every
600 s (`ANNOUNCE_EVERY`, `maarse-host/src/node.rs`). The firmware announces
5-35 s after a start (at random), then every 15 minutes
(`RadioConfig::BOOT_ANNOUNCE_S`) until it hears a repeater or an anchor, and
from then on at the interval in the radio settings
(`RadioConfig::announce_interval_s`, default 12 h), each time ±10 % at
random (`firmware/common/src/node.rs`).

### 4.2 App data fields

Source: `maarse-core/src/appdata.rs`.

```text
0x01 | address (16)                         mailbox       (fixed length, no length byte)
0x02                                        FS capable    (no data, no length byte)
0x03 | len | precision (1) | lat (i32 LE) | lon (i32 LE)                 position (len 9)
0x04 | len | battery mV (u16 LE) | flags (1) | energy (1) | uptime s (u32 LE) | temp (i16 LE, 0.1 °C)
                                                                         telemetry (len 10)
0x05 | len | UTF-8 name (at most MAX_NAME = 24 bytes)                    display name
0x06 | len | role (1) | flags (1) [| bridge (PathId, u16 BE)]          role (len 2, or 4 with the chosen edge bridge)
0x07 | len | capabilities (1)                                           capabilities (len 1)
```

Capabilities (`APP_CAPS`): bit 6 `CAP_COMPACT` (reads compact forward-secrecy
messages, section 6.3), bit 0 `CAP_TEXTZ` (reads compressed text, content
type `TEXT_Z`, section 7.1), bit 1 `CAP_BUNDLE` (reads mail bundles,
`OP_BUNDLE`, section 8.1). A sender compresses text and bundles mail only for
a recipient whose announce has the bit; older nodes skip the field by its
length.

**Announce scope.** Forwarders pass an announce on at most
`ANNOUNCE_SCOPE` = 3 hops (`Router::announce_scope`), except an announce
whose role flags carry `ROLE_MOBILE` (read from the app data at byte 69 of
the payload), which keeps `LOCAL_SCOPE` = 8. Nodes further away ask for a
key with a `KeyRequest` when they need it, answered by the node or a host's
announce cache. In maarse-sim this cut airtime by 16–32 % in every scenario
with the same or better delivery (docs/denmark/README.md).

| Constant | Value |
| --- | --- |
| `APP_POSTBOX` | 0x01 |
| `APP_FS` | 0x02 |
| `APP_POSITION` | 0x03 |
| `APP_TELEMETRY` | 0x04 |
| `APP_NAME` | 0x05 |
| `APP_ROLE` | 0x06 |
| `APP_CAPS` | 0x07 |
| `ROLE_HEARS_REPEATER` | 0x01 (role flags bit 0: "I hear a repeater or anchor directly") |

Role flags: bit 0 `ROLE_HEARS_REPEATER` (the sender hears a repeater or
anchor directly), bits 1-2 the number of edge bridges heard (0-3; hellos send
0), bit 3 `ROLE_MOBILE` (the sender is moving: routes to it are looked up
afresh after `MOBILE_ROUTE_MS`, 5 minutes, and it never acts as an edge
bridge). The optional bytes 2-3 name the edge bridge the sender has
chosen (a node without coverage relies on the nearest relay that hears a
repeater, see GUIDE "Edge bridges in role auto"); receivers that read only
role and flags are unaffected.

Parsing rules (`appdata::parse`):

* Fields `0x01` and `0x02` have fixed sizes and must come first, in that order
  (when present).
* Every field type `>= 0x03` is TLV: `type | length | data`. A receiver skips
  unknown TLV types by their length, and reads only the bytes it knows inside
  a known field (so fields may grow).
* A type byte below `0x03` that is not `0x01`/`0x02`, or a truncated field,
  stops parsing.

Position precision (`Precision`): 0 off, 1 = 0.1° grid, 2 = 0.01° grid,
3 = 0.00001° (precise). The sender rounds the position to the **centre** of
the grid cell before sending (`Position::quantised`); latitude must be within
±90°, longitude within ±180°. Precision 0 means no position.

Telemetry flags: bit 0 external power/USB, bit 1 temperature valid. Energy is
0 critical, 1 low, 2 ok, 3 full (`Energy` in `router.rs`). Battery 0 = unknown.

Display names are not authenticated beyond the announce signature; they are
for display only. `clean_name` removes control characters, trims whitespace
and truncates at a character boundary.

### 4.3 Proof of work (stamps)

Source: `maarse-core/src/stamp.rs`.

Announces, path requests, key requests, mail syncs and deposits must end
with a stamp:

```text
... payload ... | stamp version (1) | nonce (u32 BE)
```

The stamp is valid when

```text
SHA-256("maarse/stamp" | version (1) | packet type (1) | destination (16) | binding | nonce (4))
```

has at least `D` leading zero bits. Binding:

* `Announce`: payload bytes 64..68 (the timestamp).
* `PathRequest`: payload bytes 0..16 (the reply token).
* `KeyRequest`, `MailSync`, `Deposit`: the whole payload before the stamp
  (all small), so a stamp cannot be moved to another request. A deposit is
  stamped before its message ID is computed, so the end-to-end ack covers
  the stamp. Difficulty 8 bits, as a path request: nothing for one message,
  costly for someone flooding requests or filling post offices.

| Constant | Value |
| --- | --- |
| `STAMP_VERSION` | 1 |
| `STAMP_LEN` | 5 |
| `ANNOUNCE_BITS` | 12 (version 1) |
| `PATH_REQUEST_BITS` | 8 (version 1; also key requests, mail syncs, deposits) |

The difficulty is looked up by `(version, type)` in `difficulty()`; an
unknown version is rejected. Nodes drop announces and path requests with a
missing or weak stamp before any other processing, and do not forward them
(`RouterStats::bad_stamps`, companion stat key 77). Applications receive the
payload with the stamp removed.

---

## 5. Routing

Source: `maarse-core/src/router.rs` unless stated otherwise. The `Router` has
no I/O and no clock; it turns received bytes and timestamps into actions.

### 5.1 Roles and forwarding permission

| Role | Forwards? |
| --- | --- |
| Client (0) | never |
| Repeater (1), Anchor (3) | yes, unless energy is Critical |
| Relay (2) | yes, unless energy is Critical, and only while `hops < min(relay_hops, 7)`; `DEFAULT_RELAY_HOPS = 4` |

Energy also limits floods (`on_flood`): at `Low`, local floods (channel,
announce, probe) are not forwarded; at `Critical` nothing is forwarded.

### 5.2 Path discovery

1. The sender floods a `PathRequest`:
   destination = target, path present with count 0, payload
   `reply token (16, random) | [first queued Data payload] | stamp (5)`.
   The first queued `Data` message for the target is included when
   `16 + len + 5 <= MAX_DIRECT_PAYLOAD`; the reply then also acknowledges it.
2. Each forwarder that may forward appends its own path ID to the path
   (record route), increments `hops`, and re-floods after the flood delay
   (5.3). A non-anchor whose path is already 12 long drops the request.
3. The target delivers any included message at once, then waits
   `RESPONSE_WAIT_SLOTS × slot_ms` (10 slots) to collect more copies and picks
   the best trail (5.3.3).
4. The target sends a `PathResponse` to the reply token, header path =
   reverse of the chosen trail, payload `count (1) | trail (count × u16 BE)`
   (the forward path).
5. The sender recognises the token (or one of its last two older tokens),
   stores the forward path as the route, acknowledges any included message,
   and sends queued messages along the route.

Retries: an unanswered request is repeated after `PATH_RETRY_MS` = 20 000 ms
with a new token (last token byte + 1), up to `PATH_RETRIES` = 2 times. A late
reply to an older token is still accepted. After the last retry, queued
messages are dropped (or carried, 5.10).

### 5.3 Floods

A forwarder with at least `REQUEST_FWD_MIN_NEIGHBORS` = 3 neighbours passes on
only `REQUEST_FWD_PERMILLE` = 800 of 1000 path requests (chosen from the packet
ID and its own ID, so the same node always makes the same choice for the same
request; `Router::request_fwd_permille`, 0 or 1000 = all). A node with fewer
neighbours may be the one way through (a chain, a host between two networks)
and forwards every request. Other floods are always forwarded. In maarse-sim (8 seeds, with the
compact frame) this and the 20 s retry together changed the airtime by -9 %
(forest), -24 % (car), -4 % (town), -13 % (national) and -16 % (outage) with the
share delivered +0.2, +7.0, +5.2, +0.3 and -1.3 points
(`docs/maybe/airtime-optimisation.md`, section 9). Not yet tested on boards.

#### 5.3.1 Flood delay

```text
bucket = (clamp(snr_db, -20, 19) + 20) / 5            -- 0..7
delay  = bucket × slot_ms + energy_penalty + (Relay ? 2 × slot_ms : 0) + extra_flood_delay_ms
```

High SNR (a near neighbour) gives a long wait, so distant forwarders send
first and routes get fewer hops. The radio layer should add a random
0..`slot_ms` on top. `DEFAULT_SLOT_MS = 200`.

Energy penalty (`Energy::penalty_ms`): Full 0, Ok 150, Low 800, Critical 3000 ms.

#### 5.3.2 Suppression

When a node hears another copy of a flood it has scheduled, it cancels its own
transmission if the copy was heard with `snr_db >= weak_snr_db` and the number
of such copies reaches `suppress_after` (default 1). Weak copies do not count:
they come from far away and cover a different area.

`weak_snr_db` defaults to `weak_snr_for_sf(9)`. `weak_snr_for_sf(sf)` is the
sensitivity floor plus 5 dB:

| SF | 5–6 | 7 | 8 | 9 | 10 | 11 | 12 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| floor (dB) | −5 | −8 | −10 | −13 | −15 | −18 | −20 |
| weak limit (dB) | 0 | −3 | −5 | −8 | −10 | −13 | −15 |

Scope: channel, announce and probe floods stop at `LOCAL_SCOPE` (8) hops;
other floods at `MAX_HOPS` (64).

#### 5.3.3 Choosing the reply trail

Among the copies of a path request heard during the wait, the target picks the
trail with the smallest score `(last hop weak?, hop count, −SNR)`, compared
lexicographically: avoid a weak last hop first, then fewest hops, then best SNR.

### 5.4 Source routing and per-hop acknowledgement

For packets with a path (all types except `PathRequest`), `hops` is the index
of the next forwarder:

* If `hops < path.len()` and `path[hops]` is this node's path ID (and it may
  forward), it increments `hops` and retransmits after
  `SOURCE_ROUTE_DELAY_MS` = 50 ms. A copy already seen is forwarded again
  (the previous hop only retransmits when it did not hear us).
* If `hops == path.len()`, the packet has reached the end of its path and is
  handled by the destination (or an anchor, 5.8).

Implicit per-hop acknowledgement: after sending a source-routed packet that
has a next hop, the node listens for the same packet ID with a higher `hops`.
If it is not heard within `hop_timeout_ms` (`DEFAULT_HOP_TIMEOUT_MS` = 3000 ms,
plus jitter derived from the packet ID on retries), it retransmits, up to
`HOP_RETRIES` = 4 times, then tries local repair (5.7). The last hop is
covered by the end-to-end acknowledgement.

### 5.5 End-to-end acknowledgement

```text
ack_token(msg_id) = msg_id (u64 LE) | !msg_id (u64 LE)          -- 16 bytes
```

The destination answers every `Data`, `Fragment` and `Deposit` packet that
reaches it (also repeats, because the first ack may have been lost) with an
`Ack`: destination = `ack_token(packet_id)`, path = reverse of the received
path, empty payload. `Datagram` is never acknowledged end to end.

### 5.6 Route repair

Without an acknowledgement within `ack_timeout_ms`
(`DEFAULT_ACK_TIMEOUT_MS` = 20 000 ms) the sender forgets the route and sends
the message again with a new path request, up to `E2E_RETRIES` = 2 times.
A node marked `mobile` ignores routes older than `MOBILE_ROUTE_MS` = 30 000 ms.

### 5.7 Local repair

When a forwarder gives up on its next hop (no forward heard after
`HOP_RETRIES`), it:

1. Records the next hop as dead for `DEAD_HOP_MS` = 300 000 ms (or until it is
   heard again); at most `MAX_DEAD_HOPS` = 8 entries.
2. Rewrites the path to skip ahead: if the destination itself was heard within
   `BYPASS_FRESH_MS` = 120 000 ms, the remaining path is cut off; otherwise the
   path continues at the farthest node further along it that was heard within
   that time. If no such node exists, the packet is dropped.
3. Sends the rewritten packet at once.

Later packets whose next hop is a known dead hop are rewritten the same way
before they are sent. The packet format is unchanged; nodes that do not do
local repair interoperate.

### 5.8 Anchors: paths beyond 12 hops

A forwarder with role `Anchor` that receives a path request with a non-empty
trail:

1. Remembers `(reply token, target, reverse trail)` for `ANCHOR_TTL_MS` =
   120 000 ms (at most `MAX_ANCHORED` = 16 legs).
2. Forwards the request with an **empty** trail (and `hops + 1`).

When the `PathResponse` for that token reaches the anchor (its path ends
there), the anchor stores the response's forward path as its own route to the
target and sends a new `PathResponse` back along the first leg with the first
leg as payload. The sender's route therefore ends at the anchor.

A `Data`, `Fragment`, `Deposit` or `Datagram` packet whose path ends at an
anchor but whose destination is another node is re-sent by the anchor along
its own route to the destination; the anchor remembers
`(ack_token(id), reverse of the received path)` and relays the `Ack` back.
Several anchors in a row work the same way, up to `MAX_HOPS` in total.

### 5.9 Edge bridge

Purpose: a relay that hears a repeater is often its group's only way into the
repeater network and must not be silenced by the group's own copies.

* Neighbours learn each other's role from the role app-data field
  (`0x06`, section 4.2), from 0-hop announces.
* `is_edge_bridge`: the node is a `Relay`, it hears a repeater or anchor
  directly (within `EDGE_WINDOW_MS` = 300 000 ms), and at least one recent
  non-repeater neighbour does **not** (by its own `ROLE_HEARS_REPEATER` flag;
  for neighbours without the field, a guess: it forwards floods and no
  repeater has been heard echoing it).
* While an edge bridge, a duplicate flood copy only counts towards
  suppression (5.3.2) if its previous hop is a repeater or anchor.

### 5.10 Store-carry-forward and the delivered notice

Enabled when `carry_copies > 0` (firmware: relays with 4 copies; off
elsewhere). A `Data` message the router gives up on (path request failed, or
no ack after all retries) is carried with `carry_copies` tokens
("binary spray and wait"), for `CARRY_TTL_MS` = 30 min, at most `CARRY_MAX` = 8
messages.

Every `CARRY_EVERY_MS` = 5 s the carrier looks at neighbours heard within
`CARRY_FRESH_MS` = 30 s (at most two handoffs/notices per step):

* if the destination's path ID is among them, the message is handed to it
  (repeated at most every `CARRY_RETRY_MS` = 60 s);
* otherwise, if the carrier has more than one token, it gives half of them to
  a newly met non-repeater neighbour that has not had a copy;
* a sender that comes near a repeater retries ordinary routing (on regaining
  repeater contact, and every `CARRY_REROUTE_MS` = 300 s).

**Carry handoff** (type 11, path = one element naming the receiving neighbour,
destination = final destination):

```text
tokens (1) | ttl left in s (u16 BE) | from (PathId, u16 BE) | message (the original Data payload)
```

The destination delivers the message as an ordinary `Data` message (it is
still end-to-end encrypted).

**Delivered notice** (type 11, **no path**, destination =
`ack_token(packet_id(Data, dest, message))`, empty payload; 18 bytes on air).
The destination sends it for every carried copy it receives. A carrier that
hears it deletes its copy, remembers the notice for the rest of the copy's
lifetime (at most 16 notices), passes it on once if it gave copies away, and
repeats it when it meets a neighbour it exchanged a copy with. It also
answers a handoff of an already delivered message with the notice. When it
reaches the original sender, the router reports `CarryAcked` (companion event
`0xa7`). A sender that gets an ordinary `Ack` for a message it still carries
stops its carriers the same way. A receiver checks that the destination's
second half equals the bitwise NOT of the first, and that the payload is
empty, before treating it as a notice.

Notices are not entered in the duplicate filter, so they can be repeated.

### 5.11 Send priorities

Source: `maarse-core/src/qos.rs`. The radio queue reads the priority from the
first byte of an encoded packet (lower goes first):

| Priority | Constant | Packets |
| --- | --- | --- |
| 0 | `PRIO_CONTROL` | `Ack`, `PathResponse`, `Carry` without path (delivered notice) |
| 1 | `PRIO_DIRECT` | `Data`, `Datagram`, `Carry` handoff, `Deposit`, `Control`; channel messages to the bulletin channel (`priority_with`); anything that is not a version-0 packet |
| 2 | `PRIO_CHANNEL` | `Channel` |
| 3 | `PRIO_BULK` | `Fragment` |
| 4 | `PRIO_FLOOD` | `PathRequest`, `Announce`, `Probe` |

`qos::slot_ms` (flood slot sized to the airtime of a 24-byte packet, clamped
to 20–2000 ms) exists but is not used by the firmware.

Airtime budget: `maarse-core/src/duty.rs` implements a token bucket over
`WINDOW_MS` = 3 600 000 ms with the limit in permille (EU default 100 ‰ = 10 %,
`RadioConfig::EU_DEFAULT`).

### 5.12 Regions

Source: `maarse-core/src/region.rs`.

```text
region_id(name) = fold16(FNV-1a-32("maarse-region:" | lowercase(trim(name))))
fold16(h)      = (h >> 16) XOR (h & 0xffff);  0 becomes 1;  empty name gives 0
FNV-1a-32: offset 0x811c9dc5, prime 0x01000193
```

* Each node serves up to `MAX_REGIONS` = 8 region IDs.
* A flood carrying region `r != 0` is forwarded only if the node's list
  contains `r`, or the list is empty and the node is not `strict`. Floods
  without a region are always forwarded. Reception for the node's own user is
  never filtered.
* Channel names may carry a region suffix: `name@region`. The channel key
  depends only on `name`; the region only tags the packets.

### 5.13 Trace route (`Probe`)

Source: `maarse-core/src/diag.rs`.

```text
payload: 0x7E | flags | fixed length n | fixed part (n) | SNR (i8) ...
request (flags 0): fixed part = token (16)
reply   (flags 1): fixed part = k | k × PathId (BE) | m | m × outbound SNR
```

Sent along a known route; every forwarder appends the SNR it heard the packet
with. The target answers to the token along the reverse path, and forwarders
append their SNR again. The packet ID covers only the fixed part (the first
`3 + n` bytes), so per-hop acks still match.

---

## 6. Cryptography

Source: `maarse-crypto/src/*`. **Not externally reviewed.**

Primitives: Ed25519 (signatures), X25519 (key agreement), SHA-256,
HKDF-SHA256, ChaCha20-Poly1305 (96-bit nonce, 128-bit tag), PBKDF2-HMAC-SHA512
(recovery phrase only).

### 6.1 Identities

An identity is an Ed25519 key pair and an independent X25519 key pair. The
secret form is 64 bytes: `Ed25519 seed (32) | X25519 secret (32)`. The public
form is `Ed25519 public (32) | X25519 public (32)`. The address is defined in
section 2.

**Safety number** (`safety_fingerprint`, `safety_number`):

```text
h0     = SHA-256("maarse/safety-number/v0" | sign | dh)
h(i+1) = SHA-256(h(i) | sign | dh)                 -- SAFETY_ITERATIONS = 512 rounds in total
fingerprint = 6 groups: bytes [5i .. 5i+5) as u40 BE, mod 100000, 5 decimal digits
safety number = sort(fingerprint(A), fingerprint(B)) concatenated   -- 60 digits
```

### 6.2 Static seal (direct messages without a session)

Source: `maarse-crypto/src/seal.rs`. One-way, in the style of Noise `X`.

```text
e_pub (32) | sender address XOR k1 (16) | ChaCha20-Poly1305(k2, message) | tag (16)

es = X25519(e, R_dh)                 ss = X25519(S_dh, R_dh)
k1 = HKDF(salt "maarse/dm/v0/sender", ikm es,      info e_pub | R_addr)          -- 16 bytes
k2 = HKDF(salt "maarse/dm/v0",        ikm es | ss, info e_pub | R_addr | S_addr) -- 32 bytes
nonce = 12 zero bytes;  AAD = the first 48 bytes
```

`DM_OVERHEAD` = 64. Largest message: `MAX_DM_TEXT` = 212 − 64 = 148 bytes.
Non-contributory DH results are rejected. Only the recipient learns the
sender; the recipient must know the sender's public keys (from an announce)
to verify `ss`.

### 6.3 Forward-secrecy ratchet (draft)

Source: `maarse-crypto/src/ratchet.rs`.

```text
type (1) | ratchet key (32) | sender XOR mask (16) | n (u16 LE) | pn (u16 LE)
  | ChaCha20-Poly1305(mk, message, AAD = these 53 header bytes) | tag (16)

type  INIT    = 0xF0   (the other party has not answered in this session yet)
      RATCHET = 0xF1
      COMPACT = 0xF2   (short form, see below)
```

`HEADER_LEN` = 53, `FS_OVERHEAD` = 69, `MAX_FS_TEXT` = 212 − 69 = 143.

**Compact form** (`CAP_COMPACT`, `maarse-crypto/src/ratchet.rs`). Once a
session has taken its first ratchet step, a sender whose peer announces
`CAP_COMPACT` sends most messages as:

```text
type 0xF2 (1) | session id (3) | n (u16 LE)
  | ChaCha20-Poly1305(message key, message, AAD = these 6 bytes) | tag (16)

session id = HKDF-Expand(prk = chain key at the start of the chain, "maarse/fs/v2/sid")[..3]
```

`COMPACT_OVERHEAD` = 22 (instead of 69). The tag is the full 16 bytes and the
message key is new for every message, as in the long form. The recipient
finds the session by session id (trying every session with that id) and the
sender from the session. A compact message that opens under no session is
rejected.

Rules:

* The first `FULL_FIRST` = 2 messages after a ratchet step carry the full
  ratchet key (`0xF1`), so one lost message does not break the chain.
* A sender that has heard a new ratchet key takes its next step after at most
  `STEP_EVERY` = 8 messages on the current key, not at once. A step is never
  delayed before the first one (the init chain is only as strong as the static
  format). Without `CAP_COMPACT` the behaviour is exactly the long form.
* A session whose peer may have lost its state sends the next message in the
  long form (`force_full`): when an announce from the peer arrives, and when a
  static message arrives from a peer we have a session with. A node that
  cannot read a compact message announces within a second (at most every
  30 s), so the unknown sender switches to the long form, and the receiver of
  the long form starts a new session as before (`NoSession` and resync).
* Call audio datagrams and mail copies never use the compact form.
* Measured average overhead (maarse-crypto tests): 29.7 bytes in mixed chat,
  33.6 bytes in strictly alternating chat, against 69. With 20 % loss and three
  tries per message and up to 20 % of messages swapped with the next one,
  0.1 % of the messages did not open (long form: 0); they were all behind a
  lost step message.
* The session id and `n` are visible, so a listener can tell that messages
  belong together (the ratchet key does the same in the long form); the
  sender stays hidden. The post-compromise healing delay grows from one turn
  to at most 8 messages in each direction.
* Sessions are stored in a version 2 form (nine more bytes: both session ids,
  the count on the current key and a flag byte); version 1 files load and
  send the long form until their next step.

Derivation:

```text
init:  rk0 | ck0 = HKDF(salt "maarse/fs/v1/init", ikm es | ss, info A1_pub | R_addr | S_addr)   (64 bytes)
       es = X25519(A1, R_dh), ss = X25519(S_dh, R_dh)
root:  rk' | ck  = HKDF(salt rk, ikm X25519(own ratchet, their ratchet), info "maarse/fs/v1/root")
chain: ck' = HKDF-Expand(prk ck, "maarse/fs/v1/chain"), mk = HKDF-Expand(prk ck, "maarse/fs/v1/message")
mask:  HKDF(salt "maarse/fs/v1/sender", ikm X25519(ratchet secret, R_dh), info ratchet_pub | R_addr)  (16 bytes)
nonce: 12 zero bytes (every key is used once)
```

Rules:

* A node only uses this format towards a contact that announces `APP_FS`
  (0x02) or has already sent it an FS message; otherwise the static seal.
* A received message starting with `0xF0`/`0xF1`/`0xF2` is tried as FS first and, on
  failure, as static (a static message starts with a random key byte).
* Skipped message keys: at most `MAX_SKIP` = 8 per session; a chain may jump
  at most `MAX_JUMP` = 512 messages.
* Simultaneous start: the init from the numerically lower address wins.
* A recipient that cannot open a ratchet message (no or broken session)
  starts a new session and sends content type `RESYNC` (0x04, section 7).
* Known gaps (from the source): init messages have no forward secrecy against
  the recipient's static key; a replayed init can reset a session (denial of
  service); headers (`n`, `pn`, ratchet key) are not encrypted.

### 6.4 Channels

```text
nonce (8, random) | ChaCha20-Poly1305(k, sender address (16) | message) | tag (16)

k     = HKDF(salt "maarse/channel/v0", ikm channel key, info "message")
nonce = the 8 bytes followed by 4 zero bytes;  AAD = the 8 nonce bytes
```

`CHANNEL_OVERHEAD` = 40. The claimed sender is **not** authenticated: anyone
with the key can write as anyone.

Linked devices: maarse-host maps the claimed sender to a person before showing the
message. A device that is on the host's own signed device list and not revoked
counts as the person itself (no certificate needed), and its channel messages are
shown on the person's other devices as sent by the person. A device of another
person is shown as that person once its certificate has been checked. Because the
sender is only claimed, someone who holds the channel key can also make a message
appear as the person's own on that person's devices; the same trust boundary as
writing as anyone. Direct messages are authenticated and have no such problem.

Channel keys (`maarse-crypto/src/lib.rs`):

| Channel | Key |
| --- | --- |
| public | `SHA-256("maarse-public")` |
| hashtag `#name` | `SHA-256("maarse/hashtag/v0" \| name)`, where `name` includes the leading `#` (the host adds it if missing; `public` and `#public` mean the public channel) |
| private | 32 random bytes, shared by invitation (companion command `0x13`) |

### 6.5 File chunks

Source: `maarse-crypto/src/seal.rs` (`seal_chunk`).

```text
file ID (u64 LE) | chunk index (u16 LE) | ChaCha20-Poly1305(file key, data) | tag (16)
nonce = file ID (8, LE) | index (2, LE) | 0x0000;  AAD = the first 10 bytes
```

`CHUNK_OVERHEAD` = 26, `CHUNK_DATA` = 212 − 26 = 186 bytes per chunk. The file
key is random per file and travels in the encrypted file offer (section 7.2).

### 6.6 Signed bulletins

Source: `maarse-crypto/src/bulletin.rs`. A bulletin is a channel message on
the hashtag channel `#bulletin` whose plaintext is:

```text
0xB1 | timestamp (u32 BE, Unix seconds) | Ed25519 signature (64) | text
signature over: "maarse/bulletin/v1" | channel address (16) | timestamp (BE) | text
```

`TAG` = 0xB1 (never valid as the first byte of UTF-8), `OVERHEAD` = 69. The
receiver accepts it only if the channel sender address is one of its
configured trusted identities and the signature is that identity's. Everything
else on the channel must not be shown as a bulletin. Room for text: 128 bytes
(126 with a region).

v1 bulletins have no serial and no expiry, so an old one can be sent again;
hosts now treat a v1 bulletin as an alert valid for 24 hours from its
timestamp, shown once (section 6.6.1). New senders use alerts v2.

#### 6.6.1 Alerts v2

Source: `maarse-crypto/src/alert.rs`, host side `maarse-host/src/node/alerts.rs`.
Design and threat model: `docs/maybe/alerting-review.html`. Still a channel
message on `#bulletin` (old repeaters forward it unchanged). The seal's
sender field is the **issuer key**'s address: a separate Ed25519 key used
only for alerts, never the node identity.

```text
alert:  0xB2 | flags | serial (u32 BE) | issued (u32 BE, Unix s) | expires (u16 BE, minutes)
        | event (1) | severity (1) | [reference serial (u32 BE), update and cancel only]
        | [area: lat (i32 BE) | lon (i32 BE), 1e-5 degrees | radius (u16 BE, 100 m)]
        | text (UTF-8) | Ed25519 signature (64)
flags:  bits 0-1 status (0 actual, 1 exercise, 2 test, 3 system)
        bits 2-3 kind (0 alert, 1 update, 2 cancel); bit 4 area present
signed: "maarse/alert/v2" | bulletin channel address (16) | issuer address (16) | all bytes before the signature

cert:   0xC1 | issuer public identity (64) | not_after (u32 BE) | events (u16 BE, bit per event)
        | max severity (1) | flags (1, 0) | name length (1) | name (UTF-8, <= 24) | Ed25519 signature (64)
signed: "maarse/alert-cert/v1" | all bytes before the signature, by a root key
```

Events: 0 info, 1 evacuate, 2 shelter, 3 fire, 4 flood, 5 drinking water,
6 chemical, 7 power, 8 weather, 9 missing person. Severity: 0 minor,
1 moderate, 2 severe, 3 extreme.

Trust: an issuer key configured on the receiver (`--bulletin-key`), or an
issuer whose certificate (sent on the same channel and cached) is signed by a
configured root key (`--alert-root`). A certificate limits the issuer to its
events, a highest severity and a last valid moment (checked against the
alert's issue time). Update and cancel refer to a serial of the same issuer,
so an issuer can only cancel its own alerts.

Replay (`ReplayGuard`): every alert expires (`issued + expires`); each
(issuer, serial) is shown once (256 kept, stored on disk); an issue time more
than 10 minutes ahead of a known clock is refused; a node without a clock
uses the newest verified issue time as a lower bound on the time. Serials
are `max(last + 1, now)`, so they grow even after the state file is lost.

Showing: the host sends `Event::Alert` (companion 0xac), never a channel
line, and the client draws the alert apart from all chat text, so no message
can look like a verified alert. Typing on `#bulletin` is refused; alerts are
issued with `Command::SendAlert` (0x2c) on a host with `--issuer-key`.

Radio priority: packets on `#bulletin` go before other channel traffic, but
the firmware boosts at most 8 per minute (`BOOSTS_PER_MIN`); the channel key
is public, so junk there must not push other traffic aside. Repeaters do not
yet verify alerts before boosting them (they have no trust anchors).

### 6.7 Firmware update signing

Source: `maarse-crypto/src/ota.rs`. Manifest (`MANIFEST_LEN` = 112):

```text
0   "MOTA" (4)
4   format (1) = 1
5   flags (1) = 0 (reserved)
6   board ID (u16 LE)   1 XIAO nRF52840, 2 WisMesh Tag, 3 Heltec V4, 4 XIAO ESP32-S3
8   version (u32 LE) = major << 16 | minor << 8 | patch
12  image size (u32 LE)
16  SHA-256 of the image (32)
48  Ed25519 signature (64) over "maarse/firmware/v1" | bytes 0..48
```

A node installs only if the signature matches its built-in release key, the
board ID matches and the version is newer (an older version may be allowed
only locally, with flag `OTA_ALLOW_DOWNGRADE`). OTA is disabled when the configured
public key is missing, invalid, weak or the public development key. Release archives are signed separately:
Ed25519 over `"maarse-release-v1\n" | SHA-256(SHA256SUMS file)`.

Staging: blocks of `BLOCK` = 4096 bytes, progress on a meta page `"OTM1"`,
so a transfer can be resumed.

### 6.8 Recovery phrase

Source: `maarse-crypto/src/recovery.rs`.

```text
entropy (32 random bytes)
  -> 24 words, BIP39 English word list (256 bits + 8-bit checksum)
  -> seed   = PBKDF2-HMAC-SHA512(phrase, "mnemonic" | passphrase, 2048 iterations)   (64 bytes, as BIP39)
  -> master = HKDF-SHA256(salt "maarse/master/v1", ikm seed, info "")                 (64 bytes)
  -> identity secret = master  (Ed25519 seed | X25519 secret)
```

The phrase must be canonical (lowercase, single spaces); the passphrase must be
NFKD-normalised. Today the node identity **is** the master identity.

---

## 7. Content of direct messages, files and fragments

Source: `maarse-core/src/content.rs`.

### 7.1 Content types

The decrypted plaintext of a `Data` message starts with a content type byte:

| Byte | Constant | Body |
| --- | --- | --- |
| 0x00 | `TEXT` | UTF-8 text |
| 0x01 | `FILE_OFFER` | file offer (7.2) |
| 0x02 | `CONTROL` | counter (u32 LE) \| companion command without length field |
| 0x03 | `CONTROL_REPLY` | counter (u32 LE) \| companion event without length field |
| 0x04 | `RESYNC` | no data; sent as the first message of a new FS session |
| 0x05 | `PAGE_REQUEST` | page path (UTF-8, at most `MAX_PAGE_PATH` = 60 bytes) |
| 0x06 | `CALL` | call signalling or Opus audio; format belongs to the clients; TCP only |
| 0x07 | `IP` | IP tunnel session setup (see `maarse-host/src/tunnel.rs`); TCP, or LoRa with `--tun-lora` |
| 0x08 | `TEXT_Z` | text compressed with `maarse-core/src/textz.rs`; only to a recipient announcing `CAP_TEXTZ` |
| 0x0E | `SENSOR` | readings or an alarm from a sensor node to an owner: `op (1)` then the body; see `docs/sensors.md` |

`TEXT_Z` is a static codebook of about 230 common Danish and English
fragments (after SMAZ): each byte is a codebook index, `ESC` (254) + one
literal byte, or `RUN` (255) + length + literal bytes. Text is compressed
only when that makes it shorter; the receiver expands it to `TEXT` before
anything else looks at it. Short chat messages shrink to about 56 % of their
size (`textz` test).

Unknown types are reported as `Content::Unknown` and ignored.

Remote control (`CONTROL`): the node executes the command only if the sender
(proven by the encryption) is on its administrator list and the counter is
higher than the last one accepted. Commands that are local-only (section 9)
are refused.

Page requests: the path is limited to `a-z 0-9 - _ . /`, no empty segments,
no segment starting with `.`; empty becomes `index`. The answer is an ordinary
file with MIME `PAGE_MIME` = `text/x-maarse-page;deflate` (raw deflate,
RFC 1951) and the path as its name.

### 7.2 File offer

```text
0x01 | file ID (u64 LE) | size (u32 LE) | chunk count (u16 LE) | chunk size (u8)
     | file key (32) | SHA-256(file)[0..16] (16) | MIME length (u8) | MIME | name (rest, at most 64)
```

`OFFER_FIXED` = 65 bytes.

### 7.3 Chunks as fragments

After the offer is acknowledged, the sender sends each chunk (section 6.5) as
a `Fragment` packet along the route; every chunk is acknowledged end to end.
The receiver checks the 16-byte hash when all chunks are in. Implementation
parameters (`maarse-crypto/src/files.rs`, used by firmware): `MAX_FILE` =
8192 bytes, `MAX_CHUNKS` = 64, `WINDOW` = 4 chunks in flight,
`CHUNK_TIMEOUT_MS` = 30 000, `STALL_TIMEOUT_MS` = 300 000. `maarse-host` uses
its own values (`FILE_WINDOW` = 8, `CHUNK_TIMEOUT` = 20 s, `FILE_TIMEOUT` =
600 s) and allows larger files over TCP.

---

## 8. Mailboxes and the backbone

### 8.1 Post offices (Deposit, MailSync)

The managed replica policy filters the placement pool by operator-configured
host identities and requires the mutual DHT exchange for replica RPCs. It
does not change the local mailbox or the LoRa packet formats. Hosts that
coordinate placement and retrieval should use the same policy.

Source: `maarse-core/src/mail.rs`, `maarse-host/src/node.rs` (post office),
`firmware/common/src/node.rs` (`mail_step`).

**Who keeps mail.** Post offices say so in their hellos and announces: role
flags bits 4-5 = class (0 none, 1 small flash, 2 megabytes, 3 a host with a
disk). Hosts are class 3. Radio nodes are class 0 today: repeaters carry mail
but do not keep it (8 KB of flash holds 30-40 messages for everyone in range).

**Home post office.** Every node picks the best post office among its
neighbors (`Router::best_post_office`: highest class, then best SNR) and names
it in its announce (app-data field `0x01`).

**Dropping mail.** When a direct message has no end-to-end ack within
`DEPOSIT_AFTER_MS` = 45 s, the sender hands it to **its own** home post office
(a host keeps it itself), sealed without forward secrecy (static format: a
ratchet message delivered days later may fall outside the skipped-key window).

```text
Deposit payload = recipient (16) | op (1) | body
```

| op | body | meaning |
| --- | --- | --- |
| 0 `STORE` | sealed message | keep it for the recipient |
| 1 `SYNC` | have IDs (u32 BE each) | deliver the recipient's mail to it, wherever it is |
| 2 `FETCH` | asker (16) \| have IDs | send the rest to the asker as `STORE` |
| 3 `DELIVERED` | IDs | delete them |
| 4 `BUNDLE` | n × (length (1) \| sealed message) | one or more stored messages, sent to the recipient itself |
| 5 `RECEIPT` | n × receipt ID (u64 LE) | recipient to the original sender: the mail arrived |

`BUNDLE` goes only to a recipient announcing `CAP_BUNDLE`: a host packs as
many waiting messages as fit in one packet (a single message of up to 255
bytes too), and one end-to-end ack covers them all (then `DELIVERED` goes to
the replicas). The recipient opens each sealed message as if it had arrived
as `Data`.

**Delivery receipt.** The post office does not know who sent a message, and
the recipient's ack goes to the post office. So a recipient that opens a
message from a `BUNDLE` sends the sender (now known from the decrypted
message) a `Deposit` with op `RECEIPT` addressed to the sender. The receipt ID
is `packet_id(Data, recipient, sealed)` of the copy the sender deposited; the
sender recomputes it for the copies in its outbox and reports the matching
message as acknowledged ("delivered") to its app. A node that does not know
op 5 ignores it (hosts) or treats it as the old format (older firmware keeps
a few useless bytes in its small mailbox until they expire). Messages
delivered straight to the recipient are acked as before and need no receipt.
Deposits carry a stamp (section 4.3).

A mail ID is the top 32 bits of `packet_id(Data, recipient, sealed)`. A
payload without a valid op byte is taken as the old format `recipient |
sealed` and stored.

**Copies.** A host that takes a message keeps it and sends copies to the
recipient's **three replica hosts**: the `REPLICAS` = 3 highest rendezvous
scores `packet_id(Deposit, host, recipient)` over the hosts in its DHT table
(and itself if it is public), over the DHT's TCP requests (section 8.4), never
over radio. Every host computes the same three hosts for a recipient; when a
host joins or leaves, only the recipients it ranked first move.

**Asking for mail.** A node sends a `MailSync` (packet type 14, scope
`MAIL_SCOPE` = 2 hops; payload `home (PathId u16 BE, 0xFFFF none) | have IDs
(at most 8) | nonce (u16)`) when it starts, when its home post office changes,
when a post office's hello names it, and every 6 hours. A post office that
holds its mail delivers; the one it named as home also fetches the copies from
the replicas first. A node whose home post office changed also sends `SYNC`
to the previous one, which delivers over whatever route reaches the node now
(LoRa trunks included).

**Mail waiting.** A post office's hello carries up to 4 path IDs of recipients
with mail waiting (`role | flags | bridge | ids...`), like the traffic
indication map in a Wi-Fi beacon.

**Presence.** A host that hears a node's announce over its own radio asks the
node's replicas for mail (at most every 10 minutes per node), as a mobile
network delivers SMS when a phone registers.

**Delivery and retries.** A post office delivers stored mail as ordinary
`Data` (`send_direct`); the recipient's duplicate filter drops copies. When
the delivery is acknowledged, the office tells the replicas `DELIVERED`. An
office without the backbone retries delivering what it holds after 5, 15 and
35 minutes, then waits for the recipient to ask; with the backbone, presence
and syncs deliver (in maarse-sim unlimited retries cost 40 % more airtime). An
office delivers a message at most once a minute, and a copy that arrives for a
present recipient is delivered on its own. Limits: 100 messages per
recipient, 7 days.

### 8.2 TCP packet links

Source: `maarse-host/src/links.rs`.

```text
backbone frame: length (u16 LE, 1..255) | Må:rse packet
```

A zero length or one above `MAX_PACKET` closes the link. Packets received over
TCP are treated as heard with SNR 10 (`TCP_SNR`). Links use TCP keepalive
(30 s idle, then every 10 s). Outgoing peers are reconnected every 5 s.

The first two bytes of an incoming connection select the protocol:

| First bytes | Meaning |
| --- | --- |
| `D7 D7` (`dht::MAGIC`) | one DHT request (8.4) |
| `4D 4D` = `"MM"` (`MODEM_HELLO`) | ESP32 LoRa modem over Wi-Fi |
| anything else | the length of the first backbone frame |

Neither magic can be a valid frame length (both exceed 255 as u16 LE).

**Region hello.** Right after connecting, each side may send a frame whose
content is:

```text
0xFF 'R' 'G' | strict (1) | count (1) | count × region ID (u16 BE)
```

`0xFF` has version bits `11`, so it is never a valid Må:rse packet; older hosts
count it as invalid. After a hello, floods with a region are only sent to peers
that serve that region; peers without a hello get everything.

**Packet observations (working copy).** An observation collector sets bit 5
of the capability flags appended to its region hello (section 8.4). An opted-in
host sends reports only on the configured outgoing TCP peer link after it sees
this capability. Each report is a normal length-prefixed TCP frame, with content
`0xFF 'O' 'B' | encrypted report`. TCP consumes it at the link layer. The mesh
router and LoRa transports are outside this exchange. Old peers do not advertise
the capability and are sent ordinary traffic.

The encrypted report uses the existing direct-message sealing envelope addressed
to the configured collector's public identity (section 6). The collector's key
lookup is limited to its configured observer identities. Plaintext is
`"OBS1" | session (u64 LE) | batch sequence (u32 LE) | Unix ms (u64 LE) | rows`.
A row is `packet ID (u64 LE) | age in ms (u16 LE) | packet type (u8) | hops (u8)
| flags (u8) | SNR (i8)`. Flags use bits 0–1 for radio (0), trunk (1) or
backbone (2), bit 2 for a queued send and bit 3 if SNR is present. SNR is absent
for TCP and queued sends. There are one to eleven rows per frame. The receiver
checks the timestamp within 30 seconds of its clock and increasing sequence per
observer/session while the recent replay cache is present. TCP timing and frame
sizes remain observable. Authentication binds each report to an allowed host,
which still controls the accuracy of its telemetry. Relaying reports between
collectors is outside this version.

**Open channel names (working copy).** Capability bit 6 in the TCP region
hello signals support for an optional length-prefixed descriptor
`0xFF 'O' 'C' | name (UTF-8, up to 240 bytes)` immediately before an
ordinary `Channel` flood. Names use `public` or `#name` and may include
`@region`. The receiver checks that the name's deterministic open key
derives the next frame's destination address, that the region ID matches,
and that the packet is a channel flood. Hosts can remember up to 256 such
names and attach them when forwarding matching channel traffic to compatible
TCP peers. The original channel packet format and routing are unchanged,
and old peers and LoRa links receive the ordinary packet alone.
Locally joined channels supply a descriptor only if the configured key is the
deterministic open key. Descriptors disclose open names to the TCP peer.
They are hints and do not authenticate the sender. A public collector
authenticates ciphertext with the derived key before publishing text and
treats the sender field of open-channel text as a claim.

**Modem framing.**

```text
serial modem   host -> node: length | 0x40 | packet
               node -> host: length | 0xC0 | snr (i8) | packet
               (companion framing, section 9.1)
Wi-Fi modem    after "MM": node -> host: length (u16 LE) | snr (i8) | packet
                           host -> node: length (u16 LE) | packet
```

**Firmware updates through a modem link** (`maarse-host/src/fwupdate.rs`). The
host updates the radios attached to it. On a USB or Bluetooth modem it writes
the companion commands `OtaBegin`/`OtaChunk`/`OtaEnd`/`OtaStatus` to the same
stream (the node also answers them in modem mode) and reads `OtaState`
(0x9a) and `OtaInfo` (0x9b) frames back. A Wi-Fi modem carries them on its link: host -> node
`length | 0xFF 'O' 'T' | command frame (no length)`, node -> host
`length | 0x7D | OtaState or OtaInfo frame (no length)`. The node accepts only those four
commands from the link and ignores the "allow an older version" flag there;
it checks signature, board, version and SHA-256 itself, so the link needs no
trust (a host that is not yours can send an update the node refuses, nothing
more). Chunks are 200 bytes (a multiple of 4, and the whole frame stays under
the 255-byte packet limit). Firmware from before this change does not answer
on a Wi-Fi link and must be updated once over USB.

### 8.3 DHT purpose and records

Source: `maarse-host/src/dht.rs`. Kademlia-like, TCP only, never over LoRa.
Node ID = the host's Må:rse address, distance = XOR.

Table limits: at most `MAX_PER_NET` = 4 contacts whose endpoint is in the same
IPv4 /24 or IPv6 /48 (loopback and DNS names are not limited), against one
machine filling the table with made-up IDs. Contacts are saved to
`dht-contacts.txt` (one `address-hex endpoint` per line) and used as seeds at
the next start. A node with `--min-peers N` (maarse-server: 6) opens packet
links to contacts from different buckets until it has N backbone links.
**Wi-Fi modem key** (`maarse_core::modemauth`). After a modem's `MM` a host
with a key sends `length | 0xFF 'M' 'K' | nonce (16)`; the modem answers
`length | 0x7F | 0xFF 'M' 'R' | tag (16)` with tag = the first 16 bytes of
SHA-256("maarse/modem-auth" | key | nonce) and key = SHA-256("maarse/modem-key"
| passphrase). No valid answer within 10 s (16 frames) closes the link.
maarse-server requires a key; maarse-host only when `--modem-key` is set.

A Wi-Fi modem also sends its status to the host every 30 s: `length | 0x7E |
event frame without length` (`Stats` twice, then `Radio`). With its host set to
`listen:<port>` the modem accepts the host's connection instead of connecting
out (the host uses `--modem-ip`); the stream is the same after `MM`.

**Capabilities** (`dht::Caps`): `version (str) | flags (bit 0 server, 1 radio,
2 post office, 3 relay, 4 call test, 5 observation collector, 6 open channel names) |
mail free MB (u32 LE, 0xFFFFFFFF = no limit) | n |
n × region (u16 BE)`. Asked with DHT request type 8 (`Caps`, no body),
answered with response type 0x86. The same bytes follow the regions in the
backbone link's region hello (older peers ignore them). Replica post
offices are chosen only among contacts that offer a post office (unknown =
yes). A host with `--no-relay` publishes records only for addresses heard over
its own radio; with `--no-post-office` it answers `MailStore` with
`Stored(false)` and its hellos say mail class 0.

`maarse-server` is this host without radio and web client; it requires
`--public-addr`, so it publishes records for every address it reaches,
including those behind hosts that connect out to it from behind NAT.

A record says where an address can be reached:

```text
record = dest (16) | gateway ID (16) | endpoint (str) | hops (1) | age s (u32 LE)
       | announce length (u16 LE) | announce (the dest's signed announce payload)
str    = length (1) | UTF-8 (at most 255; endpoints at most MAX_EP = 100)
```

A record is valid if the endpoint is non-empty and at most 100 bytes, the
announce is at most 255 bytes, `age < RECORD_TTL` and the announce verifies
against `dest`. A store keeps the new record if it has fewer hops, the same
hops and is not older, or the old one is older than `2 × REPUBLISH`. The
gateway part is not signed by `dest`; a lying gateway can at most drop
(end-to-end encrypted) traffic.

| Constant | Value |
| --- | --- |
| `K` | 8 (bucket size, replication factor) |
| `ALPHA` | 3 (parallel requests per round) |
| `RECORD_TTL` | 2 h |
| `REPUBLISH` | 30 min |
| `RPC_TIMEOUT` | 3 s |
| `FULL_REPLICA_MAX` | 64 |

Gateway route proofs bind the destination, gateway, endpoint, hop count, issue
time and complete node announce with an Ed25519 signature under the gateway's
identity. The proof authenticates the gateway's claim. The destination's signed
announce identifies the destination. A gateway can still make an incorrect
reachability claim. An unsigned legacy record may supply a node announcement,
but a new outbound gateway connection requires a valid proof or an explicitly
configured endpoint. The proof is an optional trailing 136-byte extension to a
record: 8-byte little-endian Unix issue time, 64-byte public gateway identity,
and 64-byte signature. Legacy relays may drop this extension when forwarding a
record, so upgrade the DHT seed and gateway hosts before relying on automatic
gateway connections across a mixed deployment.

### 8.4 DHT wire protocol

For mutual DHT identity authentication, TCP starts with `D7 D9` and a
random 32-byte client challenge. The server issues its own random 32-byte
challenge. After the length-prefixed request, the client sends its 64-byte
public identity and 64-byte Ed25519 signature over both challenges and the
exact request. The receiving host verifies that this identity derives the
sender address in the request before it handles the operation. The response
uses the signed reply format below. An eligible, explicitly configured
endpoint can try older exchanges if the newer exchange yields no response.
DHT identity authentication does not encrypt traffic or authenticate a
particular recipient's authorization to access their mail.

For authenticated replies, TCP starts with `D7 D8`, a random 32-byte challenge,
then the existing length-prefixed request. The length-prefixed response appends
the 64-byte responder identity and a 64-byte Ed25519 signature. The signed
digest binds the challenge and both request and response bytes. An expected
contact address must match the responder identity. A configured endpoint can
fall back to legacy `D7 D7` if the signed exchange returns no frame. Discovered
endpoints require signed replies. This exchange authenticates the responder
and leaves request authentication and transport encryption to other layers.

One request per TCP connection:

```text
client -> host: D7 D7 | length (u16 LE) | request
host -> client: length (u16 LE) | response        (then close)
messages larger than 4096 bytes are rejected

request  = type (1) | sender ID (16) | sender endpoint (str, "" = none) | body
response = type (1) | responder ID (16) | body
```

| Type | Name | Body |
| --- | --- | --- |
| 0x01 | Ping | – |
| 0x02 | FindNode | target (16) |
| 0x03 | FindValue | target (16) |
| 0x04 | Store | record |
| 0x05 | MailStore | recipient (16) \| length (u16 LE) \| sealed message |
| 0x06 | MailFetch | recipient (16) \| count (1) \| count × have ID (u32 BE) |
| 0x07 | MailDelivered (legacy) | recipient (16) \| count (1) \| count × ID (u32 BE). Updated hosts refuse this unauthenticated deletion request. |
| 0x81 | Pong | – |
| 0x82 | Nodes | count (1, at most 16) \| count × (ID (16) \| endpoint (str)) |
| 0x83 | Value | record |
| 0x84 | Stored | ok (1) |
| 0x85 | Mail | count (1) \| count × (length (u16 LE) \| sealed message) |

The three mail requests carry post office copies between hosts (section 8.1).

`FindValue` returns `Value` if the record is stored, otherwise `Nodes`.
Lookups are iterative (at most 20 rounds, `ALPHA` new contacts per round).

### 8.5 Full replication

When a host knows at most `FULL_REPLICA_MAX` (64) DHT contacts, it stores its
records on all of them instead of the `K` closest, so every host holds the
whole table and each side of a split backbone keeps working. When a backbone
link comes up, the host republishes all its records.

A host that needs an unknown address looks it up, connects to the gateway as
an ordinary TCP peer, and normal routing does the rest.

---

## 9. Companion protocol

Source: `maarse-core/src/companion.rs` (and `maarse-host/src/history.rs` for
0x30–0x33 / 0xB0–0xB3).

### 9.1 Framing

The same frames are used over serial/USB, Bluetooth LE (Nordic UART Service)
and WebSocket:

```text
length (u16 LE) | type (1) | content
```

`length` counts type + content and must be 1..`MAX_FRAME` (300). A stream
decoder that sees an invalid length drops one byte and resynchronises.
Integers are little endian, addresses 16 bytes, "text" is UTF-8 filling the
rest of the frame (at most `MAX_TEXT` = 240 bytes). `str8` below means a length
byte followed by that many bytes. `PROTOCOL_VERSION` = 2 (reported in `Info`).

Commands (client to node) have type < 0x80; replies and events (node to
client) have type >= 0x80. `req` is a client-chosen u16 echoed in replies.

### 9.2 Commands

| Code | Name | Content | Notes |
| --- | --- | --- | --- |
| 0x01 | GetInfo | – | reply `Info` |
| 0x02 | SendDirect | req (2) \| dest (16) \| text | reply `Sent` |
| 0x03 | SendChannel | req (2) \| channel (16) \| text | |
| 0x04 | JoinChannel | name (text) | `#name`, `public`, optional `@region`; reply `ChannelJoined` |
| 0x05 | AnnounceNow | – | |
| 0x06 | FileBegin | req (2) \| dest (16) \| size (u32) \| mime (str8) \| name (text) | |
| 0x07 | FileData | req (2) \| offset (u32) \| data (text) | |
| 0x08 | FileEnd | req (2) | reply `FileStarted` |
| 0x09 | GetStats | optional: up to 24 stat keys (1 byte each) | reply `Stats`. With a key list the reply is one frame with only those keys; an older node ignores the list and answers as without it |
| 0x0a | SetRadio | radio config (16) \| optional trial minutes (1) | see 9.5. Locally it is applied and saved at once. Through `Remote` it is a **trial**: applied but not saved until `ConfirmRadio`; the byte is the trial length (0 or absent = 5, at most 30). An older node saves at once |
| 0x0b | GetRadio | – | reply `Radio` |
| 0x0c | Remote | req (2) \| dest (16) \| frame (type + content, no length) | sent as content `CONTROL`; reply `RemoteReply` |
| 0x0d | SetAdmins | up to 4 × address (16) | local only |
| 0x0e | GetAdmins | – | reply `Admins` |
| 0x0f | SetTime | unix (u32) | |
| 0x10 | SetRole | role (1) | see Notes |
| 0x11 | SetRegions | strict (1) \| names (comma separated text) | reply `Regions` |
| 0x12 | GetRegions | – | |
| 0x13 | JoinPrivateChannel | key (32) \| name (text) | reply `ChannelJoined` |
| 0x14 | GetFileLimit | dest (16) | host only; reply `FileLimit` |
| 0x15–0x17 | – | reserved | |
| 0x18 | OtaBegin | req (2) \| flags (1) \| manifest (112) | flag bit 0 allow downgrade (local only); reply `OtaState` |
| 0x19 | OtaChunk | req (2) \| offset (u32) \| data | offset and length multiples of 4 except the last |
| 0x1a | OtaEnd | req (2) \| apply (1) | |
| 0x1b | OtaStatus | – | |
| 0x20 | TraceRoute | req (2) \| dest (16) \[\| via (1)\] | reply `TraceResult`; `via` 0 (or absent) = the node's choice, 1 = look for the route and send the probe over the radio only, 2 = over the internet links only (a host with both kinds of link; other nodes ignore it; a host without that kind answers `Error`) |
| 0x21 | GetNeighbors | – | reply one or more `Neighbors` |
| 0x22 | SetSharing | sharing (10) | reply `Sharing` |
| 0x23 | GetSharing | – | |
| 0x24 | SetName | name (text, at most 24 bytes used) | reply `Name` |
| 0x25 | GetName | – | |
| 0x26 | RequestPage | req (2) \| dest (16) \| path (text) | reply `Sent` |
| 0x27 | SendCall | dest (16) \| data (text) | TCP only, else `Error` code 9 |
| 0x28 | SetWifi | flags (1: bit 0 enabled, bit 1 modem) \| ssid (str8) \| password (str8) \| peer (text) | local only |
| 0x29 | GetWifi | – | reply `Wifi` |
| 0x2a | SetRangeTest | range test config (15) | local only; reply `RangeState` |
| 0x2b | GetRangeTest | – | reply `RangeState` then `Power` |
| 0x2d | SetModemKey | passphrase (UTF-8, empty = remove) | Wi-Fi modem key, local only; the node stores SHA-256("maarse/modem-key" \| passphrase) |
| 0x2c | SendAlert | req (u16) \| AlertMeta \| text | issue a signed alert (host with `--issuer-key`); reply `Sent` with msg_id = serial, firmware replies an error |
| 0x30 | GetHistory | req (2) \| conv (16) \| before id (u64) \| count (1) | host only |
| 0x31 | MarkRead | conv (16) \| id (u64) | host only |
| 0x32 | GetReadState | – | host only |
| 0x33 | – | reserved for history | |
| 0x34 | SetIdentity | flags (1: bit 0 confirm) \| secret (64) | local only; reply `IdentityChanged` |
| 0x35–0x37 | – | reserved (linked devices) |
| 0x39 | Sensor | req (2) \| dest (16, all zeros = the connected node or the host) \| op (1) \| data | sensor nodes: recipe, owners, status, host operations (history, simulation); a command for another node goes to it as a control command; see `docs/sensors.md` | |
| 0x44 | ConfirmRadio | – | keep the radio settings on trial (allowed through `Remote`); reply `Radio` with the settings in use. Without a trial it only answers that, so a repeated confirmation is harmless |

### 9.3 Events

| Code | Name | Content |
| --- | --- | --- |
| 0x81 | Info | addr (16) \| repeater (1) \| version (1) \| max_text (1) \| max_file (u32) \| [identity (64)] |
| 0x82 | Sent | req (2) \| msg_id (u64) |
| 0x83 | DirectReceived | from (16) \| hops (1) \| text |
| 0x84 | ChannelReceived | channel (16) \| from (16) \| hops (1) \| text |
| 0x85 | Acked | msg_id (u64) |
| 0x86 | RouteLearned | dest (16) \| hops (1) |
| 0x87 | Heard | addr (16) \| hops (1) \| [identity (64)] |
| 0x88 | ChannelJoined | channel (16) \| name (text) |
| 0x89 | FileIncoming | file_id (u64) \| from (16) \| size (u32) \| mime (str8) \| name (text) |
| 0x8a | FileChunk | file_id (u64) \| offset (u32) \| data |
| 0x8b | FileProgress | file_id (u64) \| done (u16) \| total (u16) \| outgoing (1) |
| 0x8c | FileDone | file_id (u64) \| ok (1) \| outgoing (1) \| text |
| 0x8d | FileStarted | req (2) \| file_id (u64) \| chunks (u16) |
| 0x8e | Log | text |
| 0x8f | Error | req (2) \| code (1) \| text |
| 0x90 | Deposited | msg_id (u64) |
| 0x91 | Stats | n × (key (1) \| value (i32)), at most 59 pairs |
| 0x92 | Radio | radio config (16) |
| 0x93 | RemoteReply | from (16) \| event frame (type + content) |
| 0x94 | Admins | up to 4 × address (16) |
| 0x95 | Regions | strict (1) \| names (text) |
| 0x96 | FileLimit | dest (16) \| max_bytes (u32) \| flags (1: bit 0 via LoRa/unknown, bit 1 mailbox) \| est_airtime_s (u32) |
| 0x9a | OtaState | req (2) \| state (1) \| next (u32) \| size (u32) \| version (u32) \| current (u32) \| text |
| 0x9b | OtaInfo | board (u16 LE, `maarse_crypto::ota::board`); sent after `OtaState` as the answer to `OtaStatus` by firmware that can be updated, so a host picks the matching image. Older firmware never sends it (the host then says "update once over USB"). A frame of its own because `OtaState` ends in free text. |
| 0xa0 | TraceResult | req (2) \| dest (16) \| k (1) \| k × PathId (u16 LE) \| m (1) \| m × SNR out (i8) \| SNR back (rest) |
| 0xa1 | Neighbors | part (1) \| last (1) \| n × neighbour (15) |
| 0xa2 | Sharing | sharing (10) |
| 0xa3 | NodeInfo | addr (16) \| hops (1) \| app-data TLVs `0x03`/`0x04`/`0x05` (section 4.2) |
| 0xa4 | Name | name (text) |
| 0xa5 | Call | from (16) \| data |
| 0xa6 | Wifi | flags (1: bit 0 enabled, bit 1 modem) \| state (1) \| ip (4) \| ssid (str8) \| peer (text) |
| 0xa7 | CarryAcked | msg_id (u64) |
| 0xa8 | RangeState | config (15) \| sf (1) \| synced (1) \| count (u32) \| missed (u32) \| seq (u32) \| n × per-SF (12) |
| 0xa9 | RangeRx | see `RangeRx` in `companion.rs` |
| 0xaa | RangeTx | see `RangeTx` in `companion.rs` |
| 0xab | Power | see `PowerReport` in `companion.rs` |
| 0xac | Alert | issuer (16) \| serial (u32) \| issued (u32) \| AlertMeta \| hops (1) \| trust (1) \| name length (1) \| name \| text | a verified alert, shown once (section 6.6.1); trust 0 key, 1 certificate, 2 old v1 bulletin |

`AlertMeta` (little-endian like the rest of the companion protocol):
status (1) | kind (1) | event (1) | severity (1) | expires minutes (u16) |
reference serial (u32) | area present (1) [| lat (i32) | lon (i32) |
radius (u16, 100 m)].
| 0xb0 | HistoryItem | host only, see `history.rs` |
| 0xb1 | HistoryEnd | host only |
| 0xb2 | ReadMark | host only |
| 0xb3 | HistoryInfo | host only |
| 0xb4 | IdentityChanged | addr (16) \| public identity (64) |
| 0xb5–0xb7 | – | reserved (linked devices) |
| 0xbd | Sensor | node (16) \| op (1) \| data: an answer or a push about a sensor node (recipe pieces, status, owners, a reading or alarm message); see `docs/sensors.md` |

Firmware answers commands it does not know (including 0x14 and 0x30–0x33)
with `Error`.

### 9.4 Error codes and state values

Error codes (`companion.rs`, module `error_code`): 1 `UNKNOWN_DEST`, 2 `TOO_LARGE`,
3 `QUEUE_FULL`, 4 `NOT_IN_CHANNEL`, 5 `BAD_FRAME`, 6 `UNKNOWN_SENDER`,
7 `FILE_TOO_LARGE`, 8 `NO_FILE`, 9 `NOT_OVER_TCP`.

OTA states (`ota_state`): 0 idle, 1 receiving, 2 verified, 3 installing,
4 error, 5 unsupported.

Wi-Fi states (`wifi_state`): 0 off, 1 joining, 2 online, 3 linked,
10 skipped after crash, 11 bad settings, 12 driver failed.

### 9.5 Structures

```text
radio config (16): freq Hz (u32) | bandwidth Hz (u32) | sf (1) | cr denominator (1)
                   | tx power dBm (i8) | duty permille (u16) | preamble (u16)
                   | announce interval (1, quarter hours; 0 = default 12 h)
EU default: 869 525 000 Hz, 125 kHz, SF9, 4/5, 22 dBm, 100 ‰, preamble 16, announce 0

sharing (10):      precision (1) | lat (i32, 1e-7°) | lon (i32) | telemetry (1)
neighbour (15):    path ID (u16) | address prefix (4, zeros = unknown) | SNR (i8)
                   | RSSI (i16, 0 = unknown) | seconds since heard (u32) | packet count (u16)
range config (15): mode (1) | flags (1) | interval s (u16) | sf mask (1) | per step (1)
                   | pad (1) | lat (i32) | lon (i32)
```

### 9.6 Stats keys

Keys in `Stats` (module `stat`): 1 uptime s, 2 known nodes, 3 routes,
4 channels, 5 links, 6 clients, 7 role, 10 rx packets, 11 rx invalid,
12 rx duplicates, 13 forwarded, 14 originated, 15 delivered, 16 acked,
17 routes learned, 18 path requests, 19 hop retries, 20 floods cancelled,
30 radio rx ok, 31 radio rx err, 32 radio tx ok, 33 radio tx err,
34 last RSSI, 35 last SNR, 36 avg RSSI, 37 avg SNR, 38 CAD busy,
39 duty deferred, 40 airtime ms, 41 duty used ‰, 42 radio bytes rx,
43 radio bytes tx, 44 tx queue, 45 freq kHz, 46 SF, 47 BW kHz, 48 tx power,
49 battery mV, 50 min RSSI, 51 max RSSI, 52 radio dropped, 53 radio stage,
54 radio loops, 60 mailbox recipients, 61 mailbox messages, 62 files out,
63 files in, 64 outbox waiting, 65–68 link bytes/packets in/out,
69 deposits made, 70 mail delivered, 71 errors, 72 DHT contacts,
73 DHT records, 74 DHT lookups, 75 DHT found, 76 DHT peers, 77 bad stamps,
80 noise floor dBm, 81 energy, 82 external power, 83 neighbours,
84 position precision, 85 battery %, 86 temperature (0.1 °C), 87 stack free,
88 heap free, 89 heap min free, 90 crashes, 91 panic line, 92 panic file,
93 hellos sent, 94 key requests sent, 95 local repairs, 96 messages carried,
97 pinned contacts, 98 mobile mode (0/1), 99 quota dropped, 100 messages
waiting for a key, 101 outbox (messages kept for a post office deposit), 118 seconds left of a remote radio change on trial (0 = none).
Status page keys (2026-11-25, working copy, not released): 103 GPS satellites
in use, 104 seconds since the last GPS fix, 105 GPS state (0 off, 1 on but the
module is silent, 2 searching, 3 has a position), 106 satellites in view, 107
NMEA bytes received, 108 Bluetooth state (0 off, 1 advertising, 2 connected),
109/110 Bluetooth bytes in/out, 111/112 USB bytes in/out, 113 seconds since the
USB host last sent a frame, 114 (host) LoRa radios connected, 115 (host)
seconds since a radio last sent anything, 116/117 own GPS latitude/longitude
x 10^7. Keys 116/117 are sent only to a directly attached companion (a third
frame after `GetStats` over USB or Bluetooth), never in a remote-control reply.
Clients must ignore unknown keys.

A `Stats` frame holds at most 59 values (the frame limit is 300 bytes), so the
firmware answers `GetStats` with **two** frames: the first with keys 1–92,
the second with the post office counters (60, 61, 70) and keys 93–101.
Receivers merge them by key (a frame containing key 1 starts a new set).
Over remote control the values come in chunks of 25 as before. In modem mode
the firmware answers `GetStats` and `GetRadio` without leaving modem mode;
Experimental tools (2026-11-25, working copy, not released): command `0x3c`
`GetGps` (a board with a GPS module answers with event `0xBE` `Gps`, the layout of
`maarse-core/src/gpsinfo.rs`: state, satellites used and in view, HDOP, height, speed,
course, UTC, position and up to 24 satellites with system, number, elevation, azimuth,
signal and used flag) and `0x3d` `Sweep` (start kHz u32, step kHz u16, count u8 (at most
128), sweeps u8: 255 = until stopped or 2 minutes, 0 = stop). The radio measures the
signal strength on each step and answers with one event `0xBF` `Sweep` per sweep: number
u16, start kHz u32, step kHz u16 and one byte per step, `-dBm x 2` (255 = not measured).
Both are answered in modem mode without leaving it; maarse-host passes them to its USB
and Bluetooth radios and the events back to its clients. While it sweeps, the radio does
not receive packets.

Modem mode frame `0x42` (host to radio, one byte, no content): a message arrived at the
host; a board with a buzzer beeps. It does not end modem mode. Boards without a buzzer ignore it.

Recording on the radio itself (2026-11-26, working copy, not released). Command `0x3e` `SweepRecStart` (start kHz
u32, step kHz u16, duration s u16 (1 to 600), interval u8 in tenths of a second with 0 = 1, threshold dB u8 with 0 = 6) makes
the radio record 64 steps from the start frequency, one sweep after the other (10 a second with an interval of 1), and make
the file below. While it records the radio hears nothing and sends nothing (what waits to be sent waits), so a repeater is out
of the network for the duration; a longer interval makes it listen between the sweeps, but then the strongest value of a
bucket is mostly noise and the file is much larger (5 minutes, one sweep every 2 s: 1.9 KB). Command `0x3f` `SweepRecStop` stops it and drops what it has; command `0x43`
`SweepRecStatus` asks how it goes. (`0x40`, `0x41` and `0x42` are modem frames, so a command number must not use them.)
The answer to all three is event `0xB9` `SweepRecState`: state u8 (0 idle, 1 recording, 2 ready, 3 sending, 4 failed),
seconds recorded u16, seconds asked for u16, sweeps u16, file size u16 (0 until it is made). When the file is made the radio
sends it to whoever asked: through `Remote` (the three commands are accepted there, from administrators only) it goes to that
node as an ordinary file over the air (MIME `application/x-maarse-msr`, name `Spectrum.msr`, the file transfer of section 7)
and the requester also gets a `SweepRecState` with state 3 in a control reply; asked over USB or Bluetooth it arrives as an
incoming file from the radio's own address. The radio keeps the cells of a recording in its two 8 KB file buffers, so a
recording costs no RAM of its own (an ESP32's stack is what is left of its RAM and has no room): while it records, files
cannot be sent or received, and a new recording is refused while a file is being transferred. A file that does not fit
(6,400 bytes; a typical one is 0.1-1 KB) makes the recording fail. The file only exists in RAM: it is gone after a
restart. maarse-host passes the three commands to its USB and Bluetooth radios and forwards the state event and a radio's
own file events (a radio in modem mode receives no files from the air) to its clients; clients save a file with this MIME
under Recordings instead of showing it in a chat.

Spectrum recordings (2026-11-26, working copy, not released; `maarse-core/src/sweeprec.rs`). A
long run of sweeps is kept as one small file, the "MSR2" format: 5 minutes at 10 sweeps a second is 205 KB
of sweeps and a few hundred bytes of file. All numbers are little endian. The recorder takes the strongest
value of each step in each time bucket, `bucket ms = max(ceil(duration ms / 256), 1000)` (at most 256
buckets; 1 s for up to 4 minutes, 1.17 s for 5, 2.3 s for 10: with fewer than about ten sweeps in a bucket the
strongest value is mostly noise). A sweep that is not 64 steps wide is ignored. Sweeps with a known packet on
the air or an overloaded receiver (most steps stronger than -60 dBm) are *masked*: they count as time, not
as a measurement.

The *usual peak* of a step is the middle of its bucket peaks, taken from the first 16 buckets (or all, in a
shorter recording) and then followed half a dB a bucket towards each new value (a peak is compared with peaks:
against the mean the strongest of a few dozen noise samples is 6 dB or more over it nearly everywhere), with no
step stronger than 2 dB over the middle of all steps, so a constant carrier does not become its own floor. The
*level* of a cell is how far its strongest value is over the usual peak of the step, in steps of 3 dB, rounded,
1 to 15; 0 means under the threshold (default 6 dB) or not measured.

| Part | Size | Content |
|---|---|---|
| header | 21 | `"MSR2"`, flags u8 (bit 0: a list of buckets without a measurement follows the floors), threshold dB u8, bucket ms u16, buckets u16, sweeps u16, first frequency kHz u32, step kHz u16, strongest dBm i8, interval u8 (units of 0.1 s), floor base dBm i8 |
| floor | 32 | the usual peak of each step, dBm: the base plus a 4-bit signed difference in dB (-8 to 7), two steps a byte, step 0 in the low nibble |
| masked | ceil(buckets/8) | only if flag bit 0: bit k = bucket k has no measurement |
| profile | the rest | the level of every cell, step by step from step 0, each step over all its buckets, range coded (below); a tail of zero bytes is left out, a reader reads zeros past the end |

The profile uses the range coder of LZMA (11-bit probabilities initialised to 1024, shift 5, 32-bit range; its
first byte, always 0, is not written). Each cell is coded as a bit "above the threshold" and, if so, the level
minus 1 as 4 bits, high bit first, in a binary tree of 15 nodes (node 1 first, then `2 node + bit`). Context of a cell,
with `a` the level of the previous bucket, `a2` the one before that and `u` the level of the step below in the same
bucket (0 outside the recording): the bit uses `min(a,2) * 3 + min(u,2) + (a2 > 0 ? 9 : 0)`; the tree nodes use
the context `a > 0 ? a : u` (16 contexts of 16 nodes, after the 18 of the bit). A reader reads buckets x 64 cells.

The file has no events and no groups: a reader finds them. An *event* is a connected area (four neighbours) of
cells with a non-zero level, found in the order of its first cell, step by step. Readers join events that look alike
(centres within 2 steps, widths within 3 steps) into groups and call each group *constant* (on more than 80 % of the
time), *periodic* (three or more, regular), *rising* (one long event whose last third is more than 3 dB stronger than
its first third), *long*, *pulses* or *single*; `Recording::groups` in `maarse-core`, the port in `maarse-client` and
the one in the Android app do this the same way (`tools/test_sweeprec.mjs` and `SweepRecTest` check the ports byte for
byte against `maarse-core/tests/data/sweeprec-golden.*`, written by `GOLDEN_WRITE=1 cargo test -p maarse-core golden`).
A reader also gives the duty of a step (share of measured buckets with a level) and its highest level (floor plus
the highest level, at most the strongest dBm of the header).

Files of the first format, "MSR1" (24-byte header, floor, highest level and duty of every step, 120 buckets of run-length
coded levels, the events), are still read; no writer makes them any more. Real data, 5 minutes at 10 sweeps a second in a
busy band: MSR1 with the strongest-against-mean floor 3,469 bytes, MSR1 with the floor above 781 bytes, MSR2 595 bytes
(a file of 20 s: 69 bytes).

The web client and the Android app record from the live sweeps while a radio is connected (maarse-host, USB or
Bluetooth). The radio ends a run after 2 minutes, so the client stops and starts it again every 105 s; the gap (a
fraction of a second) is a masked part of a bucket.

maarse-host asks every USB or Bluetooth modem for its counters every 10 s and
passes them on in extra `Stats` frames after its own (keys 30-59 and 80-117 that
the host does not set itself), so a client of the host sees the radio's GPS,
Bluetooth and USB numbers. A Wi-Fi modem pushes its own every 30 s.

### 9.7 Local-only commands

State-changing radio administration, including remote OTA operations, remains
restricted to the administrator list. The receiver persists a per-administrator
reserved counter ceiling before applying a change. On restart it rejects
counters at or below that ceiling. The reservation covers 60 additional counter
values to limit flash writes during a burst. Read-only commands do not reserve
flash. This policy preserves the existing control wire format.

`SetAdmins`, `SetWifi`, `SetIdentity` and `SetRangeTest` are refused when they
arrive through `Remote`. Through `Remote`, firmware accepts `GetStats` (with or without a key list),
`GetRadio`, `SetRadio`, `ConfirmRadio`, `GetNeighbors` (the first 8 neighbours in one frame), `SetRole`, the OTA commands (downgrade flag
ignored) and the three spectrum recording commands (`SweepRecStart`, `SweepRecStop`, `SweepRecStatus`).

**Radio change on trial** (`maarse-core/src/radiotrial.rs`). A `SetRadio` through `Remote` applies the settings but leaves
flash alone and starts a timer (default 5 minutes, at most 30). `ConfirmRadio` from an administrator saves what is in use. If the timer
runs out first, the node returns to the settings in flash (a reboot during the trial does the same, as flash never changed). A second
`SetRadio` during a trial restarts the timer but still returns to the settings that were in flash before the first one. The reply to
`SetRadio` is sent before the switch; the administrator's own radio must hear the node on the new settings for `ConfirmRadio` to arrive.
Stats key 118 shows the seconds left (second stats frame).

---

## 10. Versioning and compatibility

| Mechanism | What an older node does |
| --- | --- |
| Packet version (flags bits 7–6) | Rejects any version ≠ 0. Version `11` is used on purpose for non-Må:rse frames (region hello `0xFF`, range-test beacons `0xD7 'R' 'T'`) so that nodes never forward them. |
| New packet types | Rejected as unknown (type 15 is free; 14 is `MailSync`). Types 11 (`Carry`), 12 (`Hello`) and 13 (`KeyRequest`) are rejected by nodes from before they existed: older nodes simply do not learn from hellos and do not answer key requests. |
| Region flag (bit 0) | Nodes from before regions reject the packet, so region floods are not forwarded through them; floods without a region are bit-for-bit unchanged. |
| Delivered notice (`Carry` without path) | Nodes without the notice drop `Carry` packets without a path. |
| App-data TLVs (type ≥ 0x03) | Skipped by length; receivers read only the bytes they know inside a field. New fields must come after `0x02`. Nodes that predate TLVs stop at the first unknown byte, which is harmless because `0x01`/`0x02` come first. |
| Stamp version | The version is inside the stamp; difficulty can be raised with a new version, and old versions removed from `difficulty()`. Nodes from before stamps reject stamped announces (length check) and new nodes drop unstamped ones: this was a breaking change. |
| Ratchet vs. static seal | New nodes read both formats. FS is only used towards contacts that announce `APP_FS` or have sent an FS message. |
| Compact FS messages (`0xF2`) | Sent only to contacts that announce `CAP_COMPACT`. A node without it never receives one (and a stray one is tried as a static message and fails). Version 1 session files load. |
| Trace route | Older forwarders pass `Probe` packets unchanged; their SNR is missing. |
| Local repair, edge bridge, anchors | No format change; any node can be on such a path. |
| Companion frames | Optional fields are appended at the end (e.g. identity in `Info` and `Heard`); decoders accept their absence. Unknown command types give `Error`. Unknown stats keys must be ignored. |
| Backbone | A host without region hello support counts the hello as one invalid packet and gets all floods. |

Rules for future changes:

1. Do not change the meaning of existing bytes; append.
2. A change that older nodes would misinterpret (rather than drop) needs a new
   packet type, a new flag, or a new version.
3. Prefer formats that older nodes drop silently over formats they forward
   wrongly.

---

## Notes for maintainers

Discrepancies found while writing this document. The spec follows the code.

1. **Roles in GUIDE.** `docs/GUIDE.md` ("Routing") lists the roles as
   `Client`, `Repeater` and `Relay`; the code also has `Anchor` (3)
   (`Role` in `router.rs`). The doc comment of companion command `0x10 SetRole`
   also lists only 0–2.
2. **Position size in GUIDE.** GUIDE ("Position and telemetry") says the
   app-data fields take "at most 41 of 99 bytes"; with the name (`0x05`) and
   role (`0x06`) fields the maximum is 67 bytes, as `appdata.rs` states.
3. **Announce interval in `stamp.rs`.** The module doc says announces are sent
   every 15–30 minutes; the code sends every 600 s (host) and 900 s (firmware).
4. **Path request retry in `stamp.rs`.** The module doc says path requests
   repeat after 15 s; `PATH_RETRY_MS` is 20 000 ms (45 000 ms until 2026-11-26).
5. **Packet type 5 `Control`.** Defined in `packet.rs` ("Remote control of
   repeaters") but never sent; remote control uses content type `0x02` inside
   an encrypted `Data` message.
6. **Packet type 7 `Probe`.** Documented in `packet.rs` as "coverage
   measurement for the map"; the code uses it only for trace route.
7. **`RadioConfig` byte 15** was always 0 (reserved); since 2026-11-15 it is
   the announce interval in quarter hours, where 0 keeps the default (12 h),
   so older clients and nodes stay compatible.
8. **GUIDE Wi-Fi formats.** GUIDE ("Wi-Fi backbone") describes the first
   byte of `SetWifi` and `Wifi` as `enabled`; in the code it is a flags byte
   (bit 0 enabled, bit 1 modem). GUIDE does mention bit 1 elsewhere.
9. **`ratchet::announces_fs`** stops at the first field that is not `0x01` or
   `0x02`. That is correct only as long as `0x02` stays before every TLV
   field, which `appdata::build` guarantees.
10. **`maarse-core/src/lib.rs`** refers to "Må:rse – protocol specification
    v0.1"; this document replaces that reference.


### MeshCore service enrollment (working copy)

`/meshcore/session` uses a WebSocket behind the backbone's HTTPS endpoint. It
sends a one-use 32-byte random challenge as hexadecimal with `version: 1`.
The client requests `PersonSetup` operation 10 from its local host, with an
operation byte (`0` access, `1` ensure, `2` announce, `3` scan, `4` scan-results,
`5` public-history, `6` public-send, `7` public-read, `8` direct-list,
`9` direct-history, `10` direct-send, `11` direct-read, `12` channel-list,
`13` channel-join, `14` channel-leave, `15` channel-history, `16` channel-send,
`17` channel-read, `18` repeater-login, `19` repeater-command, `20` repeater-status) followed by the raw
challenge. For operation bytes 6, 7, 9–11 and 13–20, append the exact operation string
including its argument so the signature covers the payload.
The host sends `0xbc` proof chunks with request, offset and total as little-endian
`u16` values followed by the bytes. Concatenate the 64-byte device public
identity, 129-byte certificate, 64-byte signature and signed device list.

The WebSocket request contains the corresponding operation name and `device`,
`certificate`, `signature` and `list` as lowercase hexadecimal. The Ed25519
signature covers SHA-256 of `maarse/meshcore-access/v1`, the challenge, the
length of the operation (little-endian `u64`), the operation bytes and the
16-byte person address. The backbone verifies device membership, compares the
signed device list with the highest version it has stored, and checks the
administrator's grant before acting. Device revocation takes effect on that
backbone once it has received a newer signed device list; grant revocation is
checked on every request. The connection returns `allowed`, and for permitted
users the person and a nullable `publicKey`. An `ensure` request may also return
an error if the radio worker is unavailable. `announce` and `scan` acknowledge
that the action was queued. `scan-results` reports `scanning` while pending and
returns `results` once complete. `public-history` returns up to 100 recent
Public messages, `unread`, `lastId` and `totalUnread`. History and read responses include `totalUnread` across Public, joined channels and direct conversations. The operation `public-send:<hex>` queues
a message of at most 90 UTF-8 bytes, and `public-read:<lastId>` advances the
person's shared read cursor monotonically. Public send is acknowledged as
queued. `direct-list` lists the public keys of conversations with stored
history. `direct-history:<key>` and `direct-read:<key>:<lastId>` use the same
response and read cursor as Public, scoped to the 64-hex-digit contact key.
`direct-send:<key>:<hex>` queues up to 60 UTF-8 bytes. The worker looks up the
contact among adverts it has heard and reports failures in the next history
read. Direct-message delivery acknowledgements are not exposed by this API.

Channel requests use `channel-join:<name>:<hex-key>`, `channel-leave:<name>`,
`channel-history:<name>`, `channel-send:<name>:<hex-text>` and
`channel-read:<name>:<lastId>`. An empty key selects the first 16 bytes of
SHA-256 of `#` followed by the channel name. Channel keys stay in the private
worker directory and the channel list returns names only.

Repeater requests are `repeater-login:<key>:<hex-password>`,
`repeater-command:<key>:<hex-command>` and `repeater-status:<key>`. The worker
requires an administrator bit from the repeater's MeshCore login reply before
sending a command. `repeater-status` returns the latest login or CLI response.
The temporary login queue is removed on pickup or after roughly one second
if the worker is unavailable.


### Host receive limits (working copy)

The PC host validates an incoming file offer against the configured maximum and
requires exactly `max(1, ceil(size / chunk_size))` chunks. Each chunk must match
its expected authenticated plaintext length. The host accepts up to 32 active
incoming files, with at most eight per sender. A pull-completion receipt is
issued after checking the received size and digest. These checks use the
existing wire format.

The page server serves regular files of up to 2 MiB from its own page tree.
Symbolic links and Windows reparse points are excluded. Image conversion also
limits decoded dimensions to 4096 by 4096 pixels. Unreadable access rules deny
access to the affected content. The operator still owns and protects the data
directory against concurrent local changes.
