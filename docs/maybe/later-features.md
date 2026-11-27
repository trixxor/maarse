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

## 2. Wi-Fi hotspot gateway on an ESP32 node

**Paused (2026-11-27), kept as a future feature.** The code stays in the repository but is off: the Cargo feature `hotspot` is not in
the ordinary builds, the board runs the ordinary firmware with Wi-Fi off, and a hotspot `SetWifi` is refused. Pick it up from "Status of the
firmware" below. The first thing to do then is to free RAM on the ESP32 (the hotspot leaves only 6.4 KB of main stack free, against 14.4 KB
without it), see the RAM ideas at the end of that section.

Idea: an ESP32 node (Heltec V4, XIAO ESP32-S3) opens its own Wi-Fi network. Phones join it and use the node's LoRa
antenna as a gateway. Each phone keeps its own identity; several people can be on at once.

### What exists today

- The ESP32 firmware has Wi-Fi only as a station (it joins a network), and runs **either Bluetooth or Wi-Fi**, never
  both (heap: about 34 KB for Bluetooth, 37 KB for Wi-Fi). PRG held for 3 s switches.
- "LoRa modem for the host" mode (`SetWifi`, mode bit): the node is a dumb radio over a TCP stream. It sends `MM`, then
  forwards raw LoRa packets with SNR to the host and sends what the host asks for. The Android app has the host
  built in, so a phone can already be the "host" of such a modem.
- Modem key (`maarse_core::modemauth`): challenge-response, so only a host with the passphrase may use the modem.
- nRF52840 boards have no Wi-Fi. This is ESP32 only.

### Recommended design: the node is a shared modem, the phones are the nodes

1. **Access point.** New Wi-Fi mode "hotspot": the node runs a soft AP (WPA2 or WPA3, generated or chosen
   passphrase, hidden or visible name, shown on the display) and a TCP listener (port 4244, say) that speaks the
   existing modem protocol.
2. **Several phones.** The listener accepts up to about 4 clients. Each phone runs its own host (as today) and its
   own identity, and treats the node as its radio. The node does no routing for them.
   - Received packets are fanned out to every connected phone (each phone filters and decrypts its own).
   - Packets a phone transmits go into one shared queue. The node echoes them to the *other* phones, because the radio
     does not hear its own transmission, so two phones on the same hotspot can talk to each other without LoRa.
   - One duty-cycle budget and one fair queue for all phones (a round robin, with a per-phone cap), so one phone cannot
     use up the airtime.
3. **Security.**
   - Wi-Fi passphrase on the AP, and the modem key challenge-response per client on top (already built).
   - End-to-end encryption is untouched: the node only carries sealed packets and cannot read messages.
   - Per-client rate limit (the host's guard has the model).
   - A phone's identity is never shared with the node or other phones.
4. **Activating it.**
   - From the phone over Bluetooth: a new local command (`SetAp`: on or off, minutes until it turns itself off). The node
     switches from Bluetooth to hotspot (the Bluetooth link ends, as with the PRG switch) and the phone joins the Wi-Fi.
   - From a distance over LoRa: a signed `Remote` command from an administrator, `SetAp { minutes }`, so a repeater in
     the field can open a service hotspot and close it again. The passphrase must not travel over LoRa: the node derives
     it (for example from its own identity and a counter) or uses the one set locally, and the administrator reads it
     from the node's display or from the app that knows the identity. It always turns itself off after the time, and
     returns to its old mode (Bluetooth or Wi-Fi station).
   - PRG button long press as a third way, as today.
5. **App side.** An Android "Hotspot node" entry: join the node's Wi-Fi, then connect as modem (the app already
   speaks it). The PC web client could do the same through the host's `--modem-ip`.

### Limits and things to decide

- A soft AP costs more heap than a station and more current (about +80 to 100 mA), so it is a service or event
  mode, not an always-on mode for a battery repeater. Needs a heap and stack measurement first (the Heltec has little stack spare,
  see tools/stack_probe.py).
- Range is the Wi-Fi range of the antenna (tens of metres); the point is that LoRa reaches far, not Wi-Fi.
- While joined to the hotspot the phone has no internet over Wi-Fi. Android usually keeps mobile data for internet; the
  app must bind its own connection to the hotspot network.
- Access point and station at the same time (hotspot plus the node's own uplink to a host) is possible in principle
  but costs more heap; first version: hotspot only.
- Fairness is the real design work: phones that send path requests or files over a shared 10 % duty cycle.
- Alternative not recommended: make the firmware a multi-client companion (like the host). It needs per-client state in
  the node, which the RAM and stack budget on the ESP32 does not allow.
- Security review before use: the Wi-Fi passphrase handling, the remote `SetAp` (replay, who may open it) and the
  echo between phones.

### Simulation of the airtime budget (2026-11-27)

`tools/sim-experiments/hotspot.mjs`: a town (`DK.city`), every user chatting at 0.2 messages a minute (network total about
6,000 s of airtime per hour, so a busy town), and k phones at the gateway's spot. The phones are k nodes at the same place
(they hear each other perfectly, like the node's echo), so the sum of their airtime is what the one shared radio must carry.
The budget is the 10 % duty cycle: 360 s per hour for all phones together. Three seeds, 45 minutes of traffic after warm-up.
Phone traffic per phone: light 1 message per 10 min, normal 1 per 3 min, heavy 1 per min, flood 3 per min.

Phones that do not forward (client role), share of the 360 s budget used by the whole group:

| Phones | light | normal | heavy | flood |
| --- | --- | --- | --- | --- |
| 1 | 1.5 % | 3 % | 18 % | 37 % |
| 2 | 5 % | 13 % | 31 % | 93 % |
| 4 | 10 % | 23 % | 52 % | 150 % |
| 8 | 10 % | 36 % | 103 % | 225 % |

Phones that also forward other people's packets (repeater role, which is what `maarse-host` has):

| Phones | light | normal | heavy |
| --- | --- | --- | --- |
| 1 | 122 % | 122 % | 128 % |
| 8 | 403 % | 413 % | 543 % |

What it says:

- **With client-role phones the budget is not a problem for normal use.** Eight phones chatting normally use about a
  third of it; four phones about a quarter. It becomes a problem with heavy use (8 phones writing once a minute each
  fill it) and with floods (a few phones are enough).
- **Per phone the cost is roughly 10 to 25 s of airtime per hour at normal use.** A message costs about 1 s of
  airtime once acknowledgements, path requests and announces are counted, not only the 0.3 to 0.5 s of the message.
- **If the phones forward, the budget is gone.** One forwarding node in a busy town already needs more than 100 % of
  the budget (forwarding other people's traffic), and eight forwarding phones use four times the budget, although the
  extra phones add less than they would alone because they overhear each other's forwards. The phone host's
  always-repeater role must therefore be switched off while it sits behind a hotspot node; only the node itself
  (or nobody) forwards.
- Delivery in the simulation is 30 to 50 % in every row, including one phone, so it is dominated by the busy town and
  not by the hotspot; this experiment measures the airtime, not delivery. The latency stays at 5 to 25 s without a clear
  trend.
- The sim lets each phone use its own 10 %. The real node needs one shared queue with a fair share per phone
  (budget divided by the number of active phones, with spare capacity lent out), otherwise the heavy and flood columns
  starve the quiet phones.

Ideas to bring the cost down further, to measure next: the node merges identical path requests from different phones;
phones on one hotspot reach each other through the echo without any LoRa; announces from the phones are sent less often
while they sit behind a node.

### Decision (2026-11-27)

**Phones may only work as clients on public repeaters.** A phone that joins a repeater's hotspot never forwards other
people's packets and never takes a relay or repeater role: only the repeater itself forwards. The simulation above shows
why: with forwarding phones the shared budget is gone at one phone. The hotspot protocol therefore carries the rule: the
node tells the phone it is a client link, and the phone's host switches its forwarding off while the link is up (and
the node drops forwarded packets that a phone sends for others, as a second line of defence).


### Status of the firmware (2026-11-27)

The hotspot is built and the access point runs on the Heltec V4: `SetWifi` with the hotspot bit opens a WPA2 network, the node answers
on 192.168.4.1 and the `Wifi` event reports state 4 (open). It is behind the Cargo feature `hotspot`
(`scripts/build-firmware.ps1 heltec-v4 -Hotspot`), not in the ordinary builds. Not yet tested with a phone.

Built and tested without hardware (`maarse-core/src/hotspot.rs`, six unit tests): the DHCP server (one address per phone, the same
one when it comes back, no router and no DNS so the phone keeps its mobile data), the fair share of the duty cycle (a bucket per phone
that fills at 10 % divided by the connected phones, tested to stay within the duty cycle over an hour with four greedy phones), and the
client-only rule (a packet with `hops > 0` is not accepted from a phone).

Built into the node (`firmware/esp/src/hotspot.rs`): the access point, the DHCP task, four phone slots on port 4244 with the backbone
framing, every packet the node transmits sent to all phones, a phone's packets echoed to the other phones and handed to the node's
router, which forwards them as a public repeater does. The `Wifi` event reports the hotspot states 4 (open) and 5 (phones connected).

**The crash, and what it was.** The first builds crashed when the Wi-Fi driver started (`StoreProhibited` in
`esp_radio::wifi::os_adapter::mutex_lock`), in access point mode and also in the station mode that worked before. It was not the access
point and not the esp-radio version. The main stack is what is left of RAM after the statics, and the Wi-Fi driver's start-up uses nearly
all of it: about 51 KB (the ordinary firmware has 65 KB and 14 KB left over). The hotspot first added 27 KB of statics (a TCP buffer pair
per phone, the queues, the network stack's sockets), which left 37 KB, so the driver overran the stack. Found by dumping code addresses from the
stack in the panic handler (the trace went through `WifiController::new` and `esp_wifi_init_internal`) and by building commits with and
without the feature. Now the buffers and queues are on the heap and the stack's sockets are shared with the station mode, so the stack is
57 KB and at least **6.4 KB** stay free while the hotspot runs (14.4 KB in the ordinary firmware). That is tight, which is why the hotspot
is opt-in for now; the heap has 24 KB left. (esp-radio 1.0.0-beta.2 and `esp-metadata-generated` 0.6.0 were withdrawn by the maintainer
without a stated reason and are not needed.)

Not done yet: the Bluetooth command and the signed `Remote` command with a timer to open the hotspot, the phone side (the Android app
and the PC web client connecting to `192.168.4.1:4244` as a backbone peer), and a hardware test with a phone. `tools/hotspot_hw_test.py`
switches the hotspot on and off over USB and shows the state. The simulation above models separate radios; with the node routing for the
phones (one forward instead of one per phone) the real airtime should be lower.


### RAM ideas for the ESP32 (2026-11-27, not started)

From the symbol table of the hotspot build (static RAM 194 KB): `Router<256,1024>` 60.5 KB, `NODE` 32.5 KB, three 8 KB buffers (`MAIL_BUF`,
file `OUT_BUF` and `IN_BUF`) 24.6 KB, task pools about 22 KB, `USB_OUT` and `BLE_OUT` 5 KB each; the heap is a separate 72 KB. The main stack is
what is left, and the Wi-Fi driver's start-up needs about 51 KB of it.

1. Measure first: `tools/mem_report.py` that diffs sections and the biggest symbols between builds, and a build check that fails if `.stack` falls below
   about 60 KB (the hotspot did this to us unnoticed: 65 KB to 37 KB).
2. Share the three 8 KB buffers (about 16 KB saved).
3. Find the smallest router tables with the simulator (`contact_capacity`): delivery and airtime against table size.
4. Trim queue depths (`USB_OUT`, `BLE_OUT`), the BLE packet pool and the Wi-Fi buffer counts; look at the future sizes of the async tasks.
5. Mailbox and carry in flash (16 MB), as in the crisis-carry note (wear and flash-erase stalls to handle).
6. Overlay for the modes that never run together: BLE and Wi-Fi/hotspot task storage on the heap, only for the mode that is active (15 to 30 KB).
7. Run the radio start-up on a temporary stack from the heap, so the main stack can be small.
8. A repeater-only firmware with compact tables (8-byte prefixes), because phones behind a hotspot run their own routers.
9. Spare RTC memory (about 16 KB) as extra heap for data that does not need DMA.
10. The Heltec V4 reports no PSRAM in `espflash board-info` ("WiFi, BLE, Embedded Flash"); check the board, as PSRAM would change the picture.
