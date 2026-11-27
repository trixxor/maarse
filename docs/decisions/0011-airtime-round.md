# 0011: Airtime round: compact frame on, lean header and others not built

- **Status:** accepted (compact frame, path request tuning) · parked (the rest)
- **Date:** 2026-11-26 to 2026-11-27

## Context

A short direct message is about 80 % overhead (a 64-byte seal plus an 18-byte
header). Path requests are 38-69 % of airtime in the simulator.

## What was built and switched on

- **Compact forward-secrecy frame** (`0xF2`): 22 bytes overhead with a full 16-byte
  tag, announced by capability `CAP_COMPACT` (`0x40`); average overhead 29.7 / 33.6
  bytes against 69. Simulated gain: forest -17 %, car -30 %.
- **Path request tuning:** retry after 20 s (was 45 s), and a repeater forwards 80 %
  of the path requests it hears (`REQUEST_FWD_PERMILLE` 800); hosts forward all.
  If route discovery fails on boards, set `request_fwd_permille` to 1000 first.

## What was not built

| Idea | Why not |
| --- | --- |
| Lean header (shorter addresses, session AEAD) | Needs a security review and a header-version bump |
| Preamble 8 | Needs a bench test |
| Announce policies (jitter, partial forwarding) | No effect in simulation |
| Preset deflate dictionary for pages | Measured -30 % on pages; not built yet |
| Codec2 700C/450 | Mode switch; not built |

## Consequences

- The simulator was optimistic about the old frame (preamble 8, no 64-byte seal):
  an honest car scenario went from 954 s and 97 % to 2426 s and 81 %.
- The firmware change had not been built here when decided (a pre-existing build
  error in `sweep_step_fast`), so it needs a stack probe before flashing
  (`tools/stack_probe.py`) and an external review (gate 5) before it is relied on.
- Real traffic counters are the next step before further optimization.

## Evidence

[airtime-optimisation.md](../maybe/airtime-optimisation.md), sections 6-9;
`tools/sim-experiments/airtime_round.mjs`; 75 crypto tests and 3 million fuzz
iterations on the compact frame. Branch `airtime-compact-frame` (not merged to `main`
at the time of writing).
