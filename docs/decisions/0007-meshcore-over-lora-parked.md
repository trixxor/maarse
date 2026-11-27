# 0007: MeshCore chats over the Må:rse message loop

- **Status:** parked (proposed, waiting for a go)
- **Date:** 2026-11-20

## Context

MeshCore in the apps needs an HTTPS session to the backbone (`/meshcore/session`,
the node signs every operation, see [spec, MeshCore service enrollment](../protocol-spec.md#meshcore-service-enrollment-working-copy)).
A phone whose only link is LoRa gets Må:rse messages but no MeshCore.

## Options

1. Tunnel the HTTPS session over LoRa. **Rejected:** every operation needs a
   signed proof plus TLS; far too chatty.
2. A **bridge identity** on the backbone sends MeshCore chats as ordinary Må:rse
   direct messages (at most 148 bytes; MeshCore text is at most about 150), and
   replies go the other way. Reuses acknowledgements, post office and carry.
3. Use file transfer for catch-up (the latest 60-100 messages), because it is
   too heavy for live chat (8 KB over LoRa, one at a time).

## Proposed

Options 2 and 3 together. Phase 1 needs no protocol or firmware change: the
bridge sends plain text with a marker the client recognises from the bridge
address. A forwarding policy per person (off, direct only, direct plus chosen
channels, everything rate-limited) keeps LoRa from drowning. LoRa-only users get
chat in and out only: no scan, admin, telemetry or node list.

## Effort

A couple of days: bridge identity and sending in `maarse-server`, a queue from the
worker's history files, policy in the dashboard and clients, specification text.
The MeshCore enrollment section of the specification needs an update at the same time.

## Status

Nothing is built.
