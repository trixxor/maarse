# 0006: Regional routing with custody transfer and home mailbox

- **Status:** parked (not started)
- **Date:** 2026-11-15

## Context

For long LoRa-only distances (Denmark-wide with many users, or no internet in a
crisis), mobile networks use home registers and message centres. Floods across a
country do not scale.

## Idea

1. A backbone of fixed anchors that know the next anchor towards each region
   (proactive, no floods across the country).
2. **Custody transfer** between anchors: each stores the message in flash and
   acknowledges the previous one, so it survives outages.
3. A personal **home mailbox** registered in the backbone; a user's node asks
   "fetch my mail" from anywhere.
4. Later, hardware: a separate fast backbone channel between mast repeaters
   (this conflicts with [decision 0003](0003-one-radio-setting.md)).

## Why parked

For Denmark today (20-40 hops, internet usually available) the current design
(anchors up to 64 hops, TCP backbone, DHT, post offices) is enough. The gain
comes with many users or no internet, and the cost is a new packet format and
firmware on all repeaters.

## How to resume

Start with a corridor simulation: A end-to-end as today, B anchors with custody,
C B plus home mailbox. Do not touch the protocol first.

## Related

[Post offices and carry](../concepts/post-offices-and-carry.md),
[crisis-carry.md](../maybe/crisis-carry.md).
