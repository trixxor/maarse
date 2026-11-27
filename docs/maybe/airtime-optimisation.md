# Airtime optimisation: what is left to win

Status: investigation, 2026-11-26. Nothing in this document is built. Numbers
marked **measured** were computed with `maarse-core/src/airtime.rs`'s formula
(AN1200.13) at the default radio settings (868 MHz, 125 kHz, CR 4/5, preamble
16, SF9) or run on files in this repository. Numbers marked **estimate** come
from the literature or from reasoning and have not been tested here.

## 1. The real cost of a small packet

At SF9 / 125 kHz one symbol is 4.1 ms. The preamble alone is 20.25 symbols
= **83 ms**, and the smallest possible packet (18 B) takes **218 ms**. So:

| Packet (SF9) | Bytes | Airtime |
| --- | ---: | ---: |
| Hello | 22 | 239 ms |
| Ack (3-hop path) | 25 | 239 ms |
| Path request | 40 | 321 ms |
| Direct message, 22 B text, 3-hop path | 112 | 648 ms |
| Channel message, 40 B text | 99 | 587 ms |
| Announce (name, role, caps) | 180 | 955 ms |
| File chunk (186 B data) | 237 | 1 201 ms |

Two facts follow:

* For short packets the **fixed cost** (preamble, explicit header, 8 payload
  symbols) is a third to a half of the airtime. An ack carries no data and
  still costs 239 ms.
* A 22-byte direct message is **80 % overhead** (18 header + 7 path + 64 seal
  + 1 type). Compressing the text further helps very little until the
  overhead shrinks.

Where the simulator says the airtime goes (national scenario, cold start,
`tools/sim-experiments/airshare.mjs`): announces 52 %, path requests 33 %,
hellos 4.5 %. Announce scope 3 has since cut total airtime by 16-32 %.

One message over 3 hops (data forwarded 3 times, ack back 3 times) costs
**2.66 s** of airtime in total at SF9 (0.80 s at SF7).

## 2. Ranked levers

Savings are for the whole 3-hop message above (**measured** arithmetic, not a
network simulation) unless stated.

| # | Lever | Where | Saving | Cost / risk |
| --- | --- | --- | ---: | --- |
| 1 | Session AEAD without a 64 B seal per message | Data | **-33 %** of a short DM | New session state; replay and reboot handling |
| 2 | Hash-chain announce beacons | Announce | **-60 %** per announce (955 -> 370 ms) | New packet type, trust model change |
| 3 | 8-byte addresses and ack tokens on source-routed packets | all | -10 to -15 % | Header version bump, collision analysis |
| 4 | Preamble 16 -> 8 | all | -7 % (-33 ms of every packet) | Breaks sleepy receivers; all nodes must agree |
| 5 | Window acks and larger chunk payload | files | **+35 % throughput** (129 -> 175 B/s) | Protocol change in `files.rs` |
| 6 | Trained text model instead of `textz` | text | 22 B -> ~11 B; another -5 % of a short DM | Model shipped in firmware, versioned |
| 7 | Dictionary for pages | pages | **-30 %** (measured, 7 772 -> 5 422 B over 9 pages) | Dictionary versioning |
| 8 | Lower-rate voice codec plus silence removal | voice | -40 to -65 % | Quality, per-codec packaging |

Combined, levers 1, 3, 4 and 6 take a short DM from 2.66 s to **1.36 s
(-49 %)** at SF9 and from 0.80 s to 0.40 s at SF7. Lever 2 attacks the
largest share of the cold-start airtime.

### 2.1 Session AEAD (lever 1): the biggest single win

Today every direct message is either a static seal (`e_pub 32 + sender 16 +
tag 16` = 64 B) or a ratchet message (`type 1 + ratchet key 32 + sender mask
16 + n 2 + pn 2 + tag 16` = 69 B). The 32-byte ephemeral/ratchet public key
and the 16-byte masked sender address travel in **every** message.

Signal's double ratchet only needs the new public key when the DH ratchet
steps. The rest of the time a message can be:

```text
type/flags (1) | counter (2) | ChaCha20-Poly1305 ciphertext | tag (8)
```

That is about 11 B instead of 64-69 B. The sender is implied by the session
(looked up from a short session ID derived from the counter and the
destination), so the masked sender field goes too. An 8-byte tag halves the
forgery margin to 2^-64 per attempt, which is acceptable on a link where an
attacker also has to be in radio range and cannot try millions of guesses; a
12-byte tag is the conservative choice and still saves 52 B.

Keep the static seal for the **first** message and for contacts without a
session. The open gaps listed in 6.3 (replayed init, unencrypted headers)
should be fixed in the same change.

### 2.2 Hash-chain announce beacons (lever 2)

An announce is 156-180 B: two public keys (64 B) and an Ed25519 signature
(64 B), repeated every 15 min to 12 h and flooded 3 hops. The start-up wave
re-sends each announce about 32 times. Receivers only need the keys once.

Proposal: the full announce stays, but also **commits to a hash chain**
(`h_n = H(h_{n-1})`, a 16-byte anchor, signed). Later "I am here" beacons
carry `destination | timestamp | app data | next chain element (16 B)` and no
keys or signature: about 60 B, 370 ms instead of 955 ms. A receiver that
holds the full announce checks that the element hashes to the committed
anchor. Receivers without the keys use the existing `KeyRequest`.

Trade-offs: an attacker who hears a beacon first could replay it with a
different timestamp or app data. Bind the app data into the chain element
(`h = H(prev | app data | timestamp)`) and accept only the next unused
element. This needs a security review before building; the cryptography is
simple but the failure modes (chain exhaustion, reboot without storage) are
not.

### 2.3 Shorter addresses on the wire (lever 3)

`destination` is 16 B in every packet, and the ack token is 16 B. A routed
packet already has a path of 2-byte IDs; 8 B of destination is enough to find
the end of the path and an 8-byte ack token (`msg_id` 8 B, no `!msg_id`) is
as good. For floods (path request, channel, key request) 8 B of 2^64 is
still collision-free at national scale. The decoder needs a version bump in
the flags byte (bits 7-6 are free). Saving: 8 B per data packet, 8 B per ack,
and 16 B from a path request (`dest` 8 + `reply token` 8).

### 2.4 Preamble (lever 4) and radio settings

16 preamble symbols are 33 ms more than 8 at SF9. They exist so that
receivers that wake periodically can catch a packet. Repeaters and anchors
are always listening, and the simulation already showed that sleepy handhelds
are not worth it, so 8 symbols are enough for the always-on network. Test on
the bench with the two boards (COM6 / COM7) at the weakest SNR the network
works at before changing the default. This respects "same radio settings
everywhere"; it changes them everywhere at once.

What does **not** help: a higher coding rate (already the minimum, 4/5),
implicit header (needs fixed length packets) and a lower SF (conflicts with
range, and the rule that all nodes share settings).

### 2.5 Files (lever 5)

Each chunk is 237 B on the air for 186 B of data (22 % overhead) plus a
239 ms end-to-end ack: **129 B/s**. With 8-byte addresses and a shorter file
ID/tag (about 12 B overhead instead of 26), a chunk can carry about 226 B in
a full 255 B packet. A cumulative window ack for every 4 chunks (the host
already pulls in batches; the firmware still acks each chunk) gives
**175 B/s**.

Also worth doing, in rising order of effort:

1. Content-addressed chunks: skip chunks the recipient already has
   (re-requested files, edited pages).
2. Deltas (bsdiff/VCDIFF) for updated pages and firmware images.
3. Fountain codes (RaptorQ) over multi-hop paths so the receiver never has to
   say which chunks are missing.

### 2.6 Text (lever 6)

`textz` is a 230-entry static codebook (about 56 % on short chat). It cannot
learn context. A pre-trained order-2/3 context model (PPM or a small
logistic-mixing model, a few KB of tables) typically reaches **35-45 %** on
short chat text. **Estimate only**: this repository has no chat corpus to
train or test on. Before building it, collect 5-10 thousand real Danish and
English short messages (with consent) and measure.

### 2.7 Pages (lever 7)

Measured on the nine sample `.mu` pages (16 511 B raw):

| Method | Total | Share of raw |
| --- | ---: | ---: |
| raw deflate (today) | 7 772 B | 47 % |
| LZMA, 64 KB dictionary | 7 873 B | 48 % |
| deflate with a preset 32 KB dictionary (leave-one-out) | **5 422 B** | **33 %** |

A better entropy coder does not help; a **preset dictionary** does, because
the markup syntax (headings, links, board and picture tags) repeats across
pages. Use the dictionary from the pages the host serves plus the markup
keywords, version it in the file's MIME (`;deflate;dict1`) and keep the
current format working. Binary markup tokens would add a little more but make
pages harder to debug; skip that.

### 2.8 Pictures (what does not help)

Measured on the six demo pictures at a 1 300 B budget, greyscale:

* **WebP vs AVIF:** PSNR within a few dB of each other, AVIF better on 3 of 6
  and worse on 3 of 6. Not worth a second decoder in the clients.
* **JPEG:** worse than WebP at this size.
* **1-bit dots:** the 4x4 ordered dither is periodic, and a JBIG2-style
  adaptive context model (10-pixel template) was **no better than PNG** (and
  up to 2.5x worse) because it cannot see a period-4 pattern. A better dots
  format would encode the **un-dithered** low-resolution grey levels at 2-3
  bits and let the client dither, which is what WebP greyscale already does.
* **Neural image codecs** (learned compression) advertise 20-40 % under
  JPEG/WebP at very low bitrates. **Not measured here** (no model
  available). They need a large decoder (tens of MB) in every client and
  produce plausible detail that is not in the original, which is wrong for
  maps, signs and evidence. A reasonable experiment on the host only, behind
  a "may be reconstructed" label, but it is not where the cheap wins are.

The sharper lever for pictures is the **transport**: a 1.3 KB picture over a
3-hop path is 8 chunks and about 12 s at SF9. Lever 5 takes about 3 s off.

### 2.9 Voice (lever 8)

The web client already encodes **Codec2 at 1200 bit/s** (10 s = 1.5 KB) and
sends it as a file. Measured airtime for 10 s of speech over one hop at SF9,
including acks:

| Rate | Bytes | Chunks | Airtime |
| --- | ---: | ---: | ---: |
| 1200 bit/s (today) | 1 500 | 9 | 12.9 s |
| 700 bit/s (Codec2 700C) | 875 | 5 | 7.2 s |
| 450 bit/s (Codec2 450) | 562 | 4 | 5.8 s |
| 300 bit/s (neural, e.g. LPCNet-style) | 375 | 3 | 4.3 s |

Codec2 700C and 450 are in the same `codec2` crate that `maarse-voice` already
compiles, so they are a mode switch, not a new codec. Add **silence removal**
(voice activity detection; typically 20-30 % of frames in conversational
speech, **estimate**), and speech-to-text on the sender as an option
("send as text", 10 s of speech is about 40 B).

Opus (content type 0x06) stays TCP-only; the lowest usable Opus rate is far
above LoRa's capacity.

## 3. What to do first

1. **Measure real traffic.** Add per-type byte and airtime counters to the
   host stats and read them from a real network for a week. The simulator's
   52/33/4.5 % split is a cold-start picture.
2. **Cheap and low risk, near-term:** preamble 8 (bench test), pages
   dictionary, Codec2 700C plus silence removal, window acks for files.
3. **The big one:** session AEAD with a short header (-33 %), together with
   8-byte addresses (one header version bump, designed once).
4. **After a security review:** hash-chain announce beacons.
5. **Needs data:** trained text model.

Every item that changes the wire format must go through the compatibility
rules in section 10 of the protocol specification (capability bit or header
version) and be re-run in the simulator (`tools/sim-experiments/final.mjs`)
before it is flashed.

## 4. How the numbers were made

* Airtime: `maarse-core/src/airtime.rs` formula, ported line for line,
  preamble 16 and 8, SF7 and SF9, 125 kHz, CR 4/5, explicit header, CRC on.
* A 3-hop message: three data transmissions with a 7-byte path, three acks
  with the same path, no retransmissions.
* Pages: raw deflate (level 9, zlib window 15) over
  `docs/sample-pages/**/*.mu`; dictionary built from the other eight pages
  (leave-one-out).
* Pictures: Pillow 13 (WebP method 6, AVIF speed 0, JPEG optimised) on
  `docs/sample-pages/backbone/demo/*.png`, side from 256 down, quality from 80
  down, first to fit; PSNR against the original resized to the same size.

## 5. Frame and overhead budget

Added 2026-11-26. Same method as section 4. "Lean" is a concrete frame
design (below), not a measurement of a built system.

### 5.1 The layers of one frame

Every transmission pays at three layers (radio settings: explicit header,
CRC on, preamble 16, see `firmware/common/src/radio.rs`):

| Layer | Cost today | Can we change it? |
| --- | --- | --- |
| LoRa PHY | preamble 16 + 4.25 symbols, 8 symbols minimum payload (header, CRC), rounded up to whole 5-symbol blocks | preamble only (16 -> 8); the rest is fixed by the chip; implicit header needs fixed-length packets |
| Må:rse header | flags 1, hops 1, destination 16, path count 1 + 2 per hop, optional region 2 | yes, protocol version bump (flag bits 7-6) |
| Crypto + content | direct message: seal 64 (or ratchet 69) + type 1; channel: 40; chunk: 26; announce: 64 keys + 64 signature + 5 stamp | yes, session state and trust model |

Byte layers of a 22-byte direct message over 3 hops (112 B, 648 ms at SF9):
header 18, path 7, seal 64, type 1, **text 22 (20 %)**. Time layers: PHY
fixed cost 18 %, header and path 22 %, seal 57 %, text 20 % of the bytes.

The airtime is a staircase: at SF9 every 4.5 extra bytes cost 20.5 ms (one
5-symbol block), at SF7 3.5 bytes cost 5.1 ms, at SF10 5 bytes cost 41 ms. A
saving of 1-2 bytes can therefore vanish, so the lean design below was
checked block by block, not byte by byte.

### 5.2 The lean frame used for the numbers

```text
flags (1)  version 2 | type 4 | path | region
hops/count (1)  for routed packets: hop index 4 bits | path count 4 bits
                (MAX_PATH = 12 fits); flood packets keep a full hops byte
destination (8)  was 16
path (2 per hop)  unchanged
```

* Ack token: 8 (`msg_id`) instead of 16 (`msg_id | !msg_id`).
* Path request: reply token 8 instead of 16.
* Session message: `type/flags 1 | counter 2 | ciphertext | tag 8`, so 11 B
  of overhead plus the content type byte, instead of 64 or 69.
* Static seal (first message, no session): unchanged at 64 B.
* File chunk: file ID 2 (agreed in the offer), index 1, tag 8: 11 B instead
  of 26.
* Hello: destination 8 instead of 16.
* Announce beacon: destination 8, timestamp 4, app data 24, chain element
  16 (section 2.2).

An 8-byte destination collides with probability 2^-64 per pair, so it is
safe far above national scale. A 16-byte value is only needed where it is a
public key hash, and that stays in the full announce.

### 5.3 Savings per frame (SF9, 125 kHz, CR 4/5)

| Frame | Today B | Lean B | Today ms | Lean ms (preamble 16) | Lean ms (preamble 8) | Saving |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Hello | 22 | 14 | 239 | 198 | 165 | **31 %** |
| Ack, 3-hop path | 25 | 16 | 239 | 198 | 165 | **31 %** |
| Path request (0 hops recorded) | 39 | 23 | 300 | 239 | 206 | **31 %** |
| Path request (6 hops recorded) | 52 | 35 | 361 | 280 | 247 | **32 %** |
| DM 22 B, static seal (first message) | 112 | 103 | 648 | 607 | 574 | 11 % |
| DM 22 B, ratchet session | 117 | 50 | 669 | 361 | 329 | **51 %** |
| DM 22 B, session + text model | 117 | 39 | 669 | 300 | 267 | **60 %** |
| Channel message, 40 B text | 99 | 91 | 587 | 546 | 513 | 13 % |
| Announce, full | 180 | 180 | 955 | 955 | 923 | 3 % |
| Announce, hash-chain beacon | 180 | 54 | 955 | 382 | 349 | **63 %** |
| File chunk, 186 B data | 237 | 213 | 1 201 | 1 099 | 1 066 | 11 % |

The same frames at SF7 save 26-32 % for the small frames and 52 % for a
session message; at SF10 the percentages are the same within 3 points. The
saving does not depend much on the spreading factor.

What to read from this:

* **Small frames (hello, ack, path request) lose about 31 %**, almost all of
  it from the header diet plus preamble 8, because the preamble is 35-48 % of
  their airtime. No crypto change is involved, so they are the safest win.
* **A session message loses half.** The saving is the 69 -> 11 B overhead
  change; it needs the ratchet to keep session state, which it already does.
* **The first message to a new contact only loses 11 %.** Contact setup is
  therefore still expensive; the lever there is the announce (below).
* **Channel messages and chunks barely move** (10-13 %): their overhead is
  the AEAD tag and a nonce, which are small next to the payload. Their lever
  is compression (text model) and batching, not framing.
* **A full announce does not shrink** (3 %). Its 128 bytes of keys and
  signature are the payload. Only the hash-chain beacon saves airtime.

### 5.4 Effect on the whole network

Weighting the table with the simulator's cold-start airtime split
(announces 52 %, path requests 33 %, hellos 4.5 %, data and acks 10.5 %,
`airshare.mjs`):

| Change set | Airtime vs today |
| --- | ---: |
| Header diet + preamble 8, announces still full | **80 %** (-20 %) |
| Same, and the start-up wave uses beacons | **49 %** (-51 %) |

This is **arithmetic on the cold-start mix**, not a simulation. In normal
operation the mix is different (more data, fewer announces), so the real
saving is likely between the two for the header diet and smaller for beacons.
Re-run `tools/sim-experiments/airshare.mjs` and `final.mjs` with a prototype
before quoting a number to anyone.

### 5.5 Risks and costs of the lean frame

| Change | Risk |
| --- | --- |
| Hops/count in one byte | Decoder rewrite; the fuzz targets in `maarse-fuzz` must cover it |
| 8-byte destination | One flag-bit or version bump; old nodes drop version 1 packets, so a transition period needs both formats (capability bit like `CAP_TEXTZ`) |
| 8-byte ack token | Dedup and ack matching use the 16-byte token today; derive both from the same 64-bit `msg_id` |
| 8-byte AEAD tag | Forgery chance 2^-64 per guess; reasonable on a radio link but a design decision. A 12-byte tag still saves 49 of 64 bytes |
| Short session counter (2 B) | Needs window handling for lost and reordered messages; the ratchet's `n`/`pn` logic already does this |
| Hash-chain beacons | Needs a security review (replay, chain exhaustion, state after reboot) |
| Preamble 8 | Bench test at the weakest SNR the network must work at |

### 5.6 Order of work

1. Preamble 8 and the header diet for hello, ack, path request. About 31 %
   on the frames that are 40-50 % of the cold-start airtime, no crypto
   change.
2. Session frame with 11 B overhead (about 51 % on every message after the
   first).
3. Chunk overhead 26 -> 11 B and window acks (section 2.5).
4. Hash-chain beacons after review.

Do 1-3 in **one** version bump so old nodes see one incompatibility, not
three.

## 6. Simulator test (2026-11-26)

Script: `tools/sim-experiments/airtime_round.mjs` (raw numbers in
`tools/sim-experiments/airtime-round-results.json`). The five scenarios of
`final.mjs` (5-8 seeds each), the "today" profile, one change at a time.

New simulator knobs, all off by default (the committed wasm and the default
results are unchanged): `preamble`, `lean_header` (a *size model*: only the
airtime of the lean frame, no new wire format), `dm_overhead`,
`announce_jitter_ms` and `announce_fwd_permille` (the last two also in
`maarse-core` `Router`, off by default, `cargo test -p maarse-core` passes).

### 6.1 The simulator was optimistic

The simulator used preamble 8 (the firmware default is 16) and counted a
text message as 60 bytes in total (34 compressed), without the 64-69 byte
seal. The "honest" baseline uses preamble 16 and `69 + 1 + 40` bytes:

| Scenario | Legacy sim | Honest |
| --- | --- | --- |
| forest | 630 s, 99.1 % | 950 s, 98.9 % |
| car on the E45 | 954 s, 97.1 % | **2 426 s, 80.7 %** |
| town | 7 807 s, 61.4 % | 8 133 s, 54.7 % |
| national | 49 070 s, 85.0 % | 51 392 s, 83.3 % |
| outage journey | 98 623 s, 39.1 % | 99 690 s, 36.0 % |

The earlier published results (for example the car at 97.1 %) were therefore
too good for real boards, most of all where data dominates.

### 6.2 Side finding: national, town and outage have moved

With the engine of today (hills and height map since 2026-11-19) national
gives 85 % and 49 000 s, the outage journey 39 %, against 96 % / 24 000 s and
98 % in `final-results.json`. The committed wasm gives the identical numbers,
so this is not from the new knobs. `regress-baseline.json` is from
2026-11-16 and is stale. These three scenarios are in a duty-cycle-saturated
regime now, so the gains below are small there. Forest and car are the
trustworthy ones. Nobody has looked into why the numbers moved.

### 6.3 Results against the honest baseline

Airtime change and change in the share delivered (percentage points):

| Change (vs honest baseline) | forest | car | town | national | outage |
|---|---:|---:|---:|---:|---:|
| Preamble 16 -> 8 | -8 % air, -0.2 pt | -21 % air, +12.0 pt | +2 % air, +0.5 pt | -1 % air, -1.9 pt | -0 % air, +0.7 pt |
| Lean header | -9 % air, +0.4 pt | -9 % air, +9.0 pt | -5 % air, +3.0 pt | -5 % air, -0.1 pt | -4 % air, +0.0 pt |
| Session frame, 16 B tag | -26 % air, +0.4 pt | -51 % air, +14.6 pt | -3 % air, +2.9 pt | -2 % air, +1.8 pt | -0 % air, +0.7 pt |
| All three | -42 % air, -0.2 pt | -52 % air, +14.7 pt | -13 % air, +10.0 pt | -13 % air, +4.9 pt | -7 % air, +0.0 pt |
| Announce jitter 2 s | -5 % air, -1.0 pt | +2 % air, +6.2 pt | +5 % air, -0.5 pt | -2 % air, -1.5 pt | +0 % air, +1.1 pt |
| Announce forwarded by 70 % | -5 % air, -3.0 pt | -7 % air, +5.6 pt | -6 % air, -0.6 pt | -1 % air, -1.0 pt | -1 % air, +1.3 pt |
| Announce scope 2 | +1 % air, -0.8 pt | +11 % air, +3.9 pt | -3 % air, +0.3 pt | -1 % air, +0.2 pt | -1 % air, +0.0 pt |
| *Baseline: legacy -> honest* | 630 -> 950 s, 99.1 -> 98.9 % | 954 -> 2426 s, 97.1 -> 80.7 % | 7807 -> 8133 s, 61.4 -> 54.7 % | 49070 -> 51392 s, 85.0 -> 83.3 % | 98623 -> 99690 s, 39.1 -> 36.0 % |

### 6.4 What the simulator says

* **Session frame (16-byte tag): the biggest win where data is the load**
  (forest -26 %, car -51 % with +14.6 points delivered). It does little in
  national, town and outage, where path requests dominate.
* **Preamble 8:** car -21 % and +12 points, forest -8 %, nothing elsewhere.
* **Lean header:** a steady -4 to -9 % everywhere, with better delivery in
  car and town.
* **All three together:** forest -42 %, car -52 %, town -13 %, national -13 %,
  outage -7 %, with delivery up 5-15 points where the network is loaded.
* **Announce policy did not help.** Jitter, 70 % forwarding and scope 2 move
  airtime by -7 to +11 % in both directions with no pattern, and 70 %
  forwarding costs 3 points in the forest. Not adopted. The earlier "announces
  are 52 % of the airtime" was a cold-start picture: in these runs announces
  are 11-21 % and **path requests are 38-69 %**.

### 6.5 Limits

* 5-8 seeds per scenario, no confidence intervals. Differences under about
  5 % are not distinguishable from noise (for example the jitter and
  forwarding rows).
* `lean_header` and `dm_overhead` change the airtime only; a real frame
  change also needs the code, the fuzz targets and the compatibility rules.
* Window acks, chunk overhead, text model, pages dictionary and voice are
  not in these scenarios (no file or voice traffic) and are untested.
* Duty cycle and queue effects in saturated scenarios make delivery jump
  with small airtime changes; read those points with care.
* **Path requests are now the biggest line** and none of the tested changes
  attacks their number, only their size. Fewer path requests (a longer
  route cache, answers from hosts' caches) is the next experiment.

## 7. Why national and outage moved, and fewer path requests (2026-11-26)

### 7.1 Bisect of the drop in national

`final-results.json` (national 96 %, 24 300 s) is reproduced at commit
`92b5fda`. Commit `37d5926` ("host-to-host mail ops go over radio/trunks
when there is no TCP", 2026-11-19) takes it to 89 % / 41 200 s, and
`b5dcf1c` (height map) to 85 % / 48 700 s. Setting `mail_tcp_only` in the
current engine gives national 94.5 % / 35 700 s and the outage journey
-24 % airtime. In the national preset hosts have internet but not always a
TCP link between each pair, so their mail goes over LoRa: probably a model
gap (it should go over another host's TCP), not a real finding. Not fixed.

### 7.2 Fewer path requests

In the forest run 130 messages cause 132 path requests and **778 path
request transmissions** (about 6 per request) against 81 data
transmissions. The router has no time-based route expiry (the 30 minute
constant only decides whether a newer route replaces an older one), so
requests come from new destinations, dropped routes and mobile nodes.

Change against the honest baseline (airtime, delivered points), 5-8 seeds:

| Change | forest | car | town | national | outage |
| --- | ---: | ---: | ---: | ---: | ---: |
| Request jitter 1 s | +7 %, -0.4 | -21 %, +10.9 | +1 %, +0.7 | -2 %, -1.6 | -2 %, 0 |
| Request jitter 3 s | -2 %, 0 | -1 %, +4.6 | 0 %, +1.2 | -3 %, +0.1 | -3 %, 0 |
| **Forward 80 % of requests** | -10 %, -0.2 | -28 %, +8.5 | -1 %, +0.8 | -11 %, +0.2 | -13 %, +0.7 |
| Forward 60 % of requests | -18 %, -1.2 | -29 %, **-7.8** | -4 %, +1.5 | -26 %, 0 | -26 %, 0 |
| Retry after 90 s | -2 %, -0.7 | +2 %, -4.0 | -1 %, +2.0 | -3 %, -0.8 | -2 %, 0 |
| Retry after 20 s | -2 %, +0.4 | -25 %, +13.7 | 0 %, +3.2 | -4 %, +2.3 | 0 %, 0 |
| `mail_tcp_only` (the old rule) | 0 | 0 | 0 | -16 %, +7.3 | -24 %, +1.3 |
| Host proxy replies | 0 | 0 | 0 | -5 %, -1.5 | 0 |

Forwarding 80 % of path requests is the one that helps everywhere without a
delivery loss (-1 to -28 %). At 60 % the car loses 7.8 points. A shorter
retry (20 s) helps the loaded car and does no harm elsewhere. Both need a
bench check on real boards (a request that is not forwarded costs a retry
when the only route ran through that node) and more seeds before they
become defaults. The knobs are `request_fwd_permille`, `request_jitter_ms`
and `path_retry_s`, all off by default.

### 7.3 Where this leaves the list

* Reliable: the three frame changes (section 6), -13 % to -52 %.
* Promising, needs more seeds and a bench test: 80 % path request
  forwarding, 20 s retry.
* Not worth it: announce jitter, announce forwarding share, announce scope,
  request jitter, host proxy replies.

## 8. Proof plan before a deep change (2026-11-26)

Goal: no wire-format or crypto change reaches a board until we have shown
(a) a real improvement and (b) that security is no worse. Each gate has a
pass condition written down **before** the work starts. A failed gate stops
the work; nothing later is started.

### Invariants the change must keep (checked at gate 1 and tested at gate 2)

1. Full 16-byte AEAD tag on every message and chunk.
2. Full 16-byte addresses in every HKDF info and AAD; only the wire field is
   shortened. A wrong 8-byte match must fail to decrypt, never deliver.
3. Forward secrecy and post-compromise recovery as today (ratchet kept).
4. A key and nonce is never used twice, also after a reboot with lost state
   and after the 2-byte counter wraps (rekey before wrap).
5. Replay, reordering and loss handled with a bounded window.
6. A passive listener learns no more about the sender than today.
7. Proof-of-work stamps and signatures on announces unchanged.
8. Old nodes keep working (capability bit); turning the feature off
   restores today's format.

### Gates

| Gate | What | Pass condition |
| --- | --- | --- |
| 0. Real baseline | Bytes and airtime per packet type counted in the host and read from real nodes for 7 days | Projected saving from the real mix is at least 25 % of airtime per delivered message; otherwise do only the cheap items (preamble, 80 % request forwarding) |
| 1. Security requirements | A short document: the invariants above, a threat model (forgery, replay, misdelivery by 8-byte collision, traffic analysis, state loss), and the known gaps of spec 6.3 fixed in the same change | Written, and read by a second person |
| 2. Prototype and tests | New frame in `maarse-crypto` / `maarse-core` behind a capability bit | Known-answer vectors; flipping every single bit of header, counter and tag fails; replay, reorder, loss, jump and wrap tests; state-loss reboot test; simultaneous-start test; two addresses with the same 8-byte prefix do not decrypt each other's traffic; a differential test (new frame and old frame carry the same plaintext); `maarse-fuzz` parser target at least 24 h with no crash |
| 3. Simulator with real bytes | The simulator runs the new code instead of the size model; stale baseline (`regress-baseline.json`) refreshed and the mail model gap (section 7.1) fixed first | 20 paired seeds per scenario; the 95 % interval of the airtime change per delivered message excludes zero in forest and car (target at least 25 % lower); no scenario loses more than 1 point of delivery; mixed old and new nodes tested |
| 4. Bench | Two boards on COM6 and COM7, `tools/radio_counters.py` | Measured airtime per message matches the model within 10 %; for preamble 8, packet error rate at the weakest working SNR (attenuator, at least 2 000 packets per step) is not worse than at 16 by more than 1 point |
| 5. External review | The session format and the 8-byte address change, written up, reviewed by someone who is not the author, ideally a cryptographer | No open finding rated above low |
| 6. Staged rollout | Capability bit off by default; a few nodes for two weeks; the same counters as gate 0 | Real airtime per delivered message at least 25 % lower than the gate 0 baseline, no rise in undecryptable messages, no security finding; otherwise switch the bit off |

### How to be sure of the improvement

* The target is a number per scenario, fixed before the run, not read after.
* Compare paired seeds with a confidence interval, not averages of 5-8 runs.
* Measure **airtime per delivered message** and **delivered share** together:
  in duty-cycle-saturated runs delivery jumps with small airtime changes.
* Keep the control: the same scenarios on the old format, same seeds.
* Compare the real network with the same counters before and after.

### How to be sure of the security

* Tests for every invariant, written first, and run in CI.
* Nothing is shortened inside a cryptographic binding.
* No new primitive; only the existing ChaCha20-Poly1305, X25519, HKDF.
* One version bump, one review, and a kill switch that restores today's
  format without flashing.

### Abort conditions

* Gate 0 shows less than 25 % projected saving.
* Any invariant test fails and the fix would need a shorter tag or a shorter
  binding.
* Delivery drops more than 1 point in any scenario at gate 3 with no known
  cause.
* The reviewer finds a problem in the session format that cannot be fixed in
  the same frame.

## 9. Built and measured (2026-11-26)

### 9.1 What was built

| Item | Status |
| --- | --- |
| **Compact session frame** (`0xF2`, 22 bytes of overhead, full 16-byte tag, `CAP_COMPACT`) | Built in `maarse-crypto` (ratchet), used by `maarse-host` and the firmware `seal`; protocol spec 4.2, 6.3 and 10 and the README updated |
| Ratchet step at most every 8 messages once a new key was heard (never before the first step) | Built, part of the above |
| Session version 2 storage, version 1 files still load | Built |
| Path request forwarding share, jitter, retry time | Router knobs (`request_fwd_permille`, `request_jitter_ms`, `path_retry_ms`), off by default; no setting in the web client or Android yet |
| Preamble 8 | Not a code change: `RadioConfig.preamble` already accepts 6-1000 symbols. Needs a bench test on all nodes before it is used |
| 8-byte destination, hops/count in one byte, 8-byte ack token (lean header) | **Not built.** Only a size model in the simulator (-4 to -9 %). It needs a new packet version and short-address lookup through the whole router, and the 8-byte destination needs the external review of gate 5 |
| Announce policy (jitter, forwarding share, scope) | Removed again: it gave nothing |

Not compiled or flashed: the firmware crate does not build on this machine for
any change (a pre-existing `sweep_step_fast` error in the radio code, outside
this work), so the two-line firmware change is unverified, and the extra 9
bytes in `Session` need a `tools/stack_probe.py` check before flashing.

### 9.2 Tests

* `maarse-crypto`: 75 tests (63 before). New: overhead is 22 and the first
  messages carry the key; roundtrips both ways; every single bit of a compact
  message flipped is rejected and leaves the session untouched; replay
  rejected; out of order and loss inside the skip window; a third party or a
  node that lost its state cannot read it; the old API never sends compact;
  session serialization and version 1 files; 40 sessions with colliding ids
  tried one by one; a step at least every 8 messages once a key was heard;
  known lengths; random loss with retries and swapped messages.
* `maarse-fuzz`: new `fuzz_open_compact`, 3 000 000 iterations without a panic
  and without a changed message opening; `fuzz_open_ratchet` 1 000 000.
* `maarse-core` 171 and `maarse-host` 161 tests pass (one web test fails only
  in a parallel run and passes alone).

Measured in the tests (average overhead of the encryption, long form 69):

| Traffic | Overhead |
| --- | ---: |
| Mixed chat (a third of the messages from the other side) | **29.7 bytes** |
| Strictly alternating, one message each way | **33.6 bytes** |
| Unreadable messages, 20 % loss with three tries and up to 20 % swapped order | 0.1 % (long form: 0 %) |

### 9.3 Simulator, honest baseline (preamble 16, 69-byte overhead)

`TCPONLY=1` (the mail rule from before 2026-11-19, section 7.1), 5-8 seeds. The
compact frame is modelled with 34 bytes (the strictly alternating worst case).
Airtime change against the honest baseline, and delivered share:

| Scenario | Honest | Compact frame | + preamble 8 | + path request tuning | + lean header (model) |
| --- | --- | --- | --- | --- | --- |
| forest | 950 s, 98.9 % | **-17 %**, 98.6 % | -28 %, 99.1 % | -28 %, 97.3 % | -35 %, 98.9 % |
| car on the E45 | 2 426 s, 80.7 % | **-30 %**, 91.0 % | -43 %, 93.4 % | -55 %, 96.9 % | -61 %, 97.3 % |
| town | 8 133 s, 54.7 % | -3 %, 57.5 % | -4 %, 58.7 % | -7 %, 66.7 % | -14 %, 69.3 % |
| national | 42 989 s, 90.7 % | -3 %, 91.0 % | -3 %, 90.5 % | -20 %, 92.7 % | -24 %, 92.5 % |
| outage journey | 75 853 s, 37.3 % | 0 %, 38.0 % | -8 %, 38.0 % | -18 %, 36.7 % | -23 %, 36.7 % |

The earlier estimate (-26 % forest, -51 % car) assumed 19 bytes of overhead
and an 8-byte destination. The built frame is 22 to 34 bytes and the header is
unchanged, so the real gain from what is built is **-17 % and -30 %**. The
rest of the earlier numbers needs the lean header and the preamble. The path
request tuning is in the router but off by default.

Per message through one repeater (SF9, preamble 16, 40 byte text, 3 hops, data
plus ack): **0.99 s today, 0.80 s with the compact frame**; capacity at a 10 %
duty cycle 363 to 447 messages per hour (8 700 to 10 700 a day).

### 9.4 Against MeshCore

MeshCore figures are from its own documentation (docs.meshcore.io/packet_format,
docs.meshcore.io/payloads and the community spec at
swaits.github.io/meshcore-spec v0.1.0). Not found anywhere: default radio
settings, airtime per packet, throughput or a hop limit, so the airtime below
uses our radio settings for both (SF9, 125 kHz, CR 4/5, preamble 16). Nothing
was measured on a MeshCore radio; the MeshCore frame sizes are computed from
its documented layout (1 byte header, 1 byte path length, 1 byte per hop,
1+1 byte destination and source hash, 2 byte MAC, AES-128-ECB in 16 byte
blocks; ack 4 bytes).

| | MeshCore (v1) | Må:rse today | Må:rse compact |
| --- | ---: | ---: | ---: |
| Direct message, 40 byte text, 3 hops | 57 B | 135 B | 88 B (96 B average) |
| Direct message, 22 byte text, 3 hops | 41 B | 117 B | 70 B |
| Ack, 3 hops | 9 B | 25 B | 25 B |
| Advert / announce | about 126 B | 156-180 B | 156-180 B |
| Airtime per message over 3 hops, 40 byte text (SF9) | **1.68 s** | 2.97 s | 2.41 s |
| Same at SF7 | 0.50 s | 0.91 s | 0.73 s |
| Messages per hour through a repeater (10 % duty) | 643 | 363 | 447 |
| Ratio to MeshCore | 1.0 | 1.8 | 1.4 |

So MeshCore is **about 1.4x lighter than the built compact frame and 1.8x
lighter than today** on the air for a direct message. The gap is mostly not
compression: it is what is paid for security.

| | MeshCore (v1, from its spec) | Må:rse |
| --- | --- | --- |
| Authentication tag | **2 bytes** (truncated HMAC-SHA256): a forged message passes with probability 2^-16 per try | 16 bytes (Poly1305): 2^-128 |
| Cipher | AES-128-ECB with zero padding: equal blocks give equal ciphertext | ChaCha20-Poly1305, a new key per message |
| Sender and recipient | 1-byte hashes of the public keys (collisions possible; the spec itself moves paths to 2-3 byte hashes) | 16-byte addresses |
| Forward secrecy | none described in the pages read | ratchet, keys deleted after use |
| Replay protection | timestamp inside the plaintext | per-message keys, a used key is gone |

Where Må:rse is still heavier and why: an 18-byte header with a 16-byte
destination (MeshCore 2 + path), a 16-byte tag instead of 2, a 16-byte ack
token instead of a 4-byte CRC. Only the destination and the ack token can be
reduced without weakening security (with review); the tag must stay.

### 9.5 Where this leaves the plan

* Built and tested: the compact frame. Real, repeatable gain -17 % (forest)
  and -30 % (car), 0 to -3 % where path requests dominate.
* Still to do for the larger figures: lean header (needs review), preamble 8
  (bench), path request tuning (settings in the web client and Android, bench).
* Gates 0, 5 and 6 of section 8 (real counters, external review, field test)
  are not done. The compact frame should not be switched on for a network
  before gate 5.

## 10. Switched on (2026-11-26)

On the user's instruction ("just activate it") two changes are now defaults in
the code, on the branch `airtime-compact-frame` (not pushed, not flashed):

* **Compact session frame:** announced with `CAP_COMPACT` (part of `MY_CAPS`),
  so a host or firmware node with this code sends it to every contact that
  announces the same bit. Older nodes never get one.
* **Path requests:** `PATH_RETRY_MS` 45 000 -> 20 000 and
  `REQUEST_FWD_PERMILLE` = 800 (80 % of path requests forwarded). Simulator,
  8 seeds, compared with the compact frame alone:

| Scenario | Airtime | Delivered |
| --- | ---: | ---: |
| forest | -9 % | +0.2 pt |
| car on the E45 | -24 % | +7.0 pt |
| town | -4 % | +5.2 pt |
| national | -13 % | +0.3 pt |
| outage journey | -16 % | -1.3 pt |

Each part alone: 80 % forwarding -7 / -39 / -1 / -11 / -15 %; retry 20 s -2 to
-6 % and +3.5 points in car and town. `docs/GUIDE.md` (the last section)
records that 45 s was chosen on purpose over 15 s on an earlier engine; with
today's engine and honest sizes 20 s was better, so it is changed here. If a
bench test shows route discovery failing more often, set
`request_fwd_permille` back to 1000 (all) first.

Not changed: preamble 16 (all nodes must change it together, bench test
first) and the packet header (no lean header, see 9.1).

## 11. Hardware test and the restart bug (2026-11-26)

Boards: Heltec V4 (ESP32-S3, COM6) and XIAO nRF52840 + Wio-SX1262 (COM7), both
flashed with the compact frame and the path request defaults. WisMesh Tag and
XIAO ESP32-S3 build but were not on USB. `tools/compact_hw_test.py COM6 COM7`
sends 28 messages of 40 bytes (12 alternating, 8 and 8 in one direction) and
measures the airtime of both radios per message.

### 11.1 A real bug, found on the first run

The first run lost every message from the XIAO to the Heltec: the router
acknowledged them, but the Heltec logged "could not be read". The test
script opens the serial port with DTR/RTS, which restarts an ESP32-S3, so the
Heltec had lost its sessions (they live in RAM) while the XIAO kept sending
compact messages. In the long form an unknown session gives `NoSession`, the
node then starts a new session (resync) and the conversation recovers. A
compact message does not name its sender, so no node could tell who to
resync with, and the sender never found out.

Fix (crypto, host and firmware):

* A session has a `force_full` flag: the next message goes in the long form,
  then the flag clears. It is set when we hear an announce from the peer (a
  restart announces within seconds) and when a static message arrives from a
  peer we have a session with (it lost its state or does not know the session).
* A node that cannot read a compact message announces within a second (at most
  once per 30 s), so the unknown sender switches to the long form.
* Loss after a restart is the same as in the old form: the one message that
  could not be read.

New tests: a restarted peer recovers (static message, long form, `NoSession`,
new session, compact again), `force_full` gives exactly one long message,
and the flag survives serialization. `maarse-crypto` has 78 tests.

### 11.2 Result after the fix

All 28 messages arrived and were acknowledged, with the Heltec restarted at
the start of the run (the case that failed). Average airtime of both radios
per message (the message and its ack, plus whatever else the radios sent in
that window): the first four messages 1 436 ms, the later ones 885 ms; the
shortest exchanges, 724 ms, are the compact frame, the 929-1 189 ms ones are
long-form messages or retries. This is a hardware measurement of one pair of
boards at 0 hops, not of a network.

Stack on the Heltec (`tools/stack_probe.py`, stats key 87, after several
connections and a restart): **14 360 bytes free** (limit 92, 15 032 before
this work, 14 376 with the first version of the frame). Free heap 37 928
bytes.

### 11.3 Not done

* WisMesh Tag and XIAO ESP32-S3 were built, not flashed or tested.
* Three hosts or more, repeaters and routes over several hops were not tested on
  hardware.
* The restart loss window is bounded by the announce (within seconds after a
  restart, or on the first unreadable compact message) but is not zero.

## 12. Follow-up: hosts forward every path request, ping test over LoRa or TCP

**Path request forwarding on hosts.** The end-to-end test (`tools/e2e_test.py`)
failed about once in twelve runs after the 80 % forwarding share was added:
a host counts its TCP neighbours, so a dropped request saved no airtime there
but cost the 20 s retry. Hosts now forward every path request
(`request_fwd_permille = 1000` in maarse-host and for simulated hosts); radio
nodes keep the 80 % share and the three-neighbour guard. After the change the
end-to-end test passed 30 runs out of 30.

**Ping test over LoRa or TCP.** Trace route (0x20) takes an optional last byte
`via`: 0 the node's choice, 1 radio only, 2 internet links only. A host with
both kinds of link forgets the route and keeps the path request and probe on
the chosen kind of link for 60 s. The web client and the Android app can send
pings automatically, over LoRa only, over TCP only, or alternately, and show
loss, round trip and hops per kind of link; the CSV has a `via` column.
Firmware ignores `via`.

Tests: workspace tests, `tools/ping_test.mjs`, `tools/denmark_sim_test.mjs`
and the two new checks in `tools/e2e_test.py` pass. All four firmware images
and the APK build. The Android ping page was compiled but not run on a device.
