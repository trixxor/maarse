# Routing

> **Type:** explanation · **Reader:** someone who wants to know why routing is built this way · **Status:** design `simulated` in `maarse-sim` and the Denmark simulator; `bench-tested` on two boards; **not field-tested**. Exact rules: [protocol-spec.md, section 5](../protocol-spec.md#5-routing). Source: `maarse-core/src/router.rs`.

## The problem

LoRa is slow and shared. A short message can take a second or more on the air,
every node in earshot hears it, and a node may only transmit about 10 % of the
time. A routing scheme therefore has to spend as little airtime as possible, and
still deliver when nodes come and go.

## The idea: flood once, then follow a route

The simplest scheme is a **flood**: every node that hears a message repeats it.
It is robust, but every message costs airtime on every node, and in a busy area
copies start colliding.

Må:rse floods only a small **path request** (it carries the first message along).
Every repeater it passes adds itself to it. The recipient does not take the first
copy: it picks the copy whose links had the **best signal quality** and sends the
route back. After that, messages are **source routed**: they carry the list of
repeaters, and only those repeaters transmit.

## Why acknowledge every hop

With an end-to-end acknowledgement only, a packet lost on the last hop restarts
the whole journey from the sender. Må:rse repeaters confirm each handover, so a
lost packet is resent on the hop where it was lost. The recipient still sends an
end-to-end acknowledgement, which is the "delivered" mark the user sees.

## What happens when it breaks

| Situation | Behavior | Spec |
| --- | --- | --- |
| No acknowledgement within 20 s | The route is forgotten and the message is sent again with a new path request, up to 2 times | [5.6](../protocol-spec.md#56-route-repair) |
| A repeater's next hop stops answering | The repeater sends straight to a node further along the path, cutting out the dead hop, and remembers it for 5 minutes | [5.7](../protocol-spec.md#57-local-repair) |
| Nobody can reach the recipient at all | A relay may carry the message and hand it on when it meets someone ([Post offices and carry](post-offices-and-carry.md)) | [5.10](../protocol-spec.md#510-store-carry-forward-and-the-delivered-notice) |
| Route longer than 12 hops | An anchor splits it at the host, so each side may be 12 hops | [5.8](../protocol-spec.md#58-anchors-paths-beyond-12-hops) |

## Roles

A node's role decides whether it forwards for others: **client** (no),
**repeater** (yes), **relay** (yes, and carries what it cannot deliver; forest
mode, at most 4 hops), **anchor** (yes, and splits long routes). **Auto** turns a
handheld into a relay only when no repeater has been in range for two minutes.
See the [architecture overview](../architecture/overview.md#roles).

## What is deliberately not done

These were tried and dropped; the numbers are in the decision log.

- Scaling the flood slot with spreading factor on its own: no gain at SF11, worse
  at SF12 ([decision 0004](../decisions/0004-load-control-rejected.md)).
- A per-minute path request quota: saved airtime but lowered delivery
  ([decision 0004](../decisions/0004-load-control-rejected.md)).
- Separate channels or faster spreading factors for long-distance links
  ([decision 0003](../decisions/0003-one-radio-setting.md)).

## Compare

Meshtastic floods everything; MeshCore routes along learned paths but has no
built-in internet link; Reticulum routes globally but announces cost a lot of
airtime on LoRa. A table with strengths and weaknesses is in the
[README](../../README.md#why-another-mesh) and the long form in
[compare.md](../../compare.md). These comparisons come from documentation and the
simulator, not from field measurements.
