# Denmark simulator v3: plan

Status: proposal, 2026-11-18. Nothing built yet. Planning and mockup done;
waiting for the user's go. The sections further down are the history of how
the plan grew; **section 0 is the current state.**

## 0. Where we are (updated 2026-11-18)

**Mockup**: https://claude.ai/artifact/3zYfi5pfNCtcdTM8gMbcqb (9 boards,
private). Source copy in `docs/mockups/denmark-sim-v3/` (`.dc.html` boards +
`canvas.json`).

| # | Board | Shows |
|---|---|---|
| 1 | Follow a message | Skagen → Rønne, cut/restore trunk, status, journey, timeline (clickable) |
| 2 | Map editor | hover hints, click cards, right-click menu, area actions, inspector |
| 3 | Generate | users per region, connection mix, infrastructure, warnings, presets |
| 4 | How to | step-by-step follow, every status explained |
| 5 | Phones | two phones: chat, voice, picture, call refused, Pages, mosh (clickable) |
| 6 | Control room | phones live while you cut internet/trunk/power/storm; live log, numbers; dark/light |
| 7 | Results | KPIs, delivery by type, latency chart, post offices, loss reasons, busiest nodes, packet log, compare runs; dark/light |
| 8 | Every feature | all Må:rse features grouped, how each is simulated (real / shared rules / model), new vs today |
| 9 | Devices and people | every device kind, mobile/stationary, person route A → waypoints → B, speed, device, schedule |

**Decisions by the user**

- Runs on GitHub Pages (static, wasm + JS). Max 2,000 nodes is enough.
- Ask Claude is removed.
- Everything Må:rse can do must be simulated: messages, files, voice,
  pictures, Pages, alerts, calls, IP tunnel (SSH/mosh), linked devices, all of it.
- Internet-only users are nodes of their own (app, no radio).
- Generator by the five regions; presets are kept and made smarter.
- Two or more simulated phones running the app, usable while the net is broken.
- Every device kind can be placed (anchors, backbone servers, …); every node is
  marked mobile or stationary; people walk A → B with waypoints, speed and a device.
- Light and dark mode on every screen.
- **Reuse as much existing code as possible** (firmware, core, crypto, host,
  clients) so the simulator is as close to the real experience as we can get.
  Order of preference: (1) run the real code unchanged (maarse-core Router,
  maarse-crypto, maarse-voice Codec2, the web client UI); (2) move pure rules
  out of maarse-host into a shared crate both use; (3) a model only where the
  real thing needs hardware or the OS (radio waves, TUN adapter, serial ports),
  marked as a model in the UI. Options to look at: building maarse-host's node
  logic (`node.rs` and `node/*`) for wasm behind a link/clock/storage trait, so
  hosts in the sim are the real host; and showing a selected node's Heltec
  display pages with the firmware's own display code/art.
- Many logs and statistics to watch (board 7 is the floor, not the ceiling).

**More decisions (2026-11-18, round 3)**

- Yes to simulating: replica post offices, backbone self-healing, phone ↔ radio
  link drops, phone as host on Wi-Fi vs mobile data, device restore, attacks
  (spam/guard, forged/replayed alerts), day/night, range-test calibration,
  duty-cycle sub-bands.
- **One world, everything at once.** A run is one simulated world. A "normal
  day" of traffic runs in the background (24 h curve: night quiet, morning and
  evening peaks), and everything you start on top of it (follow a message from
  Aalborg to Næstved, the phones, an alert, a file, a cut) is injected into
  the same world: same airtime, queues, collisions, losses, post offices. No
  separate simulations. Planned extra: "branch here" = replay the same seed to
  time T in a second engine and run a what-if from there, then compare.
- Do not start building until the user says go.

**More decisions (2026-11-18, round 4)**

- **Rule sets with a standard.** Every rule and setting (radio: frequency, BW,
  SF, CR, TX power, duty cycle; router timers; post office, file, Pages, call,
  tunnel, DHT and guard rules from section "Host rules to share") is taken from
  today's firmware and host and saved as the profile **"Standard (as built)"**.
  The user can copy it, change anything, save it under a name, and pick a
  profile per run. Values come from the code's own constants (no hand-copied
  numbers), so the standard follows the code.
- **Change settings while it runs**: e.g. switch all LoRa to another SF and
  frequency mid-run to test range and speed; the change is an event on the
  timeline. Needs the radio model to take the frequency as a parameter (today
  `maarse_sim::radio` is fixed at 868 MHz, and the forest loss formula uses f
  too). Nodes on different frequency/SF do not hear each other (already so for SF).
- **Save a simulation locally in the browser** (IndexedDB), plus export/import
  as a file. No server.
- **Snapshots and branches**: save snapshots during a run; start a branch
  from any snapshot and compare. First version: a snapshot is scenario + seed +
  event list + time, and opening it replays deterministically to that time
  (exact, costs CPU). Later, if needed: a real memory snapshot of the engine.
- **CPU/browser limit**: a setting (max nodes, max events per frame, max
  speed) with a benchmark mode that measures what this machine can do; the
  limits are set from the benchmark later.

**Host rules to share (answer to "which rules, which host")**

The host is the `maarse-host` crate: the PC host, maarse-server (VPS) and the
Android app (via JNI) all run it. Rules in it that the sim needs:

| Area | Where | Rules |
|---|---|---|
| Post office | `node.rs` ~104-170, 2719-2805, `maarse-core/src/mail.rs` | deposit after 45 s without ack; 100 per recipient; 7 days; replicas by rendezvous hashing; fetch from replicas every 10 min; retry doubling |
| Files | `filebox.rs` | route kind TCP/LoRa/unknown; 8 KB LoRa, 1 MB TCP; file mailbox 7 days; one chunk in flight over LoRa; 600 s timeout |
| Pages | `node/pages.rs`, `node/pagelock.rs` | lookup, deflate, picture copies (grey ~1.4 KB, colour ≤ 4 KB, Bayer dithering), burst window 30 s, locks, boards |
| Calls | `node/pages.rs` ~576 | refused unless TCP all the way |
| IP tunnel | `tunnel.rs` | header compression, fragments, NACK after 2 s, give up 12 s, rekey 5 min |
| Backbone | `dht.rs`, `node.rs` | DHT upkeep 5 s, lookup ≤ 1/30 s, full replication in small backbones, bordercast repeat 120 s, key cache 30 s, min-peers |
| Guard | `guard.rs` | 8 conns/IP, 100 packets/s, 64 KB/s, ban after 500 drops or 20 invalid |
| People | `node/person.rs`, `node/seq.rs`, `history.rs` | linked devices, mail from replicas, sequence gaps, shared history |

Correction after a closer look: the engine already takes the deposit time
(`maarse_core::mail::DEPOSIT_AFTER_MS`) and the replica choice
(`maarse_core::mail::replicas`) from core; what it re-implements is the post
office *behaviour* around them (sync, fetch, retry), and none of the files,
Pages, call, tunnel, DHT or guard rules exist in it yet. Proposal: (A) move the pure parts
(filebox, tunnel, guard, pages, mail policy) into a small I/O-free crate used
by both; (B) then check whether `node.rs` can run on Link/Clock/Storage traits
so the wasm runs the real host loop. B gives the most realism and is bigger.

**Proposals from me, not yet accepted or rejected**

- Move the host's pure rules (file limits, file mailbox, tunnel, call refusal,
  Pages) into a shared library used by maarse-host and the wasm engine.
- Run the real web client in the phone frames over a simulated companion
  connection (keeps app/web parity for free).
- Presets with expected results, run in CI.
- Also simulate (found in the code): replica post offices (rendezvous hashing),
  backbone self-healing (`--min-peers`, DHT, seed DNS, reconnect), phone ↔
  radio link drops, phone as its own host on Wi-Fi vs mobile data, device restore
  from recovery phrase, spam/guard bans, forged/replayed alerts, day/night
  behaviour, path-loss calibration from real range tests, duty-cycle sub-bands.

**Steps and their status**

| # | Step | Status | What is missing |
|---|---|---|---|
| 0 | Remove Ask Claude (`ask.js`, `tools/sim-bridge`, tab) | **done 2026-11-18** (also artifact build, README section, start scripts now plain http.server) | — |
| 1 | New layout: modes, clickable map, follow panel, timeline, light/dark | **done 2026-11-19**: 4 modes, theme remembered, right-click menus, hover hints, follow panel on the map (plain status: queued/on air/backbone/post office/carried/held/delivered/no progress), coloured timeline markers + followed message dots | Control room mode moved to step 6 (needs the phones) |
| 2 | Device palette, mobile/stationary, people with routes | **done 2026-11-19**: kinds server (no radio) and app (no radio, TCP to host), radio on/off per node, routes with start time, pause per waypoint, end mode (back/stay/repeat), mobile/stationary flag; Build tools + inspector; engine tests | MeshCore gateway kind; group of people at once |
| 3 | Generator per region + connection mix + app users | **done 2026-11-19**: users per region with LoRa/both/app/cars/relays per cent; both + app get a TCP link to the nearest host with internet; summary of what was made and users cut off | warning list per island with "add a trunk" button |
| 4 | Shared host rules library + transfer model | **4A done 2026-11-19**: crate maarse-hostrules (filebox, calls, tunnel, guard, pages, pagelock, Rules 'Standard (as built)'), maarse-host uses it (107 + 38 tests green). **4B in progress** (agent): transfer model for files/voice/pictures/Pages/calls/tunnel in the engine + phones | 4B |
| 5 | Live item state on the map | **done 2026-11-19**: plain status in the follow panel + pulsing dot where the message is now, coloured by state | per-copy view for floods |
| 6 | Phone simulators on the real web client | **done 2026-11-19 (first version)**: Phones mode, two phones = the real maarse-client in iframes over a fake WebSocket to simulated nodes (phone.rs: GetInfo, names, announce, channels, SendDirect, SendChannel, stats, neighbours; events Sent/Acked/DirectReceived/RouteLearned/Deposited/Heard); keeps running while you break the net | files/voice/pictures, calls, Pages, trace route, remote admin, alerts from phones, linked devices, identity for safety numbers, 'Open as a phone' menu, Control room layout |
| 7 | Calls, IP tunnel (SSH/mosh/ping/bulk), channels, alerts v2, linked devices, remaining features of board 8 | listed (board 8) | all code |
| 8 | Failover and extras | **started 2026-11-19**: normal day traffic curve (day_start_h); backbone self-healing (min_peers per node, server default 6: reconnects to random hosts every minute, engine test) | replica post office failover preset (office_failure: passes), phone ↔ radio link drops, device restore, forged/replayed alerts, guard bans, range-test calibration, duty sub-bands |
| 9 | Results page | **first version 2026-11-19**: KPI tiles (existing), delivery time over the run (median + p95, your events marked), busiest nodes, 'why not delivered' analysis by current status, collisions, reboots, queue delay, compare runs, packet log + export | delivery by traffic type (needs step 4/7 types), post office table, dark-mode check of charts |
| 10 | Presets with expected results | **started 2026-11-19**: great_belt, bornholm_alone (1 known gap), north_power; checked by tools/denmark_sim_test.mjs (known gaps reported, not counted) | more presets (SSH across the country, storm + alerts v2, festival), run the test in CI (no CI workflow for it yet) |
| 11 | Share link, README, feature inventory | **done 2026-11-19**: share link; README 'Using it (v3)'; docs/maybe/feature-inventory.md with sim status per feature | keep the inventory up to date |
| 12 | Profiles + live settings | **in progress**: radio model takes frequency + bandwidth; Break > Radio settings switches SF/frequency/BW/power for all nodes mid-run | UI for the radio event; profiles 'Standard (as built)' from the shared rules crate (agent working on step 4A) |
| 13 | Save locally, snapshots, branches | **done 2026-11-19**: Build > Save: save run / snapshot (IndexedDB), open, branch from a snapshot (deterministic replay to its time, verified identical), delete | real memory snapshots if replay gets slow |
| 14 | CPU limits + benchmark | **done 2026-11-19**: Build > Performance: max events per frame, max speed, node warning (saved per browser); benchmark runs the scenario flat out and sets the limits | — |
| 15 | Tracking a node/person | **done 2026-11-19**: right-click > Track; track line coloured by best contact (green repeater/anchor/host, yellow only handhelds/relays, red none); watch list with contact now, lost-for time, role (forest mode), battery, messages got/sent, contact changes | timeline of received messages on the track line |

**Findings from the simulator (2026-11-19)**

- Presets ran without post offices (`mail: ""`) although hosts and firmware
  have them: fixed, the standard scenario now uses `mail: "post"`.
- Known gap: Bornholm cut off (trunk + internet) → a message from Copenhagen
  is stored at the Copenhagen office and 2 replicas, but is NOT delivered
  within 45 min after the links come back (presets have periodic announces
  off; office retries / presence fetch do not reach Roenne). Checked: still
  not delivered after 6.4 h. maarse-host does the same as the sim: with the
  backbone up an office does not retry (node.rs ~3028, "presence and syncs
  deliver more cheaply"), so delivery waits for the recipient's announce
  (firmware default every 12 h) or MailSync, and in the sim the recipient
  never syncs. mail_refetch and hourly announces did not help.
  **Root cause found (2026-11-19):** the recipient does sync (650 syncs) to
  its home office, Roenne host, which then asks the replicas in Copenhagen
  for the mail (OP_FETCH). Roenne host has no internet, only the LoRa trunk,
  and host-to-host mail ops go only over TCP (sim `send_deposit`, host:
  replicas via DHT contacts), so the fetch waits forever ("mail_deferred").
  A host reachable only over a trunk can never get mail held elsewhere.
  **Proposal (needs the user's yes, touches maarse-host):** when two hosts
  have no TCP between them but a route over radio/trunk exists, send the
  host-to-host Deposit ops (STORE/FETCH/DELIVERED) as routed Deposit
  packets over the radio, as consolidation already does
  (`send_deposit_lora`), and do the same in the sim.
- The Great Belt does not cut the radio: repeaters on Funen and Zealand hear
  each other across 18 km, so messages cross with every link down.

**Small findings while reading the code (2026-11-18)**

- `maarse-host/src/node.rs:105` has its own `DEPOSIT_AFTER = 45 s` next to
  `maarse_core::mail::DEPOSIT_AFTER_MS = 45_000`: same value today, two places
  to change. Use the core constant.
- `main` is 1 commit ahead of origin (31184b1, the `/denmark` Pages publish) —
  not pushed.
- A branch `worktree-agent-ad2417210fc1303de` still exists (memory said only
  main + backup-before-rewrite were kept).
- Open from earlier: no field measurements yet (the sim's radio model is not
  calibrated against reality); pull transfer Heltec ↔ XIAO failed once (weak
  direction suspected, not proven); the receiver's list of finished files is
  in RAM only; host as a service not done.

## 1. Goal

A full simulation of the Må:rse system, from sender to receiver, on a generated
Denmark network:

1. **Network generator**: "N devices in region X, this mix of connectivity", and
   the net is placed and wired up for us.
2. **Mixed connectivity**: some users LoRa only, some LoRa + internet, some
   internet only (app via a host), hosts with and without backbone.
3. **Follow one item live**: send a message, cut a link or stop a repeater
   while it travels, see where it waits (a post office, a carrier, a queue),
   restore the net and watch it get delivered.
4. **Every traffic type**: text, emergency/area alert, voice message, picture,
   file, Pages, "phone call", VPN/tunnel, linked devices.
5. **Feature inventory**: every function we meet goes on a list for the coming
   full system description (section 8).

**Hard constraint: it must run on our GitHub Pages site** (trixxor.github.io/maarse/denmark),
static files only. That means: no server, no maarse-host process; everything runs
in the browser as wasm + JS; generated nets and runs are saved as JSON in the
URL/local download, not on a server; the release workflow keeps publishing it
(already done for `/denmark` in 31184b1). Anything that lives in maarse-host
(Pages, filebox, tunnel) is either compiled into the wasm or modelled in the
engine, never called live. Node count must stay within what a browser handles.

## 2. What exists today (short)

- `maarse-sim-wasm`: event engine running the real `maarse_core::Router` per
  node; radio, power, backbone, clutter, regions, timeline events, `follow`,
  `reach`, `inject`, post offices/mail (~50 references), area alerts with real
  signed bulletins.
- `docs/denmark/scenarios.js`: `generateDenmark()` places nodes from towns and
  the coastline, plus presets (#normal #internet_loss #bornholm #storm #e45
  #forest #festival #spam).
- `docs/denmark/app.js`: Leaflet map editor, controls, views, "follow message".
- Node kinds: handheld, relay, repeater, anchor, host, wifi, mobile.

**Gaps against the goal**

| Want | Today |
|---|---|
| Generator by count per region and connectivity mix | Generator by fixed density; no "internet only" users |
| Live follow while I break things | Follow is mostly after the fact (trace replay) |
| Visible "where is my message now" (post office, carrier, queue) | Partly in trace, not as a state on the map |
| Voice, pictures, files, Pages, calls, VPN | Only text messages and alerts as traffic |
| App users on the internet (phone → host) | Not modelled |
| Linked devices | Not modelled |

## 3. Network generator

New panel "Generate", replacing the current density settings (old presets keep
working: they become saved generator settings + fixed edits).

**Input per region** (Hovedstaden, Sjælland, Syddanmark, Midtjylland,
Nordjylland, or a box drawn on the map):

- number of users;
- connectivity mix in %: LoRa only / LoRa + internet / internet only (app);
- share of mobile users (cars) and of relays (in-house relay boxes);
- infrastructure: repeaters per 100 km² or "auto" (enough to cover towns),
  anchors (masts), hosts with backbone, post offices, Wi-Fi trunks;
- seed (same seed = same net).

**Placement rules**

- Users weighted by town population (`dk-towns.js`) with a rural share; never at sea.
- Repeaters placed greedily where users have no coverage, high spots first.
- Hosts in towns; each region gets at least one host with TCP backbone.
- Post offices on hosts (the "host keeps, radio carries" rule).
- All nodes on the same radio settings (project rule: one frequency/SF).

**Output**: the same scenario JSON as today, so the engine needs no change for
this part. Live summary before running: nodes per kind, estimated coverage %,
islands without a link (connected components).

Code: `scenarios.js` gets `generateNetwork(spec)`; `app.js` gets the panel.

## 4. Mixed connectivity (internet users)

New user attribute `uplink`: `lora`, `lora+net`, `net`.

- `net`: a phone app with no radio. It talks to its home host over TCP (like
  the Android app/web client to maarse-host). It has no radio links; it is
  reachable only while it and its host have internet.
- `lora+net`: radio as today, plus a TCP link to its host when internet works;
  the router can use either.
- Internet loss events (global, per region, per node) already exist and then
  hit these users too.

Engine: a new `Kind::App` (no radio, TCP link to a host), reusing the backbone
link code.

## 5. Follow a message live

Turn "follow" into a live view during the run:

- **State of the item at every moment**: `queued at node`, `on air`,
  `on backbone`, `waiting at post office P`, `carried by node C`,
  `held (unknown sender)`, `delivered`, `acked`, `given up (why)`.
- Map: the item as a marker moving along hops; waiting places pulse; dropped
  copies shown in grey with the reason.
- Side panel: timeline of the item (hop, time, SNR, why it waited), and
  "where is it now".
- **Breaking things mid-flight**: click a node → stop / start; click a link →
  cut / restore; draw an area → radio loss / power cut / internet loss. These
  become timeline events at the current time, so the run can be replayed with
  the same seed.
- Pause, step 1 s / 10 s / 1 min, fast forward to "next state change of the
  followed item".

Engine: a per-message state machine in `Msg` updated at every event, and a new
call `item_state(id)`; `inject` gets "now" events for cut/stop/restore.

## 6. Traffic types

Each type copies the rules of the real code (section 6a). Where the real thing
lives in maarse-host (not in the wasm build), the engine models its sizes,
limits and rules, and the UI marks it as a model.

| Type | What we simulate (from today's code) |
|---|---|
| Text message | real router; textz; ack; on no ack -> recipient's mailbox at a post office; seq gap shown |
| Emergency / area alert | alerts v2: signed by an issuer key, replay drop, area + region scope, latency heat map, who missed it and why |
| Voice message | Codec2 1200 bit/s (`audio/x-codec2`), ~1.5 KB per 10 s, sent as a file: so it follows the file rules below |
| Picture / file | file offer + pull/batch chunks (`CAP_FILE_PULL`), one chunk in flight over LoRa, burst over TCP; route limit 8 KB over LoRa, 1 MB over TCP; unknown route -> file mailbox on the host (up to 1 MB, 7 days; >8 KB waits for a TCP route); cancel by either side |
| Pages | page request (0x05) to a host; page sent deflated as a file; locks (`@allow`), boards (post 0x0D); pictures fetched only on demand in grey/dots/colour copies, small over LoRa, full over the internet; burst limits `--pages-burst-lora/net` |
| Call | Opus 48 kHz, 20 ms frames as call datagrams: **internet all the way only** (host refuses `NOT_OVER_TCP`). The sim shows when a call is possible and why not; plus the call test service (music, echo, speed) |
| VPN / SSH | IPv6 tunnel (`fd…` address per node), session key in a sealed message, then datagrams with 28 B overhead, header compression, MSS clamp, fragments + NACK. Always over TCP; over LoRa only with `--tun-lora`. Sim: an SSH session (keystrokes + screen updates) and a bulk copy; we measure round trip and throughput per path |
| Linked devices | person = master identity; message to the person's address reaches every device; sibling sync; inbox sync |
| Channels | public and private channels (flood) |
| Position / telemetry | shared on a timer, shown on the map; MQTT bridge not simulated |

For every type the follow view (section 5) answers: delivered directly, waited
in a post office or file mailbox, carried, refused (and the reason, e.g.
"calls need internet all the way"), or lost.

### 6a. What the app and the host can do today (survey 2026-11-18)

Source: `maarse-host/src`, `maarse-core/src/companion.rs` + `content.rs`,
`android/app/.../*.kt`, `maarse-client/index.html`.

**Messaging**: direct messages (textz compression, acks, sequence numbers with
gap notice), public and private channels, store-carry-forward with carried-ack,
post office mail (deposit on no ack, sync, quotas `--mail-quota-mb`), hold
messages from unknown senders, history shared by several screens (7-day log).

**Files and media**: file offer/pull transfer with progress and two-sided
cancel; per-route size limit (`GetFileLimit`: 8 KB LoRa, 1 MB TCP); file
mailbox for offline recipients; voice messages (Codec2, both app and web
client); pictures.

**Calls**: Opus calls, internet only; call test service (music, echo, speed).

**Pages**: `maarse://` pages served from `<data>/pages`, MU markup, locks,
notice boards, picture copies, start page with search, server Pages tab with
statistics and file editor.

**Alerts**: alerts v2 with issuer key and certificate, area + region, alert
view in app and web client.

**IP tunnel**: IPv6 over Må:rse for ping/SSH/mosh, TUN adapter (Wintun/Linux),
ping answered without an adapter.

**Identity and people**: identity, recovery phrase (24 words), safety numbers,
identity replace, linked devices (QR link, device list, sibling sync, backup).

**Network and backbone**: TCP backbone with region hello, DHT (Kademlia,
TCP only), seed DNS, public server relays hosts behind NAT, bordercast, trunks
(LoRa trunk links with Bloom filters), serial link, KISS TNC, UDP links,
Wi-Fi modem with modem key, BLE and USB radios, guard (rate limits, bans,
block list).

**Bridges**: MeshCore gateway, MQTT with Home Assistant discovery.

**Operations**: radio settings, roles, regions, names, remote admin of nodes
(admins list), OTA (USB/BT only), range test, trace route, neighbours, power
report, telemetry dashboard (maarse-server), debug view, service page and log,
settings page with restart, web flasher, tray app.

For every type the follow view (section 5) answers: delivered directly, waited
in a post office, carried, or lost, and why.

Traffic panel: pick sender + receiver (or area for alerts), pick type, send now
or "background load" (N messages per hour per user of a type mix).

## 6b. Design and map editor (mockup 2026-11-18)

Mockup: https://claude.ai/artifact/3zYfi5pfNCtcdTM8gMbcqb (5 boards).

- **Ask Claude is removed** (`ask.js`, `tools/sim-bridge`, the Ask tab).
- 8 tabs become 4 modes: **Build · Send · Break · Results**.
- **Everything on the map is clickable and says so**: hover shows what it is
  and "click to …"; click selects and opens a card with actions; right-click
  gives a menu (send from here, stop, reboot, what it hears, open as a phone,
  delete); a floating tool bar (select, add node, link, draw area, measure);
  a hint bar with the mouse gestures; Ctrl Z undo.
- A drawn area asks what should happen inside it (power cut, internet down,
  storm, destroy, forest/town, alert) and for how long.
- **Follow panel** on the right while a message is followed: status in plain
  words with the reason, a button to undo what blocks it, and the journey.
- **One timeline** at the bottom with your cuts and repairs as markers and
  "skip to next change of #n".
- **Copy link to this run** (scenario + seed + events in the URL).

## 6c. Phone simulators

Two (or more) simulated phones next to the map, each bound to a node: an app
user (internet only) or a handheld over Bluetooth. They show the app's screens
(chats, map, pages, network, settings) and everything done on them goes
through the simulated network and can be followed: text, voice, pictures,
files, calls (refused with the reason when there is no internet path all the
way), Pages, the IP tunnel (SSH/mosh, round trip and throughput shown),
channels, alerts, linked devices.

Proposal for how: run the **real web client** (`maarse-client/index.html`) in
each phone frame and give it a simulated companion-protocol connection to its
node instead of the WebSocket to maarse-host. Then the phones are the real app
UI, they stay level with the app and web client without extra work, and only
the host side (what maarse-host does for the client) has to exist in the
engine. Needs: a fake WebSocket in the page, and the engine answering the
companion commands the client sends (SendDirect, FileBegin…, RequestPage,
SendCall, IP tunnel, GetInfo, …).

## 7. Reports and tests

- After a run: per traffic type delivery %, median/95 % latency, share via post
  office, share via carry, airtime per type, top bottleneck nodes.
- Compare two runs (feature on/off) side by side, as `final.mjs` does today.
- `tools/denmark_sim_test.mjs` gets one test per traffic type and one
  "cut mid-flight, restore, delivered" test.
- `cargo test -p maarse-sim-wasm` for the new engine parts.

## 8. Feature list for the system description (to document later)

Found while going through the code and memory. Each needs its own section in
the coming "how Må:rse works" document.

**Radio and routing**
- LoRa radio settings (one setting for the whole net), duty cycle, CAD, airtime
- Node roles: client, relay, repeater, anchor, host, Wi-Fi node, mobile
- Flooding, path requests, routes, local repair, route persistence
- Announces, known contacts, pinned contacts, key requests, key cache
- Hello, hold unknown sender
- Regions, bordercast, area scope
- Automatic forest mode (auto relay) and edge bridges, relay hop limit
- Mobile mode (cars)
- Anchors and split long routes (host anchor)
- Load control and QoS priorities
- Smaller packets (textz compression, stamps)
- Dedup, bloom filters

**Delivery**
- Acks and false-ack protection
- Store-carry-forward (carry, delivered notice)
- Post offices / mail system and LoRa trunks
- Crisis carry

**Hosts and backbone**
- maarse-host, TCP backbone, Wi-Fi trunks, DHT lookup
- maarse-server (VPS): admin, telemetry dashboard, capabilities, guard, modem key
- Wi-Fi modem, modem auth
- MQTT and MeshCore bridges (`mqtt.rs`, `meshcore.rs`)
- Tunnel / VPN (`tunnel.rs`, `tun.rs`)

**Content**
- Text chat, area alerts v2 (signed bulletins), alert levels
- Voice messages (maarse-voice), call test
- LoRa file transfer, filebox
- Pictures (1-bit dithering)
- Pages (`maarse://`, BBS style, locks, boards, picture menu, server Pages tab)
- History, sequence numbers

**Identity and security**
- Identity, recovery phrase, safety numbers
- Linked devices
- Crypto (maarse-crypto), network key (planned)

**Clients and devices**
- Android app (native, JNI), web client, tray app
- Firmware: Heltec V4, XIAO nRF52840, Heltec display pages
- Web flasher, release pipeline, UF2
- Range test, diagnostics, debug view

**Simulators and lab**
- maarse-sim CLI, Denmark simulator, maarse-lab

I will save this list as `docs/maybe/feature-inventory.md` when we start and add to
it as we go.

## 9. Order of work

Revised order (2026-11-18): 0 remove Ask Claude; 1 new layout (4 modes,
clickable map, follow panel, timeline); 2 generator per region + app users;
3 shared host rules library + transfer model (voice, pictures, files, Pages);
4 live item state on the map; 5 phone simulators on the real web client;
6 calls, tunnel/SSH/mosh, channels; 7 smarter presets with expected results
in CI; 8 share link, README, feature inventory.

Original table:

| Step | Content | Size |
|---|---|---|
| 1 | Generator (section 3), no engine change | M |
| 2 | Live follow + break/restore mid-flight (section 5) | L |
| 3 | App/internet users (section 4) | M |
| 4 | Pictures, files, voice messages (chunked transfer model) | L |
| 5 | Pages, calls, VPN, linked devices | L |
| 6 | Reports, run comparison, tests | M |
| 7 | Presets rebuilt on the generator, smarter and more of them (e.g. DR outage, Storebælt bridge cut, Bornholm cable cut, summer festival + calls, regional power cut with post offices, SSH to a server across the country) | M |
| 8 | README for the simulator, feature inventory file | S |

Each step ends in a working simulator that is pushed, so we can stop after any
step.

## 10. Answers (2026-11-18)

1. Internet-only users without a LoRa radio are users of their own: `Kind::App`
   with its own identity, reaching the net over TCP to a host.
2. Calls and voice: go by what exists (section 6a): voice messages as Codec2
   files work over LoRa; calls are Opus over the internet only.
3. VPN: the IP tunnel, used for SSH to a server and the like.
4. Generator: the five regions for now.
5. Presets: keep them, make them smarter, with more data and more scenarios.
6. Scale: 2,000 nodes is enough.

## 10b. Old questions

1. Should "internet only" users exist at all in the net, or only as app users
   of a host (the phone runs no router of its own)?
2. Calls over LoRa: is push-to-talk with a few seconds' delay acceptable as the
   expected result, or do we only simulate calls over internet/Wi-Fi trunks?
3. VPN: which use case? (A few kB/s between two hosts, or a whole IP net over
   the mesh?)
4. Generator regions: the five Danish regions, municipalities, or a free box on
   the map? (Proposal: regions + free box.)
5. Should the generator also make the old presets, or do they stay as they are?
6. Scale: how many nodes at most? Today ~2,000 runs in the browser; 10,000+
   needs a performance step (and maybe a Node/CLI mode without the map).
