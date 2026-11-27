# 0003: One radio setting for every node and link

- **Status:** accepted
- **Date:** 2026-11-16

## Context

Long-distance links (trunks between regions) could use a different frequency,
a faster spreading factor, or another band, which would give them more capacity.

## Options

1. Separate channels or SF7 trunks for long-distance links.
2. A faster second band (for example 5 GHz) as a given.
3. One frequency, spreading factor and bandwidth for every node, repeater and
   trunk.

## Decision

Option 3. Every device must be able to talk to every other device, because the
network is meant to work as an emergency net. Links are improved only by
**antennas and masts** (reliability) and by **software that spends the shared
channel better** (fewer floods across trunks, route caching at trunk ends,
bundling, cold standby). In the simulator, trunks use the mesh SF (SF9) and
share the channel.

## Consequences

- Capacity of a trunk is the same as any other link; software must save airtime
  instead of adding bandwidth ([decision 0011](0011-airtime-round.md)).
- The host's `--trunk` option (see [settings reference](../reference/settings.md))
  uses a LoRa node in modem mode with a directional antenna on the shared setting.

## Evidence

Design decision by the project owner; no measurement, since the point is
compatibility.
