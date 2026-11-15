# Proposal: area alerts on top of signed bulletins

Status: proposal, implemented only in the Denmark simulator (`maarse-sim-wasm`).
Nothing in `maarse-core` or the protocol changes yet.

## Goal

An authority sends an Evacuation / Fire / Flood / Water / Info alert to an
**area** (circle or polygon). Phones inside the area raise it loudly; phones
outside may show it quietly or not at all. Ideally the flood does not spend
airtime far away from the area.

## Part 1: the area inside the signed text (no protocol change)

A bulletin (`maarse_crypto::bulletin`) is a channel message on `#bulletin`
with a signed text. The area can go **inside the signed text**, so it is
authenticated together with the message and needs no new packet field:

```text
text = 'A' | version/id (u16) | kind (u8) | area | utf-8 message
kind: 0 info, 1 evacuation, 2 fire, 3 flood, 4 water
area: 'C' lat (i32, 1e-5 deg) lon (i32) radius (u32, m)
    | 'P' n (u8, 3..12) n × (lat i32, lon i32)
```

A circle costs 13 bytes, a 6-corner polygon 50. With the 69-byte bulletin
overhead and a 60-byte message it still fits a channel packet. The simulator
uses exactly this layout (the u16 is the alert index there).

The receiving app verifies the signature against trusted authorities, then
compares its own position (GPS, or a configured home position) with the area.
This is an app-only change: firmware and routers are untouched.

Priority already works: bulletins go out with `PRIO_DIRECT` through
`qos::priority_with(packet, bulletin::channel())`.

## Part 2: limiting the flood (protocol addition)

Today a bulletin floods the whole region (or the whole network). Two options:

1. **Regions (exists today).** Send the bulletin with the region that contains
   the area (`send_channel_in(channel, region, …)`); repeaters that do not serve
   the region do not forward it. Coarse (five Danish regions) but available now.
   The simulator does this when "Regions" is on: one shared region → that
   region, otherwise `dk`.

2. **A geographic scope field (new).** Add an optional TLV to channel packets
   carrying a bounding circle (centre + radius, 10 bytes). A repeater that knows
   its own position forwards the flood only if it is within the circle plus a
   margin (the simulator uses 15 km). Repeaters without a position forward as
   today. The field must be outside the encrypted payload so repeaters can read
   it, and it should be covered by the bulletin signature (the signed text holds
   the exact area; the scope field holds its bounding circle, and receivers
   check that the two agree).

The simulator's switch "Area scope for alerts (sim layer)" models option 2:
forwards of a tagged alert by nodes more than 15 km outside the area are
dropped before they reach the send queue (logged as `drop` with `why: "area"`).
Compare a run with it on and off to see the airtime saved and whether coverage
inside the area suffers (edge nodes are often reached through repeaters just
outside the area, hence the margin).

## Open questions

* Position source for repeaters (configured at install, like the region list).
* Margin: fixed, or derived from the SF / typical link length?
* Should a receiver outside the area show the alert at all (quietly)?
* Revocation / "all clear" messages: same format with kind = info and a
  reference to the earlier alert id.
