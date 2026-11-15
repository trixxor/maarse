# UI overhaul: inventory, structure and two mockups

This is a proposal only. `maarse-client/index.html` and the Android app have not been changed.

- [ui-a.html](ui-a.html): Direction A, calm messenger
- [ui-b.html](ui-b.html): Direction B, operator console

**Update 2026-11-15, the user's choice:** direction A, with the maps, graphs and activity log from B. `ui-a.html` now also has:

- **Network (phone and desktop):** a graph of the last hour against the legal 10 % limit (switch to packets, noise floor, battery or 24 h), packets in and out per 5 minutes, and each neighbour's signal as a bar.
- **Map:** offline distance grid with direct links and their signal, filter chips, a legend and a card for the selected node. On desktop there is a mini map on the Network page.
- **Node page:** key numbers, signal and battery/solar over 24 hours, and the Manage section for administrators.
- **Activity:** a live feed of everything the node and host do, in plain words (announces, sends, acknowledgements, mailbox, carry, bridge, links). Problems are tinted, each line expands to the raw packet, and you can filter and export. On the phone it opens from Network › Activity; on desktop it is a card on the Network page. It replaces today's collapsed Log strip.

Both files are static and self-contained, and they show light and dark mode (use the toggle at the top). Each has phone frames (the Android app) and 1200 px desktop frames (the browser). They use system fonts only, like the real client, because it has to work offline.

The Android app is a WebView around the same `index.html` (`android/src/dk/maarse/app/MainActivity.java`). The only native parts are the Bluetooth bridge (`window.MaarseAndroid`), the node picker dialog, permission prompts and "share CSV". So one redesign covers both. The app gets the phone layout and the browser gets the desktop layout, both from the same responsive page.

## 1. Inventory of today's client

Layout today: a header with brand, status text and connect buttons; a left sidebar with "My node", nine tool buttons, channels, contacts and (on a host) a MeshCore section; the main pane shows either a conversation or a tool view; a collapsible "Log" strip at the bottom of the page.

| Feature | Where it lives today |
|---|---|
| Connect: USB, Bluetooth, maarse-host | Header buttons. The remote-host form (address and access key) only appears when you press "Connect to maarse-host" without a saved host. |
| Connection state | Header text "Connected via X" and a dot. No battery, radio, backbone or mailbox. |
| My address, Copy, QR code, Announce | "My node" card at the top of the sidebar |
| Channels: public, #hashtag, #name@region | Sidebar list. Join box under "+ Join a channel"; regions only through the `#name@region` syntax. |
| Private channels: new, use invitation, invitation QR | Buttons under "+ Join a channel"; invitation through a "Del invitation" button in the chat header (Danish leftover) |
| Contacts: add by address or link, nickname | Sidebar list, "+ Add contact". "(not heard)" and "✓" are appended to the name. |
| DM, files and pictures, voice messages | Conversation and composer. Files only in DMs. |
| Delivery states | Text in the message meta: "sending…", "sent", "✓ delivered", "📫 in mailbox", "✓ delivered (carried)", "✉ put in mailbox", "error: …" |
| Calls | "Call" button in the chat header, only if supported; a call bar on top of the chat |
| Safety number, verify | "Verify key" button in the DM header; the view repeats your QR code |
| Status (counters) | Tool "Status": a long grouped table, with a "This node / Remote: X" dropdown |
| Radio settings, role | Tool "Radio", plus administrators, regions and Wi-Fi backbone on the same page |
| Diagnostics | Tool "Diagnostics": battery, noise floor, trace route, neighbours, **your display name**, **position and telemetry sharing**, other nodes' positions as a table |
| Range test | Tool "Range test" |
| maarse:// pages | Tool "Pages" |
| Firmware update (local and remote) | Tool "Firmware", with the same remote dropdown |
| Host settings: VPN, backbone, DHT, LoRa serial, MeshCore gateway, MQTT, regions, storage and mailbox, web client | Tool "Settings" (only on a host connection). Also lists host "Devices". |
| Host service: status, live log, restart, stop | Tool "Service" (only on a host). Also lists host "Links". |
| Backup: recovery phrase, restore, encrypted export and import | Tool "Security" (the page itself is titled "Backup") |
| MeshCore: node, adverts, channels, contacts, ping, room login, admin console, bridge per channel | Separate sidebar section on a host, with its own conversation model ("kept only while the page is open") |
| Remote admin of a node | Hidden in the "This node / Remote: X" dropdown on Status, Radio and Firmware |
| Map | None. Positions only as a table in Diagnostics. |
| Notifications, install app, new version | Loose buttons in the sidebar; a banner at the top |
| Log and errors | Collapsible strip at the bottom. Many errors only go there (for example "Choose a node or enter an address"). |

### The biggest problems

1. **No grouping.** Nine tool buttons in one flat list mix things about your node (Radio, Firmware), the host (Settings, Service), measurements (Range test, Diagnostics), content (Pages) and your identity (Security).
2. **Settings are scattered.** Your display name and position sharing sit under Diagnostics. Administrators, regions and Wi-Fi sit under Radio. Regions exist twice: on the node (Radio) and on the host (`--region` in Settings).
3. **Misleading names.** "Settings" means host settings only and is disabled on USB or Bluetooth. "Security" opens a page called "Backup". Firmware and Settings share the same icon.
4. **Duplicates.** Battery is in both Status and Diagnostics. The host's links are listed in both Settings ("Devices") and Service ("Links"). Your QR code is under My node and again under Verify.
5. **Hidden features.** Remote administration lives in a dropdown. The remote-host form only appears in one path. The regions feature is only reachable through `#name@region` syntax.
6. **Weak connection state.** "Connected via Heltec V4" says nothing about battery, radio settings, airtime, the backbone or the mailbox. Disabled tool buttons give no reason.
7. **Delivery states are emoji text.** They are hard to scan, there is no explanation of what each one means, and there is no way to see the route a message took.
8. **Feedback goes to a hidden log.** Errors and confirmations land in the collapsed Log strip, so users miss them.
9. **MeshCore is a second app inside the app**, with its own sidebar, chat lists and naming ("MC channels", "MC contacts").
10. **Leftover Danish** in visible text ("Del invitation", "Log ud"), which the translation job will fix anyway.

## 2. What other apps do

Sources actually used:

- MeshCore app changelog, the official app (Android, iOS) and web client: <https://app.meshcore.nz/assets/CHANGELOG.md>
- meshcore-open (an open-source Flutter MeshCore client), README: <https://github.com/zjs81/meshcore-open>
- Ratspeak README: <https://github.com/ratspeak/Ratspeak>
- Ratspeak docs, "Your first session": <https://docs.ratspeak.org/docs/getting-started/your-first-session>
- An overview article on Reticulum apps: <https://hamradio.my/rmap-crosstalk-columba-ratspeak-reticulum-mesh-networking/>

None of these sources show screenshots or describe full screen layouts, so the patterns below come from changelog lines and docs, not from looking at the apps. Points on **Meshtastic** and **Sideband** come from general knowledge, not from sources read for this work.

| Pattern | Seen in | Used here |
|---|---|---|
| Main tabs for contacts, channels and map; unread count on tabs; back from other tabs returns to the main one | MeshCore changelog | Five areas with unread badges; Chats is home in A, Network in B |
| Battery icon in the app bar, tap for voltage; sync progress in the app bar; connection type shown in settings | MeshCore changelog | The persistent status strip (A) and telemetry bar (B) |
| Connect screen with BLE, USB and Wi-Fi tabs; onboarding for first device setup | MeshCore changelog | First-run step 1 with three connection choices |
| "Discover" screen for recently heard adverts, showing whether a node is already a contact | MeshCore changelog | Nodes → "Heard nearby" |
| Repeater login as admin or guest, remote management with its own settings screens, path trace with SNR per hop | MeshCore changelog, meshcore-open "repeater hub" | Node page → Manage tab; Trace route |
| Empty contacts list shows an "add contact" button | MeshCore changelog | Empty states always name the next action |
| Separate screens: Network (interfaces and their status), Peers (seen through announces), Contacts (saved), Conversations | Ratspeak docs | Network area; Nodes split into Contacts and Heard nearby |
| Delivery states "Delivered" vs "Offline Inbox" (accepted by a propagation node) | Ratspeak docs | "Waiting in mailbox" vs "Delivered from mailbox" |
| 12-word backup phrase at setup, behind "Tap to reveal", with a confirmation box | Ratspeak docs | First-run step 3 (24 words, hidden until asked for) |
| Nodes list with signal and last heard; map tab; settings split into device (radio) and app settings (not from sources) | Meshtastic | Settings grouped by where things are stored |
| Announce stream, telemetry and map as first-class views (not from sources) | Sideband | Activity feed in B; map with shared positions |

No branding, names, logos or visuals are copied. Only structure and patterns.

## 3. Proposed structure

Five areas. Every feature has one home. Shortcuts elsewhere are allowed, but they open the same screen and use the same name.

| Area | What it holds |
|---|---|
| **Chats** | One list of DMs, channels (#public, #hashtag, region channels), private channels and bridged MeshCore channels and rooms, with filters (All, People, Channels, Unread). Conversation: text, files and pictures, voice messages, calls, delivery states with a per-message timeline. Conversation details: verify safety number, private channel invitation, region, MeshCore bridge, mute. "+" for new message, join #channel, new private channel, use invitation. |
| **Nodes** | Contacts (saved), Heard nearby (announces), Repeaters, MeshCore nodes. Node page: Message, Call, Verify, Trace route, Pages (maarse://), On map, and Manage when you are an administrator (status and counters, radio, role, regions, firmware, over the air). Add contact by link, QR or address. |
| **Map** | Shared positions and links from announces, filters by type and age, rounded positions drawn as circles. Works offline on a distance grid; map tiles could be an option later. |
| **Network** | "Is it working?" in one sentence, then battery, radio, airtime against the limit, noise floor, neighbours, Wi-Fi backbone, mailbox, host links, MeshCore bridge state. Tools: Trace route, Range test, Pages browser, All counters, Activity/Log. Announce button. |
| **Settings** | Grouped by where a setting is stored, and each group says so. **You**: name and contact card (address, QR, link), position and telemetry sharing, backup and recovery, verified contacts. **This node**: radio presets and values, role and regions, administrators, Wi-Fi backbone, firmware. **Host** (only on maarse-host, otherwise shown with an explanation): backbone and DHT, LoRa modem, VPN tunnel, MeshCore bridge, MQTT, storage and mailbox, web access, service and log. **App**: notifications, appearance, saved connections, install app. |

### Where every current feature goes

| Today | New home |
|---|---|
| Header connect buttons, remote-host form | First run, then the connection sheet (tap the status strip) and Settings → App → Saved connections |
| My node card: address, Copy, QR | Settings → You → Name and contact card (QR also in the connection sheet) |
| Announce | Network (button) |
| Join channel, private channel, invitation | Chats → "+"; invitation in the channel's details |
| Add contact | Nodes → "+" (and the Chats empty state) |
| Verify key, safety number | Conversation details and node page (the same screen) |
| Calls | Conversation header |
| Status | Network (summary) and Network → All counters |
| Radio | Settings → This node → Radio |
| Role | Settings → This node → Role and regions |
| Administrators | Settings → This node → Administrators |
| Node regions | Settings → This node → Role and regions |
| Wi-Fi backbone | Settings → This node → Wi-Fi backbone; its state shown in Network |
| Diagnostics: battery, noise floor, neighbours | Network |
| Diagnostics: trace route | Network → Tools, and node page |
| Diagnostics: your name | Settings → You → Name and contact card |
| Diagnostics: sharing | Settings → You → Position and telemetry |
| Diagnostics: other nodes' positions and telemetry | Map and node pages |
| Range test | Network → Tools |
| Pages | Network → Tools; node page opens the same browser |
| Firmware (local) | Settings → This node → Firmware |
| Firmware, status, radio (remote) | Node page → Manage |
| Host Settings | Settings → Host, split into one page per topic |
| Host region (`--region`) | Settings → Host → Backbone and DHT, with a note that it is the host's own filter |
| Host devices and Service links | Network (one "Links of this host" table) |
| Service: status, log, restart, stop | Settings → Host → Service and log; live events in Network → Activity |
| Security / Backup | Settings → You → Backup and recovery |
| MeshCore sidebar | Bridged channels and rooms in Chats; MeshCore nodes in Nodes; the MeshCore radio and bridges in Settings → Host → MeshCore bridge; its state in Network |
| Notifications, Install app | Settings → App |
| Log strip | Network → Activity (B shows it as a live side panel on desktop). Errors from actions show next to the action. |

### Persistent status

Every screen shows which node or host you are on and whether it is healthy: connection and transport, battery, radio setting, Wi-Fi backbone or backbone peers, mailbox and, on a host, the MeshCore bridge. Tapping it opens a connection sheet with the details and Disconnect or Switch node. When disconnected, it turns red and offers Reconnect.

### Delivery states

Each state has a word and an icon, never only an emoji or a colour:

| State | Meaning |
|---|---|
| Sending (try n of 3) | Your node is transmitting |
| Delivered | The recipient's node confirmed it |
| Waiting in mailbox | Stored at the recipient's mailbox node; they were not reachable |
| Delivered from mailbox | They fetched it later and confirmed |
| Delivered (carried) | Another node carried it to them and they confirmed |
| Not acknowledged | No confirmation after the retries. Offers Try again, Leave in mailbox, Trace route. |

Tapping a message shows its timeline (sent via, no answer, left at mailbox, picked up).

### First run and empty states

First run: 1) Connect (Bluetooth, USB or maarse-host), 2) Name (with the note that a name proves nothing, the safety number does), 3) Back up the recovery phrase (hidden until asked for, with a confirmation box; "Remind me later" keeps a warning in Settings), 4) Join #public. Every step can be skipped.

Empty lists always name the next action: "Join #public", "Add a contact", "Share my contact card", "Announce now", "Nobody shares a position yet".

## 4. The two directions

Both use the same structure. They differ in emphasis, density and look.

### A. Calm messenger

- Chats is the home tab. Bottom tabs on the phone; on the desktop a navigation rail, a chat list, the conversation and a details panel.
- A slim status strip on every screen; one plain sentence ("Everything works.") on top of the Network tab.
- Rounded, soft, generous spacing. Keeps today's green palette and system fonts.
- The desktop details panel shows the delivery timeline and a legend of what the states mean.
- **Good for:** newcomers and family use; feels like a normal messenger, so it builds trust through familiarity.
- **Costs:** operators get fewer numbers at a glance; network health is one tap away rather than always on screen; the details panel needs a wide window.

### B. Operator console

- Network is the home tab. A telemetry bar on every screen (battery, radio, airtime, noise, backbone, mailbox, MeshCore).
- Square panels, hairlines, monospace numbers, alerts in words, airtime charted against the legal limit.
- The desktop sidebar lists the five areas with the chats inline; a live Activity feed on the right replaces the hidden log.
- Inline route log on messages; node page with a Manage tab for remote administration; an offline map on a distance grid.
- **Good for:** repeater owners, field tests, debugging; the state of the mesh is never hidden; confidence through transparency.
- **Costs:** denser and more technical for casual users; the telemetry bar takes vertical space on small phones; more live data to keep up to date (more polling or events).

### A middle way

A can borrow from B without losing its calm: the Activity feed as a tool under Network, the inline route log behind a tap, and an "Expert" toggle in Settings → App that swaps the slim strip for B's telemetry bar.

## 5. Open questions

- Should the Map draw map tiles at all? Today the client fetches nothing from outside. The mockups use an offline grid.
- Remote administration over LoRa costs airtime. Should Manage ask before every read, or keep one session open?
- Should the backup reminder block sending until done, or stay a warning? The mockups use a warning.
- Do bridged MeshCore channels belong in the same Chats list (proposed), or in a filter of their own?
