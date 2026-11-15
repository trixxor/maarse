# Må:rse Denmark Simulator

Build and break a national Må:rse network in the browser. Every node runs the
**real `maarse_core::Router`** (the same code as the firmware), compiled to
WebAssembly. Only the radio, power and backbone around the routers are simulated.

> Simplified radio model. It is not a coverage prediction, and a simulator is not
> a field measurement.

![Storm surge preset: flood alert latency heat map](img/storm-alert-heatmap.jpg)
![Following one message and its path request reach](img/follow-message-reach.jpg)

## Build and open

```sh
rustup target add wasm32-unknown-unknown      # once
python tools/build_denmark_sim.py             # -> docs/denmark/maarse_sim_wasm.wasm
python -m http.server -d docs 8000            # browsers do not load .wasm from file://
# open http://localhost:8000/denmark/          (presets: #normal #internet_loss #bornholm #storm #e45 #forest #festival #spam)
node tools/denmark_sim_test.mjs               # headless test (needs the built .wasm)
cargo test -p maarse-sim-wasm                 # the engine's native tests
```

Map tiles come from OpenStreetMap (internet needed). The Denmark coastline is
embedded (`dk-land.js`), so land/sea, islands and node generation work offline.

## Design

### Layers

```
docs/denmark/app.js        map editor, controls, views (Leaflet 1.9.4)
docs/denmark/scenarios.js  Denmark generator + presets  -> scenario JSON
docs/denmark/maarse-sim.js JSON in / JSON out wrapper for the .wasm
maarse-sim-wasm            engine: radio, queues, power, backbone, timeline, trace
  maarse_sim::radio        path loss and SNR limits, shared with the maarse-sim CLI
  maarse_core::Router      unchanged, one per node
  maarse_crypto::bulletin  real signed bulletins for area alerts
```

### Why a new engine module instead of moving the CLI engine

The CLI engine (`maarse-sim/src/main.rs`) is built around a dense N×N gain
matrix and scenario flags (forest, hill, highway, bridge variants) woven through
it. Moving it unchanged into a library would not give a national map (N² links
for 2,000 nodes, no coordinates, no per-node height/SF/battery), and reshaping
it risks changing the CLI's numbers, which must stay identical. So:

* the **radio model** moved to `maarse-sim/src/radio.rs` (library) and both use
  it; the CLI's output is byte-identical for the same seed and flags;
* the Denmark engine (`maarse-sim-wasm/src/engine.rs`) is a new event loop that
  copies the CLI's **rules** one to one: CAD with 100–600 ms backoff and 8 tries,
  `DutyCycle` with a 120 s give-up, the collision rule (an overlapping signal that
  is decodable at SNR − 6 dB and less than 6 dB weaker kills the packet),
  half-duplex, slot jitter, `note_airtime` for load control, and the neighbour
  seeding the CLI does instead of real neighbour announces.

What it adds: sparse links via a spatial grid (only pairs within radio horizon
and interference range are stored), a send queue per node ordered by
`qos::priority_with(packet, #bulletin)` (the firmware has a queue; the CLI does
not model one), per-node SF, antenna height, batteries, power cuts, backbone
links and timeline events.

### No wasm-bindgen

The module exports five C functions (`buf_alloc`, `out_ptr`, `out_len`,
`sim_create`, `sim_call`, `sim_free`) and passes JSON through one buffer.
That needs no generated glue and no `wasm-bindgen-cli` whose version must match
the crate, works the same in Node and the browser, and costs little because
calls are coarse (one `step` per frame). `getrandom` gets a failing custom
backend: the engine always seeds its RNG from the scenario.

### Radio model (crude, honest)

* Path loss: `maarse_sim::radio` (log-distance, n = 3, 868 MHz), thermal noise
  + 6 dB, SNR limit per SF. Same numbers as the CLI.
* Antenna gain: 6 dB for fixed stations (repeater, anchor, host, Wi-Fi node),
  like the CLI's `--repeater-gain`.
* Height gain: +10 dB per decade above 1.5 m (handheld 1.5 m, relay 2 m,
  repeater 15 m, host 10 m, Wi-Fi node 6 m, mast/anchor 40 m; editable).
* Radio horizon: no link beyond `4.12 km × (√h1 + √h2)`.
* Nodes on different SFs do not hear each other.
* Clutter (forest): a scenario can have `clutter` polygons with a loss in
  dB/km; a link loses that much for every km of its straight line inside the
  polygon (sampled every 250 m). The forest preset uses it for Rold Skov.
* Radio loss in an area (`degrade` event: storm, rain, jamming): every link
  with an end inside loses `db` dB until `degrade_clear`.
* No terrain, no buildings, no land/sea difference in loss, no fading.

### Power

Mains nodes (repeater, anchor, host, Wi-Fi) fall back to their battery in a
power cut; nodes without a battery die. Battery nodes drain their idle power
(handheld 0.05 W, repeater 0.4 W, host 6 W) and the router's `energy` follows
the charge (Full > 60 %, Ok > 30 %, Low > 10 %, else Critical), so energy-aware
flood delays in the router apply. A host in a power cut loses its internet.

### Backbone

Like maarse-host: a host sends every packet it transmits to all its working
links too (TCP 40 ms, Wi-Fi point-to-point 5 ms), and the peer's router receives
it with SNR 10 dB. Floods with a region go only to peers that serve the region.
A TCP link works only while both ends have power and internet (global, per
region, per node); a Wi-Fi link needs only both ends alive.

Because every host sends path requests to every TCP peer and then floods them
into its own area, one direct message to a new destination wakes up the whole
country: Skagen to Copenhagen in the generated net is about 90 path request
transmissions in all five regions and 60+ backbone copies. That is how
maarse-host behaves today with permanently connected peers.

`dht_backbone` (experimental, off) models the alternative the project README
describes: a host does not send path requests to its TCP peers, but looks up
the destination's gateway (the nearest host with internet in the destination's
radio island) and sends it there directly (a dynamic link, like `dht_connect`);
routed packets then use that link too. Measured on the national preset (15 min,
seed 1): 27 % less airtime and 35 % fewer lost receptions, but 96.5 % instead
of 98.6 % delivered. It is a model of the idea, not of maarse-host's code.

### Home routing (a model, not a protocol yet)

Repeaters, masts, hosts and backbone links do not move; only the last hop
(a person's handheld to the nearest fixed node) changes. `home_routing`
models a routing that uses this, built from parts the router already has:

* **Fixed nodes**: repeaters, masts, hosts and Wi-Fi nodes that do not move.
  Links between them with an SNR 5 dB over the limit are infrastructure; the
  fixed nodes know the shortest routes between them (as a proactive routing
  among fixed nodes would; it is static, so updates would be rare).
* **Home**: every node's home is the fixed node it hears best (recomputed
  every 60 s, so moving nodes have a stale home for up to a minute).
* **Lookup**: a sender without a route asks where the destination is. Its own
  home answers if the destination is within the announce scope (8 hops:
  every fixed node hears those announces anyway, so this registry costs no
  airtime); otherwise the gateway of its island does (the DHT knows every
  node's gateway). The question and the answer are real datagrams over the
  radio, asked again every 4 s, at most 3 times.
* **Route**: the answer is a source path along the infrastructure: all the
  way by radio, or up to the sender's gateway, over the backbone, and down
  from the destination's gateway, whichever has fewer radio hops. Longer than
  12 repeaters: hosts are anchors in this model, so the route is split at the
  sender's gateway, which gets its own route for the rest (the router's anchor
  mechanism, unchanged).
* The message then goes through the real routers along that path (per-hop
  acks, local repair, and the router's own fallback to a path request when the
  route fails). Without an answer the sender falls back to a path request; a
  destination known to be unreachable (another island without a working
  gateway) is not flooded at all.

Not modelled: the airtime of keeping the infrastructure routes and the
registry up to date (announces are not simulated in any mode), and a registry
on fixed nodes in places without a gateway (the forest: lookups fall back).

Results (`node tools/sim-experiments/home_routing.mjs 600 1,2,3`: national
preset, 10 min, mean of seeds 1-3; random destinations, so every message
needs a new route, the worst case for floods):

| msgs/min | | today | DHT backbone | home | home + DHT |
|---|---|---|---|---|---|
| 6 | delivered / acked | 97.9 / 97.9 % | 94.2 / 94.2 % | 99.5 / 98.9 % | 99.5 / 98.9 % |
| | airtime | 3,797 s | 2,235 s | 439 s | 439 s |
| | latency p90 | 8.2 s | 20.1 s | 4.1 s | 4.1 s |
| 30 | delivered / acked | 82.8 / 67.3 % | 94.0 / 90.1 % | 92.8 / 88.7 % | 97.3 / 96.9 % |
| | airtime | 13,948 s | 7,633 s | 4,429 s | 1,480 s |
| | latency p90 | 54.5 s | 47.9 s | 19.9 s | 8.1 s |
| 90 | delivered / acked | 54.1 / 34.9 % | 81.9 / 69.3 % | 64.8 / 50.1 % | 77.3 / 61.4 % |
| | airtime | 17,199 s | 10,898 s | 13,387 s | 10,442 s |

Single messages (`tools/sim-experiments/home_edge_cases.mjs`, seed 1, quiet
network): Skagen to Copenhagen 18 transmissions and 5.3 s instead of 149 and
13.6 s; Skagen to Rønne while Rønne's host has no internet: 6 transmissions
(the lookup) instead of 254. The E45 car (a moving destination): one seed
worse (90 % instead of 99 % delivered, twice the airtime), one better (100 %
instead of 92 %), one the same. The forest has no gateway, so it behaves as
today.

What the numbers say: at ordinary load, routing over the fixed infrastructure
removes almost all floods. Under overload (90/min) lookups are lost in the
remaining floods and fall back, and traffic concentrates on the infrastructure
links (more losses at the next hop), so the DHT backbone alone does best there.

### Scenario JSON

```jsonc
{
  "name": "…", "seed": 1, "sf": 9, "duty_permille": 100, "tx_dbm": 20, "duration_s": 1800,
  "features": { "carry": false, "local_repair": true, "backbone": true, "regions": false,
                "sf_slot": false, "scale_timeouts": false, "load_control": false, "area_scope": false,
                "dht_backbone": false, "home_routing": false },
  "nodes": [ { "kind": "repeater", "lat": 56.1, "lon": 9.5, "h": 0, "sf": 0, "battery_wh": -1,
               "mains": null, "carry": 0, "internet": null, "region": "midtjylland",
               "route": [], "speed_kmh": 0, "wander_m": 0, "label": "" } ],
  "links": [ { "a": 0, "b": 1, "kind": "tcp", "up": true } ],
  "traffic": { "direct_per_min": 6, "channel_per_min": 1, "local_share": 0.8, "local_km": 25,
               "start_s": 5, "stop_s": 0, "spammers": [], "spam_per_min": 0 },
  "events": [ { "t_s": 120, "type": "internet_down", "region": "" },
              { "t_s": 150, "type": "alert", "from": 3, "alert": "flood",
                "area": { "polygon": [[56.0, 8.1], [56.0, 8.6], [55.5, 8.6]] }, "text": "…" } ],
  "clutter": [ { "label": "Rold Skov", "polygon": [[56.8, 9.8], [56.85, 9.9], [56.75, 9.95]], "db_per_km": 10 } ]
}
```

`kind`: handheld, relay, repeater, anchor, host, wifi, mobile. Zero / null /
negative values mean "default for the kind". `wander_m` > 0 makes a node walk
to random points within that radius of its start at `speed_kmh`. Event types:
`internet_down`, `internet_up` (optional `region`), `power_cut` and
`power_restore` (optional `area`; none = everywhere), `kill_random` (`n`,
`node_kind`), `kill` (`node` or `nodes`), `kill_area` (`area`, `node_kind`),
`revive` (optional `nodes`; none = all killed), `host_internet` (`node`, `up`),
`degrade` (`area`, `db`), `degrade_clear`, `link` (`index`, `up`), `alert`,
`announce` (`node`), `set_traffic` (`traffic`).

### Event trace

The engine writes the CLI's `--trace-json` events (`tx`, `rx`, `defer`, `send`,
`deliver`, `ack`) and extends them: every packet carries its packet id (`pid`)
and, through *causal tagging*, the message (`msg`) or alert (`alert`) that
caused it (a packet sent while a router handles a tagged packet inherits the
tag), plus `cancel` (flood suppression), `drop` (duty, busy, quota, area),
`route`, `bb` (backbone), `repair`, `carry`, `retry`, `alert_rx`, `announce`,
`event`, `dead`, `revive`, `dht`. Follow, reach, animation, the live feed and
the log are all views of this one stream. It keeps the last 400,000 lines
(older ones are dropped).

Fields for transparency: a `tx` has `nx` (the intended next hop of a routed
packet, -1 for floods), `dst` (destination node, if it is a node), `path`
(source path as node indices) or, for a path request, `trail` (the repeaters
it passed). An `rx` has `snr` (one value per decoder in `ok`) and `cw` (one
cause per entry in `coll`: the strongest interferer, or -1 for half-duplex).
A path response sent from a router tick is tied to its message through the
request's reply token.

### Collisions: what the number means

`collisions` counts every reception that was lost, at every node: one
transmission heard by 40 nodes can count 40 times. The summary splits it:
`coll_flood` (copies of floods: other copies usually get through),
`coll_next_hop` (a routed packet lost at the node that should forward it: these
cost a retransmission) and the rest (a routed packet lost at a bystander: no
harm); `coll_half_duplex` is rare because of listen-before-talk. On the
national preset (15 min, seed 1): 69,031 lost receptions, of which 66,308
flood copies and 105 at the next hop.

Flood reach (`reach`): for one packet id, every node that decoded it, its hop
count, and why it did not forward: forwarded / suppressed (neighbours covered
the area) / role does not forward / outside region / hop limit / dropped (duty,
busy, quota, area scope) / not forwarded (duplicate or not needed).

## Ask Claude

The **Ask Claude** tab lets you ask a question ("what happens to Bornholm if
the internet goes down?"); Claude plans a test and runs it here through five
page tools (`docs/denmark/ask.js`).

**Locally (preferred).** Start the bridge instead of `python -m http.server`:

```
node tools/sim-bridge/bridge.mjs      # or scripts/start-sim.bat / start-sim.sh
# open http://127.0.0.1:8765/denmark/
```

The bridge (`tools/sim-bridge/bridge.mjs`, no dependencies) serves `docs/`
and, for each question, runs Claude Code headless (`claude -p`) with the
Claude Code login on this machine. Claude gets no file or shell tools, only
the simulator's tools through a small MCP server (`sim-mcp.mjs`) that relays
each call to the open simulator page; the page runs it in the WebAssembly
engine and shows it live. Follow-up questions continue the same conversation
(`--resume`). The bridge listens on 127.0.0.1 only; API calls need a per-run
token that only a page served by the bridge can read, and other origins are
refused. Each question spends your own Claude usage.

**As a claude.ai artifact.** The same tab works through the `sample`
capability when the page is published as an artifact (see below).

The tools:

* `describe_setup`: the scenario on the map, features, events, live results,
  presets and the largest towns with coordinates;
* `run_experiment`: up to 4 variants x 3 seeds, headless in the engine, mean
  results per variant (delivered, acked, latency, transmissions, path
  requests, airtime, drops, losses at the next hop, flood copies, alerts);
* `show_live`: load a variant on the live map and start it;
* `follow_message`: send one message (towns by name), run until it is acked,
  and return its story (the Journey tab's steps and diagnosis);
* `add_event`: a timeline event now, optionally undone after N minutes.

Claude sees only what the page sends: a short description of the protocol and
the simulator, the conversation, and the tool results. Each question spends
the viewer's own Claude usage. Opened locally (not as an artifact) the tab
stays hidden. Build and publish:

```
python tools/build_denmark_sim.py         # the engine, if it changed
python tools/build_denmark_artifact.py    # docs/denmark/artifact.html
```

Then publish `artifact.html` as an artifact with the files the script lists
and `capabilities: {sample: {}, downloads: true}`. Inside an artifact the map
tiles do not load (the page may not fetch images from other hosts); the
embedded coastline carries the map, and exports go through the `downloads`
capability.

## What the UI does

* **Live view** (on the map): a dark map (OSM tiles under a CSS filter; the
  half-moon button switches), a status bar (time, transmissions per minute,
  nodes up, delivered and acked, lost at next hop, flood copies collided,
  internet), a live packet feed (type, sender, hop, next hop received or not,
  or how many decoded it; "routed only" hides flood copies; "this message"
  follows one), a detail card per packet (path, priority, queue wait, airtime,
  every decoder with SNR, every collision with its cause drawn on the map), a
  timeline with a histogram of transmissions per 10 s by type and red marks for
  timeline events, and a legend whose packet types switch the feed and the
  animation.
* **Journey**: type two towns (the nearest handheld there) or pick nodes, send,
  and read the message's story: the path request (how many transmissions, which
  regions, backbone copies, how many nodes heard it, the longest trail), the
  delivery, the route that won with its TCP legs, the path response and the
  ack, trouble on the way (collisions at the next hop with the interferer,
  retries, repairs, drops, carry), and, if it does not arrive, a diagnosis:
  destination down, another island without a backbone (and which host lacks
  internet), or the path request never reaching it (closest node, 12-repeater
  trail limit). Replay animates it on the map; nodes are coloured by their part
  in it.
* **Build**: place and drag handhelds, relays, repeaters, anchors, hosts,
  ESP32 Wi-Fi nodes and cars (draw a route), delete, draw TCP and Wi-Fi links
  (click a link to switch it). The inspector edits kind, SF, height, battery,
  carry copies, internet and region. Feature switches, SF, duty cycle, traffic.
  Export/import the scenario JSON.
* **Generate Denmark**: sliders for users, repeater spacing, % hosts with
  internet and seed. Repeaters on a jittered grid on land (plus one per small
  island town), masts in towns over 50,000, a host per town over 12,000 (TCP star
  to the nearest of the four big cities, which are meshed), users weighted by
  population^0.8 around ~100 towns (`dk-towns.js`, rounded figures). Nodes are
  coloured by radio component, so isolated islands show at once.
* **Disaster**: kill or revive one node and cut one host's internet by clicking,
  destroy all nodes (or one kind) in a drawn area, radio loss in an area, power
  cut in an area or everywhere, internet down everywhere or per region, kill N
  random repeaters, toggle a backbone link. Each action applies now or at a
  chosen time, optionally "for N min" (then undone), goes on the timeline and
  shows on the map (dashed when planned, filled while active).
* **Terrain**: draw forest areas with a loss in dB/km, place a group of walkers
  (relays or handhelds, radius, speed), and rebuild the forest scenario with
  your numbers (walkers, role, forest size and loss, repeater distance, hill
  relay and its height, walking speed).
* **Alerts**: pick a sender and type, draw a circle or polygon, send. Per alert:
  coverage of live nodes inside, latency median/p90, nodes outside that also got
  it, and a latency heat map.
* **Stats**: delivered %, acked %, latency, channel reach, airtime, collisions,
  duty-cycle hits, drops, local repairs, carry, backbone packets, queue delay per
  priority, transmissions per type. **Compare**: run the scenario twice with one
  feature flipped and see both results side by side.
* **Journey tab, lower part**: the generator's messages (click to follow) and
  flood reach for a path request or announce.
* **Log**: filter by node, packet type, message and event; export JSON or CSV.
* **Inspector**: role, state, battery and energy, internet, slot, airtime,
  duty budget used, router stats, router neighbour table (SNR, last heard, role),
  route table, dead next hops (local repair), send queue with priorities,
  backbone links, DHT gateway; "send from / to here" and "announce: show reach".

Presets: national normal day, total internet loss, Bornholm cut off, storm surge
on the west coast with power cut, E45 drive (repeaters every 15 km, carry on),
group in the forest (Rold Skov, walkers with relays, relay on a hill), Roskilde
festival, spam attack.

## Simplified or missing

* Radio: see above. No terrain profile, so "the hill" is just a tall antenna;
  forest loss is a straight-line clutter model.
* Announces are sent on demand (button / `announce` event) with a real stamp
  and real size, but an unsigned body; the router does not verify signatures.
  Neighbour roles are seeded every 60 s, as in the CLI.
* No encryption of chat payloads (the router does not need it); bulletins are
  really signed and verified.
* Cars move along polylines (back and forth), not on roads; walkers wander to
  random points in a circle.
* Energy: idle draw only; transmit energy is not subtracted.
* Mailboxes (maarse-host) are not simulated; the DHT only as the experimental
  `dht_backbone` model above.
* Changing the scenario restarts the run at t = 0; live disaster actions are
  injected without a restart and recorded on the timeline.
* The map tiles need internet (the coastline and nodes do not).
