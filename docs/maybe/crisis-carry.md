# Crisis: no internet, parts of the LoRa net destroyed

Status: simulated (2026-11-17), nothing built in firmware. Simulator:
`node tools/sim-experiments/crisis.mjs [seeds]` (env `TRAVELLERS`, `HOURS`,
`VARIANTS`).

## Scenario

National preset. At t = 0: no internet anywhere; every repeater, mast and host
within 25 km of Aarhus, Odense, Copenhagen and Aalborg destroyed; a third of
the remaining repeaters gone at random. Handhelds are in role "auto" (relay
after 2 minutes without a repeater) with mobile mode on. 30 handhelds drive
back and forth at 70 km/h between their town and another town 40-120 km away;
the rest stay put. One direct message every 20 s from 10 minutes to 6 hours,
half to someone within 25 km; measured until 8 hours. 5 seeds, ~5,250
messages.

## What was changed to test it

All off by default; firmware unchanged.

- `maarse-core`: carry lifetime and room are settings (`carry_ttl_ms`,
  `carry_max`; feature `big-carry` gives room for 128), `carry_for` lets a
  post office hand mail to carry, and **smart carry** (`carry_smart`):
  1. copies go only to neighbours that say they move (hello flag);
  2. a moving carrier that meets new neighbours tries ordinary routing from
     there (at most every 10 minutes per message);
  3. the sender re-routes its own carried message only when it regains a
     repeater, not every 5 minutes.
- Simulator: `carry_ttl_s`, `carry_max`, `office_carry`, `carry_smart`.

## Results (5 seeds, 30 travellers)

| Variant | < 25 km | 25-100 km | > 100 km | Airtime |
|---|---|---|---|---|
| No carry | 95 % | 79 % | 20 % | 97,465 s |
| Carry as today (4 copies, 30 min, 8 in RAM) | 95 % | 81 % | 21 % | 215,697 s |
| + 6 h, 64 in flash | 95 % | 77 % | 14 % | 369,085 s |
| + 16 copies | 95 % | 77 % | 15 % | 374,848 s |
| + offices hand to carriers | 95 % | 78 % | 16 % | 360,372 s |
| **Smart carry** | 95 % | 81 % | 21 % | **149,350 s** |
| Smart + 6 h, 64 | 96 % | 82 % | 23 % | 212,696 s |
| **Smart + 6 h + offices** | 96 % | **83 %** | **24 %** | 214,893 s |

With 100 travellers (3 seeds): no carry 91 / 78 / 21 %; smart + 6 h +
offices 95 / **88** / 25 %.

## What it says

- **Locally (< 25 km) the net survives**: auto mode turns handhelds into
  relays, 95 % either way.
- **Carry as built today barely helps and costs a lot**: the same delivery
  for 2.2 times the airtime. The cost comes from the sender trying to route
  its carried message again every 5 minutes while near a repeater (a path
  request flood each time), and from copies handed to neighbours that never
  move.
- **Longer life and more copies make it worse** without smart carry: the
  floods choke the channel (> 100 km falls from 21 % to 14-16 %).
- **Smart carry fixes the cost**: the same delivery as today's carry for
  31 % less airtime, and with long life and offices it is the best on every
  distance, at the airtime today's carry already costs.
- **Between distant islands (> 100 km) carry cannot do much**: 20 → 24 %.
  Only people who happen to drive the right way carry anything, and 8 hours
  is short. More movement helps the middle distance most (100 travellers:
  25-100 km 78 → 88 %).

## Recommendation

1. Replace today's carry with smart carry (one rule set, not a new
   function): copies only to moving neighbours, route on arrival, no
   5-minute re-route loop. It is cheaper even without a crisis.
2. Carry in flash with a longer life (hours) only together with smart carry.
3. Post offices may hand mail to carriers when off the backbone.
4. Not simulated yet: summary exchange when nodes meet (Bloom filter),
   channel messages and alerts in carry, reboots, crisis detection.
