# Later features (noted 2026-11-27, not started)

## 1. Canary and history for the network tests

Builds on the Ping test, Network crawl and DHT check (web client Diagnostics, Android Experimental).

- A scheduled canary: one ping (or a short ping run) to chosen nodes every N minutes, over LoRa, TCP or both in turn,
  with the result kept in a history on the host (loss, round trip, hops, worst SNR).
- A scheduled crawl (for example once an hour) that stores the links with their SNR, so a change shows up: a link that
  became uneven or one-way, a repeater that stopped answering, a new neighbour.
- A DHT check on a schedule (the host runs it itself), with a note when the status gets worse.
- Alerts through the existing alert system when a canary fails several times in a row.
- Keep the airtime in view: a canary over LoRa has a budget (a setting, default off), and the interval is never below
  the ping minimum of 10 seconds.
- Not built either: a LoRa-versus-TCP comparison by packet id (diff of what two observers heard). The ping test with
  LoRa and TCP in turn covers the common case.
