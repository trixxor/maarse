# Må:rse as a network stack (compared with Reticulum)

Status: comparison and proposal (2026-11-17). Nothing here is built yet.

## The difference today

**Reticulum** is a network stack first. Applications (LXMF messaging,
NomadNet pages, Sideband, `rnsh`, `rncp`) are built on a general API:

| Reticulum | What it is |
|---|---|
| Destination + aspects | An address per application or service (`app.aspect.aspect`), many per identity; types single, group, plain, link |
| Packet | One encrypted datagram (MTU 500 bytes) |
| Link | An encrypted, forward-secret session between two destinations |
| Channel / Buffer | Reliable, ordered messages / byte streams over a link |
| Resource | Bulk transfer of any size: split, compressed, checked, with progress |
| Request / response | RPC over a link |
| Interfaces | LoRa (RNode), TCP, UDP, I2P, serial, KISS, AX.25, pipes, auto (LAN) |
| Shared instance (`rnsd`) | Several programs on one machine use one stack |

**Må:rse** is a message and post system first. The packet types are named by
what they are for (`Data` = a direct message, `Fragment` = a file chunk,
`Deposit` = mail, `Channel` = a channel message), and the content of a direct
message has a type byte for text, files, pages, calls, control commands and IP.
Programs other than our own clients cannot use the stack.

But much of a stack is already there:

| Layer | Må:rse today |
|---|---|
| Identity and address | 16-byte hash of a key pair (as Reticulum) |
| Routing | Floods, source paths via repeaters, anchors, DHT over the backbone, trunks |
| Links ("interfaces") | LoRa modem (USB, Bluetooth, Wi-Fi), TCP, UDP, serial, KISS (`extra_links.rs`) |
| Datagram | `Datagram` packets (type 10): a session key once, then 28 bytes overhead, per-hop acks only (`tunnel.rs`) |
| Reliable messages | `Data` with end-to-end acks, route repair, forward secrecy |
| Bulk | Files: up to 8 KB over LoRa, 1 MB over TCP, with resume and a file mailbox |
| **IP** | **IPv6 tunnel**: an address per node (`fd00::/8` from the Må:rse address), header compression (40 → 2-7 bytes), MSS clamping, fragments with NACK, a TUN adapter (Wintun / `/dev/net/tun`). Ping, SSH and mosh run over it. TCP always, LoRa with `--tun-lora` |
| Delay tolerance | **Post offices, carry, MailSync**: Reticulum has this only for LXMF (propagation nodes), not for any application |
| External | MQTT bridge (telemetry, positions, channels; Home Assistant) |

So Må:rse already does something Reticulum does not (IP over the mesh) and
something Reticulum does only for one application (store and forward). What
is missing is the **general layer in between** and a way **in** for other
programs.

## What is missing, and where to put it in

In order of value for the work. Each step reuses what exists.

### 1. Ports: one address, many services

Add a content type `0x09` **app datagram**: `port (u16) | data`. The port says
which application gets it, like Reticulum's aspects or UDP ports. Ports
0-1023 are for Må:rse itself (the existing content types map onto them: text,
files, pages, calls, control, IP), the rest for applications.

- Costs 3 bytes per message.
- The announce can list the ports a node offers (a few bytes), like the
  capabilities field does today.

### 2. A way in: the local API

Today only our web client and Android app talk to a node (the companion
protocol over USB, Bluetooth or the host's WebSocket). Make that a public API
on maarse-host, like Reticulum's shared instance:

- A local socket (TCP on 127.0.0.1 and a Unix socket) with a few commands:
  *open port*, *send datagram*, *open stream*, *send resource*, *received*.
  These are new companion commands, so the same API also works over USB and
  Bluetooth to a firmware node.
- Small client libraries (Python and Rust) and a command-line tool
  (`maarse-cat`: send stdin to an address and port, like `netcat`).

This is what turns Må:rse from an app into a platform. Everything after it
can be used by others.

### 3. Sessions: datagrams and streams for everyone

The IP tunnel already has the right session layer: a key sent once in an
encrypted direct message, then datagrams with 28 bytes of overhead, pieces and
NACK for large packets. Lift it out of `tunnel.rs` into the core:

- **Datagram session**: as the tunnel's, with a port. Unreliable, cheap.
- **Stream session**: the same plus sequence numbers, a small window and NACK
  (selective resend), so a byte stream arrives whole and in order (Reticulum's
  Buffer). Over LoRa a window of 2-4 packets; over TCP larger.
- The IP tunnel becomes one user of it (port "IP").

### 4. Resources: bulk with delay tolerance

Files are already a resource transfer (offer, chunks, hash, resume, mailbox).
Make it general: any port can send a resource. The difference from Reticulum:
**when the recipient is away, the resource goes to the post office** (the file
mailbox), and with carry it can travel without any connection. That is
delay-tolerant transfer for every application, not just for messages.

### 5. IP between sites, not just between nodes

Today every node has one IPv6 address (/128). Let a host announce an IPv6
**prefix** (e.g. a /64 for the LAN behind it), so whole networks are reachable
through the mesh: a sensor network in a barn, a weather station, a camp's LAN.
The tunnel already compresses headers; routing a prefix is a table in the host
(prefix → Må:rse address), learned from announces or the DHT.

Limits to say out loud: LoRa at SF9 with a 10 % duty cycle gives a few hundred
bit/s of useful capacity per channel, shared by everyone. IP over LoRa is for
ping, mosh, MQTT-SN, small sensor data and status pages, not for the web. Over
TCP, Wi-Fi modems and future fast links it is a real network.

### 6. Interoperability

- A **Reticulum interface**: maarse-host as a Reticulum interface (or the
  reverse), so the two nets can carry each other's traffic. It is already on
  the wish list in `compare.md`.
- Ports make other bridges simpler: MQTT, APRS, Meshtastic text.

## Where Må:rse would end up

| | Reticulum | Må:rse after steps 1-5 |
|---|---|---|
| Services per identity | aspects | ports |
| Datagram | Packet | datagram session |
| Reliable stream | Link + Buffer | stream session |
| Bulk | Resource | resource, **also through post offices and carry** |
| IP | no (third-party only) | **IPv6 per node and per site prefix** |
| Delay tolerance | LXMF only | **every application** |
| Runs on the microcontroller | no (RNode is a modem) | yes (firmware is a full node) |
| API for other programs | Python library, shared instance | local socket API, Python/Rust libraries |

The strengths of each stay: Reticulum's mature API and many interfaces;
Må:rse's routing on the chip, DTN for everything and IP.

## Order of work

1. Ports (content type `0x09`, ports in announces).
2. Local API on maarse-host + companion commands + `maarse-cat` + Python
   library.
3. Session layer in the core (datagram, stream), the tunnel moved onto it.
4. General resources via the file mailbox and carry.
5. IPv6 prefixes per host.
6. Reticulum interface.
