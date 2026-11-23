# Redesign C ("Mesh native"): specification for the build

Decided with the user on 2026-11-23. Applies to the Android app and the PC web client (maarse-client, served by maarse-host). Both must end up level. English only in code, UI text and docs; the product name is written **Må:rse** wherever a person reads it (see CLAUDE.md).

Mockups (reference, private) are the source of truth for layout. Their generator sources are local files:

`C:\Users\Dennis\.claude\projects\C--Projects-maarse\memory\mockup-src\`

- `engine.js`: phone engine. The original C screens are the branches with `D.key==='C'` (chats, chat, map, nodes, mesh, settings from the avatar). `c2_override.js` is the C2 variant with a network switch: **not chosen, but its Map, Nodes and MeshCore screens are good detail references**.
- `pc.js` + `orig_patch.py`: the PC window. `orig_patch.py` turns it into the **original C** (4 places, no switch), which is the chosen PC design.
- `sens.js`: every sensor screen for phone and PC.
- `shots.json`, `shots_pc.json`: captures of today's apps (base64).
- Notes: `memory/app-redesign-mesh-native-c2.md`, `memory/sensors-mockup-mesh-native.md`.

Published mockups: Android C https://claude.ai/artifact/HwZWpX3MLU3f3Qokyrodq9 · PC original C https://claude.ai/artifact/TuQ4YmuEFs6jh9NyCwVtdV · Sensors https://claude.ai/artifact/9F2ma9MtfGc4KyBh5xyQno

## Decisions

1. **Android uses original C, not C2.** Four places in the bottom bar: **Chats, Map, Nodes, Mesh**. Settings ("Me") open from the avatar at the top right of Chats. There is **no network switch** and no fifth tab.
2. **PC uses original C too.** Four places in a collapsible left rail, settings from the avatar in the top bar. Three panes (list, content, details). Mouse first (hover actions, right-click menus, tooltips), keyboard second (Ctrl+K palette, Ctrl+1-4, Ctrl+N, Ctrl+I, Ctrl+, Esc, Del, arrows, Enter). The wide screen is used: table for nodes, dashboard for Mesh, full-canvas map.
3. **MeshCore is a marker, never a theme.** The app keeps its green look everywhere. MeshCore items carry a small violet tag (`NetworkTag`), a violet section title and a violet unread badge. Violet: light `#5B4FC4`, dark `#B3A8FF` (see `ui/Kit.kt`, object `Mc`). The old amber MeshCore theme (header chip, buttons, bubbles) is removed. Amber means only "waiting" (a message in a post office, a quiet sensor); red means "act" (alarm, error).
4. MeshCore is **read-only in tests**: never send in open MeshCore channels, never log in to repeaters.
5. Colours of the Må:rse palette are unchanged.
6. Texts are English in code. New strings need Danish entries: do **not** edit `i18n/da.json` directly. Write them to `i18n/pending-<your-name>.json` (same shape as da.json) and report it; the lead merges them.
7. Do not commit with any Claude attribution. Work on your own branch/worktree. Do not push.

## The places

### Chats (phone and PC)
- Header "Chats", the avatar opens Settings. A status pill is not needed (the Mesh tab tells the state); an amber banner appears only when something is wrong (not connected, no connection).
- Filter chips: All, Unread, People, Channels, MeshCore.
- Sections: **Channels**, **Direct**, and a **MeshCore** section (violet title, row label per kind: Channel, Contact, Repeater, Room, violet unread badge). MeshCore chats open the existing MeshCore chat screens, restyled to the same look as Må:rse chats.
- An alert card (verified public alerts, sensor alarms) is shown at the top **only when one exists**. The permanent "Alerts" row is removed; Alerts stays reachable from Mesh › Details.
- Contacts without a name show `Unnamed · 4230` (see `friendlyName`) and a "Give this contact a name" action in the contact page.
- The new-chat button sits above the tab bar; the list has bottom padding so it never hides the last row.
- New chat: three ways (join or create a channel, add by QR code, add from a link); typing a 32 character address is a last resort.
- Conversation: compact bubbles (time and delivery tick on the same line as short text), delivery in words on the last own message and on tap, route chips under received messages (hash path / hops), header shows who and how reachable ("Nearby · direct", "Reachable · 2 hops"), attach in direct chats, voice, file progress. MeshCore chat shows "Heard by N repeaters · best SNR".

### Map (new)
Offline grid map, no tiles, no internet. Chips: All, People, Repeaters, Sensors. Nodes with a shared position are drawn: dot = person or contact, rounded square = repeater, diamond = room, teal rounded square = sensor (red ring = alarm, amber = quiet). MeshCore nodes carry the violet tag. Tapping a dot shows a card (name, kind, hops, signal, Message / Open). Nodes without a position are not drawn; say so. Positions are a Må:rse announce feature (`lat/lon` in NodeEntry); MeshCore adverts may carry one too.

### Nodes
Segment control **Nodes | Sensors | Pages** (`SegmentedTabs`). Nodes: search, chips All, People, Repeaters, MeshCore; rows show kind, hops, last heard, signal bars (`SignalBars`) and the violet tag for MeshCore nodes; "Announces" entry; MeshCore actions (Advert, Scan now) when a MeshCore radio is connected. Sensors and Pages are the existing screens restyled (see below).

### Mesh (replaces "Network")
One screen, one honest sentence first ("Online, no radio in use"), then only the tiles that apply (Post office, Backbone, Carrying, Radio), the airtime bar against the 10 % limit when a radio is in use, backbones, a **MeshCore status card** (radio connected, nodes heard, repeaters, Advert, Scan now), and Details (Neighbours and routes, Alerts, All numbers, Log). Buttons sit next to what they act on (Announce now, Sync now, Turn on radio).

### Settings / Me (from the avatar)
Profile first. Groups: **Må:rse** (Account and devices, How this phone connects, Name and position, Internet link and post office, Role and regions, Firmware), **MeshCore** (setup, sending settings, repeater admin; violet group title), **Notifications** (separate tone per network so you can hear which network called), **This phone** (Language, background). Language moves out of Notifications. Each sub-page has a back arrow and no tab bar. The Settings entry never reopens on its last sub-page.

### Sensors
See `memory/sensors-mockup-mesh-native.md`. Phone: Nodes › Sensors cards with summary line, detail page, empty state, start page (USB card + nine template cards), USB claim page, add-sensor page with board note and tags (I2C, 1 pin, needs ADS1115), designer in **three steps** (Setup, Rules, Test and send) with a fixed "Fits on the sensor" bar, rule as a sentence of chips, threshold sheet, test and send. Alarms also show as a card at the top of Chats and on the Map. PC: card grid with details panel, designer as **one workspace** (Setup | Rules | live Test), USB claim page, add dialog with categories, popover for thresholds.

### Pages
Favourites first, address field second. Page view as today (restyled).

## Quality bar (the user's words: "the design must fit in the closet the first time")
- Go through **every** menu, dialog, sheet and sub-page that belongs to your files and make them fit the new design. Double-check that nothing overlaps (text over buttons, buttons under the tab bar or the keyboard, labels that wrap into two lines, clipped text, 48 dp touch targets on the phone) and that no item appears in two places.
- Keep Android and PC level: anything user-facing you change must exist on both sides. Report anything that exists only on one side.
- Keep behaviour: this is a restyle and reorganisation; do not change protocol, storage or radio behaviour.
- When you find something that could improve the design, **write it down and tell the lead**; do not decide big changes alone.
- Keep notes: for your area, list what you found, what you fixed and what must be fixed later.

## Test devices
A Galaxy S25 (adb RFCY214X98D) with the dev app, a Heltec V4 (COM6) and a XIAO nRF52840 (COM7), the PC with maarse-host (127.0.0.1:8080), and nj01 over SSH. All dev, not production. The lead does device tests; agents compile and review.
