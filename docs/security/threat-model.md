# Threat model

> **Type:** explanation · **Reader:** a reviewer or operator deciding how far to trust Må:rse · **Status:** `working copy`, written 2026-11-27 from the [protocol specification](../protocol-spec.md), the [security review of 2026-11-20](../security-audit-2026-11-20.txt) and the code. **The cryptography has not been reviewed by an external cryptographer**, and the forward-secrecy ratchet is a draft. Do not rely on Må:rse where a break would cause harm.
>
> This page states the protection **and the remaining trust boundary** for each area. Where the project has not decided or measured something, it says so.

## What is being protected

| Asset | Where it lives |
| --- | --- |
| Message content | in packets, in post offices, in carriers, in history on hosts |
| Identity keys (Ed25519 + X25519) | `maarse-data` on hosts, flash on boards, the app's data |
| The recovery phrase (24 words) | the user's own storage |
| Who talks to whom, when and from where (metadata) | visible on the air and on the backbone, see below |
| Availability of the network | radio channel, repeaters, backbone hosts |
| Firmware and release integrity | signed images and releases |
| Admin access to servers and hosts | admin password, access key |

## Who the adversaries are

| Adversary | Can do |
| --- | --- |
| **Passive listener on the radio** | Hears every LoRa packet in range, including headers |
| **Malicious repeater or backbone operator** | Sees and can drop, delay or replay packets it forwards; sees destinations and routes |
| **Malicious post office or carrier** | Holds sealed messages for others; can delete or withhold them |
| **Someone with a channel key** | Reads and writes in that channel as anyone |
| **Flooding or spamming node** | Tries to exhaust airtime, post offices or the node table |
| **Local attacker on the same machine or network as a host** | Reaches loopback and LAN web listeners |
| **Supply-chain attacker** | Tries to deliver bad firmware or a bad release |

Out of scope: an adversary who controls a user's device or can force the user to
unlock it; jamming the radio band (nothing in the protocol can prevent it);
traffic analysis by someone who can observe many places at once.

## Protections

| Area | Protection | Trust boundary that remains | Spec |
| --- | --- | --- | --- |
| Direct messages | End-to-end encrypted (static seal, Noise `X`-style); the sender address is masked and only the recipient learns it; messages are authenticated | No external review; the destination address is in clear in the header | [6.2](../protocol-spec.md#62-static-seal-direct-messages-without-a-session), [3.1](../protocol-spec.md#31-layout) |
| Forward secrecy | Ratchet between contacts that support it | **Draft.** A leaked current key exposes the current session. Mail in post offices is sealed without forward secrecy (static format) | [6.3](../protocol-spec.md#63-forward-secrecy-ratchet-draft) |
| Delivery status | A recipient confirms only a message it opened, with an 8-byte proof only the two parties can compute (working copy) | Older recipients are confirmed by anyone who heard the message. Post office deposits, bundles and file chunks have no proof | [5.5](../protocol-spec.md#55-end-to-end-acknowledgement) |
| Session state and replays | Hosts save the sending chain to disk before a message leaves, and ignore messages already received (working copy) | Restoring an old `sessions.bin` can reuse keys: delete it after a restore. Radio nodes keep no replay list | [6.3](../protocol-spec.md#63-forward-secrecy-ratchet-draft) |
| Bluetooth to a radio | A 6-digit code with Bluetooth LE Secure Connections; with the factory code only a new one can be chosen; guessing is slowed (working copy; tried on a Heltec V4 with an Android phone) | Until the owner has chosen a code, anyone in range can do so with the factory code. A fixed passkey can be learned by someone who interferes with pairing | [9.8](../protocol-spec.md#98-the-bluetooth-code-working-copy-2026-11-27) |
| Files on a host | The data folder and secret files are for the owner only on Unix; the Windows service limits its folder to SYSTEM and administrators | Not encrypted on disk | – |
| Man in the middle | 60-digit safety number that two people can compare | Only works if people actually compare it | [6.1](../protocol-spec.md#61-identities) |
| Channels | Symmetric encryption with the channel key | Public and `#hashtag` channel keys are derived from the name, so anyone can read them. **The claimed sender is not authenticated**: a key holder can write as anyone ([decision 0008](../decisions/0008-channel-sender-signing-later.md)) | [6.4](../protocol-spec.md#64-channels) |
| Announces | Signed; need a proof-of-work stamp (about 0.3 s on an nRF52840) | Limits fake identities to a few per second per device; it does not stop a well-resourced attacker | [4.3](../protocol-spec.md#43-proof-of-work-stamps) |
| Path requests, key requests, mail syncs, deposits | Smaller stamp (8 bits) | Cheap for one message, costly for a flooder; a determined attacker can still use airtime | [4.3](../protocol-spec.md#43-proof-of-work-stamps) |
| Files | Encrypted chunks with a random per-file key inside the encrypted offer; the host checks size, digest and chunk geometry; at most 32 active incoming files, 8 per sender | Host receive limits are in the working copy | [6.5](../protocol-spec.md#65-file-chunks) |
| Alerts and bulletins | Signed by issuer keys; receivers trust only configured roots and keys | Trust is whoever holds the root key | [6.6](../protocol-spec.md#66-signed-bulletins) |
| Firmware updates | Ed25519-signed manifest; a node installs only for its own board, a newer version, and a signature matching its built-in release key; OTA is disabled with a missing, weak or development key | Whoever holds the release private key. **Back it up and protect it**: without it field nodes cannot be updated | [6.7](../protocol-spec.md#67-firmware-update-signing) |
| Releases | `SHA256SUMS` signed with the release key | Same key | [6.7](../protocol-spec.md#67-firmware-update-signing) |
| Recovery phrase | BIP39 24 words; the node identity **is** the master identity | Anyone with the phrase is you; it is not stored by the project | [6.8](../protocol-spec.md#68-recovery-phrase) |
| Web client and API on a host | The credential-free path requires the request authority to match the real listener (loopback or its numeric IP) and a same-origin browser request; others need the access key; header size, time and connection limits; `--require-key` for phones and shared computers | Put a reverse proxy in front for public access and require the key; query-string keys are disabled by default | [GUIDE](../GUIDE.md#public-web-access-with-caddy-and-lets-encrypt) |
| Server admin | Password stored as a hash, sessions stored as hashes, lockout after 5 failures for 15 minutes; changing the password revokes sessions | The dashboard is plain HTTP unless you put HTTPS in front | audit item 2 |
| Backbone links | Per-IP and per-link rate limits, bans, block lists, a limit of 4 DHT contacts per network | A single operator with many networks is not stopped | [GUIDE](../GUIDE.md#dedicated-server-maarse-server) |
| Moved nodes in the DHT | A host drops its own radio path to a node only for another gateway's record with a valid gateway proof (or a configured endpoint) and an announce signed by the node that is more than an hour newer than the one heard on radio (working copy) | Any identity can sign a gateway record, so someone holding such an announce can make the host drop the radio path until it hears the node again | [8.3](../protocol-spec.md#83-dht-purpose-and-records) |
| Page server | Regular files up to 2 MiB; links, special files and reparse points excluded; image size limited | The operator must protect the data directory against concurrent local changes | audit item 3 |
| History on hosts | Optional (`--no-history`) | **Stored unencrypted** in `<data>/history` by default (kept 90 days) | [settings reference](../reference/settings.md) |

## Metadata: what is visible

- **On the air:** the destination address of every packet, the path of repeaters
  (as short IDs), packet type, size and timing. The sender is never in the header.
  Anyone with a LoRa receiver in range sees this.
- **Announces:** a node's public keys, its role, and, if the user opted in,
  position and telemetry (position is off by default and rounded to a grid when
  shared).
- **On the backbone and in the DHT:** which hosts hold which addresses, and
  traffic between hosts.
- **Public telemetry** (opt-in by the operator): packet counts and observations of
  verified announces; message contents stay encrypted.

Må:rse gives **no anonymity**: it protects content and hides the sender inside a
message, not the fact that two nodes are communicating.

## Known weaknesses and open items

1. Cryptography not externally reviewed; ratchet is a draft.
2. Channel sender unauthenticated ([decision 0008](../decisions/0008-channel-sender-signing-later.md)).
3. Host history is unencrypted on disk by default.
4. The local-only trust on loopback means any local program can reach a host that
   does not use `--require-key`.
5. A compromised release key would let an attacker sign firmware for every node
   that trusts it. A new key means visiting every node.
6. A forged packet can make a node restart a forward-secrecy session, which then
   starts again at the weaker first-message level (noted for later work).
7. No closed networks ([decision 0005](../decisions/0005-network-key-maybe.md)): any
   node can route through any repeater.
8. Dependency review of 2026-11-20 found no advisories classified as vulnerabilities,
   one unmaintained-package warning (`paste`, transitive) and nine informational
   soundness warnings (`heapless`).

## How this page is kept honest

When a change touches security guarantees, update this page and the specification
in the same change ([AGENTS.md](../../AGENTS.md)): describe the protection and the
remaining boundary, and distinguish released behavior from the working copy,
simulation and physical tests.
