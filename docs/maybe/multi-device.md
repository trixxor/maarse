# Linked devices: one person, several devices

Status: design note (2026-11-17). Nothing here is built in host, firmware or
client yet. The simulator models the routing and mail parts
(`tools/sim-experiments/linked_devices.mjs`, results below). Nothing is in
production, so formats change without backward compatibility.

## Goal

A person has several devices: a PC with maarse-host and a Wi-Fi modem at home,
a phone (with or without a Bluetooth LoRa modem), maybe a third device. Every
device gets the same messages, **over TCP and over LoRa**, also with no
internet at all. Example: the PC is switched off at home and the owner is on
Bornholm with the phone and a Bluetooth modem; messages still arrive on the
phone, and the PC catches up when it is switched on again.

Not a goal: depending on a particular server. nj01 and other public hosts are
post offices for everyone, not anyone's personal inbox.

## Prior art

| System | Model | Use for Må:rse |
|---|---|---|
| Signal | Every device has its own key; the sender encrypts and sends one copy per device (Sesame) | No: N copies over the air |
| Matrix | A master key cross-signs device keys; the homeserver stores, devices sync | Yes: master signs device certificates |
| Session | Same seed on every device; the network (swarm) stores, each device fetches | Yes: the network is the mailbox |
| Delta Chat | The mail server is the mailbox; messages to yourself sync sent items | Yes: self-sync of sent messages |
| Reticulum/LXMF, Meshtastic, MeshCore | Identity bound to one device or radio | Not solved |

None of them delivers to several devices over LoRa without internet.

## Design

### 1. Keys

- **Master identity M**, derived from the 24-word phrase exactly as today
  (`maarse/master/v1`). M is the person's address and safety number.
- The phrase lives **on paper**. **One** device holds the master secret (the
  master device). It signs device certificates and the device list.
- **Device key** per device (Ed25519 + X25519), generated on the device.
  Routing, announces, hop acks and forward secrecy sessions use the device
  key. Two devices never share a routing identity, so the "one identity on two
  nodes breaks routes" problem goes away.
- **Device certificate**: `"maarse/device/v1" | M | device public key | device
  index (1) | name (≤16) | signature by M`. **No expiry**: a device that was off
  for months is still valid. It ends only by revocation.
- **Signed device list**: `M | version (u32) | [device address]… | revoked
  [device address]… | signature by M`. The highest version wins (this also
  settles two masters after a restore, see section 7).

### 2. Inbox key: a weekly hash chain

Messages for M are encrypted to an **inbox key** that all of M's devices hold:

```
K(0)   = random 32 bytes, made by the master device ("root")
K(n+1) = HKDF-SHA256(K(n), info "maarse/inbox/next")
inbox X25519 secret for week n = HKDF-SHA256(K(n), info "maarse/inbox/x25519")
```

- The **week number n** is in every message header (2 bytes). The receiver
  needs no clock to pick the key.
- A device that knows K(n) derives every later week itself. **A device that was
  off for a month just takes four hash steps on start-up**, with no message and
  no network.
- The chain cannot be run backwards: deleting K(n) protects the weeks before
  (forward secrecy with one week's granularity).
- Devices keep the private keys of the current and the previous **2 weeks**, so
  late mail and senders with a stale announce still decrypt.
- The price: whoever steals a device can derive future weeks. So a
  **revocation starts a new chain** (new random root, chain id + 1). The new
  root is encrypted to every remaining device's own X25519 key and left at the
  post offices as a **key record**.

| While a device was off | Result |
|---|---|
| Only weekly rotation | Nothing to do, it derives forward |
| A revocation (new chain) | It fetches the key record from the offices (records are ~100 bytes and kept **1 year**, apart from the 7-day mail) |
| The key record is gone too | Link the device again by QR from the master device |

Senders learn the current week's inbox **public** key from the announce of
any of M's devices: `week | inbox public key | device certificate | device
signature`. The certificate can be left out of the announce and fetched with a
key request (it is ~100 bytes) to keep announces small.

### 3. Addressing: one copy to M

- The sender encrypts **once**, to M's inbox key, and sends to M.
- Routing to M: every device's announce carries "device of M". A sender (and a
  relay) keeps the route to M through **the device whose announce it got
  last**: the newest announce for the person wins. Simulated: it beats "the
  first device with a route" (see results).
- With no route to any device: the post offices, as today, but keyed on **M**,
  not on a device.
- One copy over the air, whatever the number of devices.

### 4. Post offices keep mail for M

- Offices store under M, 3 host replicas chosen by M's address (rendezvous
  hashing as today).
- Any of M's devices that syncs (`MailSync`, presence, announce) gets the
  waiting mail. The office delivers to that device and deletes after the ack,
  as today. Getting it to the **other** devices is the receiving device's job
  (section 5), not the office's: this keeps offices simple (no per-device
  cursors, no state that a lost device can block) and works on small offices.
- **Mail lives 7 days.** Only that expiry cleans up.
- "Mail waiting" hints name every device of M that the office knows.

### 5. Sibling sync: the receiving device forwards (also over LoRa)

The main trap: when the PC at home receives a message for M directly, it acks
it and the sender drops it, so the phone on Bornholm never sees it. Rule:

- **The device that receives a message for M sends a copy to each sibling
  device**, encrypted to that device's own key, addressed to that **device**
  (not to M, or it would come back). It is routed like any message: TCP when
  possible, else LoRa, through trunks if needed, and through the post offices
  (kept under the device, not under M) when the sibling is away.
- Copies of copies are never made. Duplicates (the sibling got the original
  too) are dropped by the message id.
- **Sent messages** are synced the same way: the sending device sends a
  "self" copy to its siblings.
- **Read state** is "read up to message x in conversation y" (max wins, never
  conflicts), batched and sent rarely over LoRa, at once over TCP.
- **Files over 8 KB**: over LoRa the sibling only gets a notice that the file
  exists. The content follows over TCP, as today.

Cost: a message crosses the trunks once more when the devices are on opposite
sides. For text this is small. It is the price of every device getting
everything.

### 6. Time over LoRa

- Today a firmware node gets the time only from `SetTime` (phone/PC
  connected), and the nRF52840 loses it at power-off. Announces already carry
  a u32 timestamp (0 = no clock).
- **Stratum** as in NTP: 1 = a host with NTP/GPS or a node with a phone or PC
  connected; n+1 = learned from stratum n; 0 = no time.
- **Hellos carry time + stratum** (5 bytes). Hellos are one hop, so the delay
  is the known airtime (from SF and length). Expected accuracy: a couple of
  seconds.
- **Taking the time**: the first time, from the neighbour with the lowest
  stratum. After that, only corrections over 2 s. A jump of more than 10
  minutes needs 2 independent neighbours that agree, so one liar cannot move a
  clock. The nRF 32 kHz crystal drifts ~1-2 s a day and is corrected by the
  next hellos.
- **Security does not depend on the clock**: the week number is in the header.
  The clock is used only for the 7-day expiry, when to delete old week keys
  (kept 2 weeks anyway) and display.
- **Order** in a conversation comes from the sender's **sequence number** per
  conversation, not from time. Sequence numbers also show gaps: "messages
  41-57 from Anders are missing".

### 7. Backup of the master

Two ways back:

1. **Paper**: the 24 words. The last resort.
2. **Backup file + a 6-word code** (`.maarse-backup` v2): master secret, the
   current chain root and chain id, the signed device list, plus everything v1
   holds (contacts, channels, regions). Encrypted with PBKDF2-HMAC-SHA256
   (600,000 rounds) and AES-256-GCM as v1.

The code **must be generated by the app**: 6 words from the BIP39 list (66
bits). The file can be stolen and attacked offline, with no server to stop
guesses after ten tries (Signal solves short PINs with a server enclave; Må:rse
has none and should not). 66 bits × 600,000 rounds ≈ 2^85 work, which is
enough. A 4-6 digit PIN or a self-chosen password is not.

The node cannot hand out its secret key (`GetIdentitySecret` does not exist),
so the client writes the backup when the phrase is entered or generated, while
it still knows it.

Restoring on a new device makes it the master: it publishes a device list with
a higher version that revokes the old master device. If the old master shows
up again, the higher version wins.

### 8. Linking and revoking

- **Link**: the new device shows a QR with its device public key. The master
  device scans it, signs a certificate, bumps the device list and sends the
  inbox chain state, encrypted to the new device's key (over Bluetooth, USB,
  TCP or LoRa).
- Every device shows "new device linked: <name>", so an unknown device is
  noticed.
- **Revoke**: the master device publishes a device list with the device under
  "revoked", starts a new inbox chain and leaves the key records. Offices and
  senders that see the newer list stop routing to the revoked device. Parts of
  the net that only have LoRa may learn this hours later: a stolen phone can
  read along until then.

### 9. Protocol numbers

Companion commands `0x35`-`0x37` and events `0xb5`-`0xb7` are reserved for
this (`maarse-core/src/companion.rs`, `docs/protocol-spec.md`):

| Code | Name | Content |
|---|---|---|
| 0x35 | `LinkDevice` | master side: device public key (32+32) and name → certificate |
| 0x36 | `DeviceList` | get, or (master) set/revoke: signed list |
| 0x37 | `InboxState` | install the chain id, week and K(n) (local only, like `SetIdentity`) |
| 0xb5 | `DeviceLinked` | certificate of a new device |
| 0xb6 | `DeviceListChanged` | the signed list |
| 0xb7 | `SiblingSync` | a synced item from a sibling: message copy, sent copy or read mark |

New packet content: the inbox week number (2 bytes) and chain id (1 byte) in
messages to M; hello time + stratum (5 bytes); Deposit op `KEYREC` for key
records.

### 10. Sync button and redirect hint

**Sync button** (also automatic on switch-on and when the internet returns):

1. `MailSync` to the home office at once (it fetches from the 3 replicas), as
   the timers do today.
2. A small **sync request** to each sibling: "I have up to sequence x per
   conversation". The sibling answers with copies of what the device lacks,
   **at most 6 per request** and none that are already on their way (a copy
   sent in the last 30 minutes). The device asks again when they have
   arrived. This also covers messages older than the offices' 7 days, when a
   sibling (the PC host with history) is on.
3. At most once a minute over LoRa. Mail syncs already carry proof of work.

Simulated lesson: an unpaced answer (~30 copies at once) crowded out the copies
already in flight, and the phone ended with 87 % instead of 100 %. Paced: 99 %.

**Redirect hint**: a device that forwards a copy to a sibling it heard from in
the last hour (a copy, a sync request or an announce) tells the sender, in its
ack, to use that sibling next time. The sender then sends straight to the
active device, and the message does not cross the trunks twice.

## Pitfalls

1. **Who gets it to the other devices?** Without sibling sync the device that
   got it first is the only one (simulated: the phone gets 1-3 %). Solved by
   section 5.
2. **Two devices on at the same time.** Routes to M can flip between them. The
   newest announce wins, and duplicates are dropped by id, at some airtime.
3. **Stale routes to the "home" device.** Senders keep a route to the PC long
   after the phone has announced elsewhere (acks keep it fresh). "Newest
   announce wins", not "freshest route", fixes most of it.
4. **Revocation is slow over LoRa-only parts.** A stolen device reads along
   until the new chain reaches the others.
5. **One master.** Lose the master device and the paper and you cannot link
   new devices (existing ones keep working). Hence the backup file.
6. **Late senders.** A sender with last week's key: devices keep 2 weeks.
7. **Order without clocks.** Sequence numbers per conversation, not time.
8. **Airtime.** Sibling copies over LoRa cost a trunk crossing each. Read
   marks are batched. Files stay on TCP.
9. **Small offices.** Repeater offices (4 messages per recipient) and nRF
   nodes keep no per-device state. They only carry.
10. **A month offline.** Keys: fine (hash chain). Messages older than 7 days
    are gone from the offices. The device fetches them from a sibling over TCP
    if one is on (the PC host keeps history), otherwise the gap is **shown**
    using the sequence numbers.

## Simulation

`node tools/sim-experiments/linked_devices.mjs [seeds]`: the Bornholm journey
from `journey_outage.mjs` (national preset, both LoRa trunks, mail bundles and
trunk summaries). The person has a PC node in Aalborg that stays on and a
phone that is off from 1200 s, travels and is switched on in Roenne at 4200 s.
Friends in Aalborg, Copenhagen and on Bornholm write to the person every
2 minutes. Measured: the messages written from 1200 s to the end.

Simulator options (scenario): `device_groups` (each list is one person's
devices), `sibling_sync` (section 5), `person_fresh` (newest announce wins),
`redirect_hint`, and the event `{type: "sync", node}` (sync button).
Offices keep mail under the person whenever `device_groups` is set (section
4), and sibling copies under the device.

Results, 5 seeds, about 338 messages to the person in all (67 per run),
measured until 6 hours after the internet returns (`TAIL_S=21600`). With
internet every variant with sibling sync ends with 100 % on both devices
within 2-4 s (median), so only the outage is shown:

| Variant (internet down 1000-6000 s) | Phone has before internet returns (Aalborg, Cph, Bornholm senders) | Phone / PC have in the end | Phone's median delay |
|---|---|---|---|
| Today: friends write to the phone's own address | 58 % (53, 63, 61) | 100 % / – | 5 s |
| Person, no sibling sync | 3 % (0, 0, 7) | 3 % / 97 % | – |
| Person + sibling sync | 27 % (29, 30, 22) | 100 % / 99 % | 79 s |
| + newest announce wins | 34 % (34, 31, 37) | 100 % / 100 % | 20 s |
| + newest + redirect hint | **37 %** (36, 40, 35) | 100 % / 100 % | 37 s |
| + sync button (paced) | 26 % (27, 28, 25) | 99 % / 99 % | 61 s |
| + newest + redirect + button (paced) | 32 % (31, 30, 34) | 99 % / 100 % | 32 s |
| Sibling sync, PC off too | 29 % (20, 0, 57) | 97 % / – | 214 s |
| PC off, sync button | 25 % (11, 0, 58) | 96 % / – | 86 s |

~70 sibling copies per run; airtime within ±5 % of today. "Sync button" here
is one press 30 s after switching on plus an automatic sync on both devices
when the internet returns.

An earlier version of this table measured only 1 hour after the internet
returned and counted the friends' background traffic in the denominator; it
showed ~95 %. The ~5 % were late, not lost.

Reading:

- **Without sibling sync it does not work**: whatever reaches the PC first
  never reaches the phone (3 %).
- **With sibling sync both devices end up with everything** (99-100 %), with
  internet within seconds, for about the same airtime as today.
- **With the PC off as well**, the offices keep the mail for M and the phone
  gets 96-97 % within 6 hours (the rest is still on its way).
- **During an outage** the phone gets less live than a single phone gets today
  (27-37 % against 58 %): messages that reached the PC first must cross the
  trunks again as copies. The best combination is **newest announce wins +
  redirect hint** (37 %).
- **The sync button gives no extra speed** in this scenario: the copies are
  already on their way, and the real limit is the trunks' capacity. It is
  still worth having for the cases where something was missed (a sibling that
  was off, mail older than 7 days), and it must be paced.

Open, not simulated: the cost of announces with certificates, revocation
spread, time sync, read-mark traffic, three or more devices, and the idea of
letting a sender reach both devices when both announced recently.

## Build order

1. Hello time + stratum, and sequence numbers per conversation (useful on
   their own).
2. Device certificates, device list, `LinkDevice` (0x35/0x36), QR linking in
   the client.
3. Inbox hash chain, M-addressed messages, the week number in the header,
   "newest announce wins" in the router.
4. Offices keyed on M; key records (`KEYREC`, kept 1 year).
5. Sibling sync of received and sent messages (`SiblingSync` 0xb7), read marks,
   redirect hint, sync button (paced).
6. `.maarse-backup` v2 with the master and a 6-word code; restore and revoke.
