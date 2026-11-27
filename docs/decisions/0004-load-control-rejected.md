# 0004: Send priorities yes, SF slot only with scaled timeouts, path request quota no

- **Status:** accepted (priorities, SF slot with scaled timeouts) · rejected (measured) for the path request quota
- **Date:** 2026-11-15 to 2026-11-16

## Context

In busy or slow (high SF) networks, floods and path requests collide and replies
are lost. Three levers were tried.

## Options and results

| Lever | Result | Status |
| --- | --- | --- |
| **Send queue priorities** (`maarse-core/src/qos.rs`): acks, path responses and carry notices first, then direct messages and bulletins, channel messages, fragments, floods; a full queue drops the least urgent | Built | accepted |
| **Flood slot scaled with SF** (`qos::slot_ms`) | On its own, SF11 showed no gain (79.6 vs 80.0 % delivered) and SF12 was worse (66 % fixed slot, 55 % with the SF slot); together with scaled ack/retry/hop timeouts it helped (120 nodes SF11: 79/48 % fixed, 90/73 % with slot and scaled timeouts) | accepted only together with scaled timeouts; the firmware applies it at startup and on every `SetRadio` |
| **Path request quota** (`qos::PathQuota`, load control) | Saved airtime (national -13 %, town -23 %, car -12 %) but lowered delivery (busy scenario -11.8 points, national -2.9) | rejected (measured); off by default |

## Decision

Keep priorities and the slot-with-scaled-timeouts. Do not enable load control
quotas. Delivery and stability are preferred over airtime savings.

## Consequences

- `Router::load_control` stays an experiment (`maarse-sim --load-control on`).
- Scaling timeouts together with the slot is documented in
  [GUIDE.md, Routing](../GUIDE.md#routing).

## Evidence

`maarse-sim` 120 and 200 nodes, 5 seeds per row; `regress.mjs` with
`FEATURES='{"load_control":true}'` and the "busy" scenario (30 messages per
minute), 2026-11-16. See also [airtime-optimisation.md](../maybe/airtime-optimisation.md).
