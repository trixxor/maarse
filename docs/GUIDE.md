# Må:rse – technical guide (v0.5)

Må:rse is an encrypted mesh network for LoRa that can reach out into the world via TCP and the internet.

## Getting started (Windows and Linux)

1. Install Rust from <https://rustup.rs>.
2. Start a node with the web client:
   - Windows: double-click `scripts\start-host.bat`
   - Linux: `./scripts/start-host.sh`
3. The browser opens `http://127.0.0.1:8080` and connects automatically.

The first start takes a few minutes, because the program is built. After that it starts right away.

**Two computers:** start one as above. Start the other with `--peer <first computer's IP>:4242`.

**Backbone with DHT:** a host with a public address is started with `--public-addr <host:port>` (and `--dht <another backbone host>`). It publishes the addresses it can reach in the DHT. Other hosts look up unknown addresses and connect directly to the right gateway. The DHT only exists over TCP, never over LoRa.

**Firmware for a radio:**

```text
Windows:  .\scripts\build-firmware.ps1 <board>  [-Flash]
Linux:    ./scripts/build-firmware.sh <board>  [flash]

board:    wismesh-tag | xiao-nrf52840 | heltec-v4 | xiao-esp32s3
```

- **nRF52840** (WisMesh Tag, XIAO nRF52840): the script makes `dist/maarse-<board>.uf2`. Double-click the board's reset button and drag the file onto the USB drive that shows up.
- **ESP32-S3** (Heltec V4, XIAO ESP32-S3): the first time, the script installs Espressif's toolchain (`espup`) and `espflash`, which takes a few minutes. The script makes `dist/maarse-<board>.bin` and flashes the board directly if you add `flash`.

Then connect with the web client via **USB / serial** (needs Chrome or Edge) or **Bluetooth** (nRF and ESP32-S3).

## What it can do

| Feature | maarse-host (PC) | nRF52840 | ESP32-S3 |
| --- | --- | --- | --- |
| Encrypted direct messages with "delivered" | yes | yes | yes |
| Forward secrecy for conversations (ratchet, draft) | yes, sessions stored on disk | yes, 8 sessions in RAM | yes, 8 sessions in RAM |
| Channels (public, #hashtag) | yes | yes (at most 4) | yes (at most 4) |
| Pictures and files (encrypted, with an acknowledgement per chunk) | yes, up to 1 MB over the internet, 8 KB over LoRa | yes, up to 8 KB (not tested on hardware yet) | yes, up to 8 KB (not tested on hardware yet) |
| Post office for recipients who are offline | yes (disk, 7 days, 3 copies on other hosts; also files up to 1 MB) | uses one (drops unanswered messages there, asks for its mail); keeps mail only if sent to it (8 KB flash) | the same |
| Remote administration (status and radio on another node) | yes | yes | yes |
| Status page (radio, routing, mailbox, traffic, DHT) | yes | yes, with RSSI/SNR, duty cycle, CAD and errors | yes, the same |
| Radio settings (MHz, bandwidth, SF, CR, dBm, duty %) | – | yes, stored in flash | yes, stored in flash |
| Battery measurement and energy-driven routing | – (always full) | yes, both boards, + USB power | Heltec V4 yes; XIAO ESP32-S3: unknown (no divider) |
| Diagnostics: trace route, neighbour list, noise floor | trace and neighbours | yes | yes |
| Position (opt-in, rounded) and telemetry in announces | yes | yes, stored in flash | yes, stored in flash |
| MQTT bridge with Home Assistant discovery | yes (`--mqtt`) | – | – |
| Display name in announces | yes (`name.txt`) | yes, stored in flash (not tried on hardware) | yes, stored in flash (not tried on hardware) |
| Voice messages (Codec2, 1200 bit/s) | yes, in the web client | yes, as a file (encoded in the web client) | yes, as a file (encoded in the web client) |
| `maarse://` pages | serves `<data>/pages/` and fetches pages | fetches pages (does not serve) | fetches pages (does not serve) |
| Connection | TCP backbone + DHT, UDP, serial link, KISS TNC (serial or TCP), WebSocket, serial LoRa modem | USB and Bluetooth | USB and Bluetooth (yes, not tried on hardware) |
| Display | – | – | Heltec V4: 3 pages, switch with the PRG button |

The web client (`maarse-client/index.html`) works over WebSocket (maarse-host), USB/serial and Bluetooth (Nordic UART). It scales pictures down automatically so they fit under the limit: 8 KB over LoRa and 1 MB over the internet.

**File limit by route.** Before a file is sent, the client asks maarse-host for the limit for the chosen contact (command `0x14 GetFileLimit {dest}`, reply `0x96 FileLimit {dest, max_bytes (u32), flags (u8: bit 0 = LoRa/unknown, bit 1 = mailbox), est_airtime_s (u32)}`) and shows e.g. *"The picture is sent over LoRa and scaled down to 8 KB (about 2 min. airtime)"* or *"Sent over the internet (up to 1 MB)"*. Pictures are scaled down to the limit; other files over the limit are rejected with an explanation. Firmware nodes do not know the command, and then `Info.max_file` is used. The host also enforces the limit on `FileBegin`. The rules (`maarse-host/src/filebox.rs`):

- **TCP all the way** (the recipient is heard over a TCP link, and it and every hop on the route are known backbone hosts: direct TCP neighbours or DHT contacts; a web client at a host has the host's address): up to `--max-file` (1 MB).
- **LoRa** (heard over a serial modem, or through a hop the host does not know is a host, e.g. a radio node behind another host's modem): 8 KB, the same as the firmware. The airtime is estimated from the EU default settings (SF9, 125 kHz, 10 % duty cycle), since the host does not know the modem's radio.
- **Unknown** (never heard, or the link has gone down): the recipient counts as offline, and the file may be up to 1 MB, because it goes into the file mailbox. Without a file mailbox 8 KB applies.
- If the host is in doubt, the answer is never TCP. Files over 8 KB are never sent along a route that is not TCP: the chunks are held back, and the file ends up in the file mailbox. Chunks for a recipient over TCP are not sent out on a LoRa modem.

**File mailbox (maarse-host only).** If the recipient does not answer (as for messages after 45 s, or when the transfer stalls), the sender's host stores the file on disk in `file-mailbox/`: the sealed offer and the sealed chunks, so the host cannot read it. The sender gets `FileDone` with the text *"put in mailbox"*. When the recipient shows up again (announce or new route, the same mechanism as the mailbox for messages), the file is delivered, and the sender gets *"delivered from mailbox"*. At most 1 MB per file, 20 MB per recipient and 200 MB in total (`--file-mailbox-recipient-mb`, `--file-mailbox-mb`; 0 turns it off). Files expire after 7 days. Files are never put in radio nodes' flash mailbox, and a file over 8 KB is only delivered when the recipient is reached over TCP; if it is only reached over LoRa, the file waits (or expires). The file mailbox lives at the sender's host, so that host must be online when the recipient comes back.

Files on firmware nodes (`firmware/common/src/node/files.rs`, the logic in `maarse-crypto/src/files.rs`) use the same format as maarse-host, so the two can send files to each other. The node sends and receives one file at a time in each direction (two static buffers of 8 KB). A received file is sent to the client when it is complete and checked; if no client is connected, it stays in RAM until a client connects (it is overwritten by the next file and disappears on restart). Files are not put in the mailbox: one file would fill the 8 KB of flash completely.

## Web flasher

`flash.html` (served by maarse-host and maarse-server at `/flash.html`, linked from the client's Firmware page) installs firmware over USB from Chrome or Edge, also on blank boards and boards with other firmware:

- **ESP32-S3** (Heltec V4, XIAO ESP32-S3): esptool-js 0.4.5 (loaded from jsDelivr) writes the merged image `dist/maarse-<board>.bin` from 0x0. Trailing 0xFF is cut off, so only bootloader, partition table, otadata (emptied, so ota_0 starts) and the app are written; the Må:rse partition at 0x210000 (identity, contacts, mailbox) is kept unless "erase the whole flash" is ticked. If it will not connect: hold BOOT, press RST.
- **nRF52840** (XIAO nRF52840, WisMesh Tag): "Start bootloader" sends the 1200 baud touch over Web Serial; "Flash over USB" then sends the firmware over the Adafruit bootloader's serial DFU with [nrfutil-web](https://github.com/takkaO/nrfutil-web) 1.0.0 (BSD-3-Clause, from jsDelivr), the same way the MeshCore and RNode flashers do. The page builds the DFU package itself from the UF2 (flat image from the app start, init packet for DFU 0.5: device type 0x0052, any softdevice 0xFFFE, firmware CRC16, packed as `adafruit-nrfutil dfu genpkg` does), so the one UF2 file serves both ways. Fallback: write the UF2 onto the bootloader drive (File System Access API) or save it and drag it over.

**Releases:** pushing a tag `v<version>` runs `.github/workflows/release.yml`: it builds all four boards (version from the tag; OTA packages signed when the secret `MAARSE_SIGN_KEY_B64` and the variable `MAARSE_RELEASE_PUBKEY` are set), attaches the files and `SHA256SUMS` to a GitHub Release, and publishes the flasher on GitHub Pages with the files in `firmware/` (merged ESP images trimmed of trailing 0xFF). There the flasher loads the latest release for the chosen board by itself. A custom domain (e.g. `flash.maarse.dk`) is set with the repository variable `PAGES_CNAME` plus a CNAME record pointing at `trixxor.github.io`. GitHub Release downloads cannot be fetched by a web page (no CORS), which is why the files go to Pages too.

Signed updates of a node that already runs Må:rse still go through the Firmware page (USB, Bluetooth or the network).

## Remote access (e.g. the phone over VPN)

maarse-host can be reached from other machines, e.g. the phone over a VPN home, when it listens on an address other than `127.0.0.1`:

```text
maarse-host --http 0.0.0.0:8080 --serial COM6 --meshcore COM9
```

At start-up the host prints an **access key** (stored in `<data>/access-key.txt`) and a ready-made address for the browser (`http://<pc>:8080/#token=…`). Connections from the PC itself work without a key as before; all others must bring the key (`ws://…/ws?token=…`), otherwise they are rejected (HTTP 403). In the Android app: tap **maarse-host**, enter the address (e.g. the PC's VPN address `10.8.0.2:8080`) and the key; the app remembers them and reconnects by itself. The phone then uses the PC's node: MeshCore gateway, LoRa via the modem, TCP backbone and history.

Only let the host listen on a network you trust (VPN or home network). The key protects access, but the connection itself is not encrypted without a VPN.

## Running the host as a service

`maarse-host --service` is meant for running under a service manager: it never opens a browser, and when settings are saved in the web client it exits with code 75 instead of starting a copy of itself, so the service manager starts it again. The host copes with being restarted at any time: a missing serial device (LoRa modem or MeshCore node) is retried every few seconds instead of stopping the host, an unplugged device is reopened when it comes back, and backbone peers (`--peer`) are reconnected every 5 seconds.

**Linux (systemd).** The unit in `packaging/systemd/maarse-host.service` runs the host as a dynamic user with its data in `/var/lib/maarse-host`, restarts it on failure, and is hardened (read-only system, no home access, no capabilities, only serial devices visible).

```sh
cargo build --release -p maarse-host
sudo install -m 0755 target/release/maarse-host /usr/local/bin/maarse-host
sudo install -m 0644 packaging/systemd/maarse-host.service /etc/systemd/system/
echo 'MAARSE_ARGS="--serial /dev/ttyACM0"' | sudo tee /etc/default/maarse-host
sudo systemctl daemon-reload
sudo systemctl enable --now maarse-host
journalctl -u maarse-host -f      # log
```

Extra options go in `MAARSE_ARGS` in `/etc/default/maarse-host` (restart the service after changing it). `--tun` needs `CAP_NET_ADMIN` and `/dev/net/tun`; add them with `sudo systemctl edit maarse-host` (see the comment in the unit). The web client listens on `127.0.0.1:8080` by default; add `--http 0.0.0.0:8080` for remote access (the access key is in `/var/lib/maarse-host`).

**Windows.** `packaging/windows/install-service.ps1` registers a Scheduled Task that starts at boot as SYSTEM (no login needed). It runs a small supervisor script that starts `maarse-host --service` and starts it again 3 seconds after any non-zero exit. No extra software is needed. From an elevated PowerShell in the repository:

```powershell
cargo build --release -p maarse-host
.\packaging\windows\install-service.ps1 -HostArgs '--serial COM6'
# remove again (data is kept):
.\packaging\windows\install-service.ps1 -Uninstall
```

The exe goes to `C:\Program Files\maarse-host`, data to `C:\ProgramData\maarse-host` (change with `-InstallDir` and `-DataDir`). Output of the running host is in `C:\ProgramData\maarse-host\logs\host.log.out` / `host.log.err`; `host.log` keeps start/exit times and the output of earlier runs. Run the script again to change options or update the exe. If you already have an identity in `maarse-data`, copy that folder's contents to the data folder before installing to keep your address.

**Tray app (Windows).** `maarse-tray` puts an icon in the notification area: green when the host runs and the web client answers, yellow while it starts, red when stopped, grey when the task is not installed. Its menu shows the status (checked every 3 seconds), starts, stops and restarts the task (only these ask for administrator rights, via a UAC prompt; Stop ends the supervisor first so it cannot restart the host), and opens the web client (address from `--http` in `settings.txt`, with the access key), the Service page and the log folder. Install it together with the task so it starts at login:

```powershell
cargo build --release -p maarse-host -p maarse-tray
.\packaging\windows\install-service.ps1 -HostArgs '--serial COM6' -Tray
```

It takes `--task`, `--data` and `--install-dir` if the defaults were changed (the Startup shortcut passes them). `-Uninstall` also removes the shortcut.

**Service page and control API.** The web client's *Service* page (in the tools menu when connected to a host over WebSocket) shows the host's version, uptime, PID, data folder, whether it runs with `--service`, and its links, plus a live log with filter, pause and download, and buttons to restart or stop the host. The same HTTP endpoints can be used by other tools (for example a tray app). Like the other `/api/` endpoints they need the access key as `?token=<key>` (from `access-key.txt` in the data folder), except for requests from the same machine without a foreign `Origin` header.

- `GET /api/service/status` returns JSON:
  - `version` (string), `pid` (number), `uptime_s` (seconds since start), `started` (Unix time in seconds)
  - `data_dir` (absolute path of the data folder), `service` (true when started with `--service`), `restart_exit_code` (75)
  - `links`: the current links, each `{ "name", "kind" ("backbone", "usb-modem" or "wifi-modem"), "up_s", "rx", "tx", "last_rx_s" (null if nothing heard yet), "snr" (null or dB) }`
  - `log_last`: sequence number of the newest log line
- `GET /api/service/log?since=N` returns `{ "first", "last", "capacity", "lines": [{ "seq", "t", "err", "text" }] }` with every line whose `seq` is greater than `N` (`since=0` or no `since` gives the whole buffer). The host keeps the last 2000 lines of everything it prints in memory; `seq` starts at 1 and grows by one per line, `t` is Unix time in milliseconds and `err` is true for lines written to stderr. Poll with `since` set to the previous `last`. If `last` is smaller than the value you sent, the host has restarted and you should start again from `since=0`.
- `POST /api/service/restart` (same as the older `POST /api/restart`) answers `ok` and restarts the host: with `--service` it exits with code 75 so the service manager starts it again, otherwise it starts a new copy of itself.
- `POST /api/service/stop` answers `ok` and exits the host cleanly with code 0. Under the systemd unit (`Restart=on-failure`) and the Windows supervisor (restarts only after a non-zero exit) the host then stays stopped until it is started again by the service manager.

## Dedicated server (maarse-server)

`maarse-server` is the dedicated host for a server on the internet (a VPS, a machine in a rack, a Raspberry Pi with a forwarded port). `maarse-host` stays the program for an ordinary PC with a radio and the chat client. Both are built from the same code by `cargo build --release -p maarse-host`.

**What it does on the network**

- **Hosts find each other through it.** It is a DHT node; hosts that point at it (`--peer server.example.org:4242`) find each other wherever they are.
- **Relay for hosts behind NAT.** A host that cannot open a port connects out to the server; the server publishes the addresses it reaches through it, and other hosts send to them through the server. Messages stay end-to-end encrypted.
- **Mail copies.** As a public DHT node it is one of the replica hosts that keep mail for offline nodes.
- **Stays connected.** It keeps at least 6 backbone links (`--min-peers`), spread over the DHT, and saves the backbone nodes it has seen (`dht-contacts.txt`), so after a restart it finds the network again even if every seed is gone. First contact: `--peer <host:port>`, `--seed-dns <name>` (a DNS name with several servers behind it) or `--public`.
- **Protection:** at most 4 DHT contacts from one network (IPv4 /24, IPv6 /48). Incoming backbone connections (`maarse-host/src/guard.rs`, also on the ordinary host): at most 8 per IP address and 256 in all; at most 120 DHT requests per minute per address; each link rate limited to 100 packets/s (bursts of 300) and 64 KB/s. A link that keeps going over the limit is closed and its address banned for 10 minutes; one sending more than 20 packets that do not decode, for 15 minutes. The block list (IP addresses, networks such as `203.0.113.0/24`, node addresses) comes from `--block` and `<data>/blocked.txt`, and the dashboard's *Connections* tab changes it at once. Loopback is never limited, banned or blocked.
- **Connections tab:** every link with its far end, uptime and traffic, with *Close* and *Block* buttons; the block list with add and remove; temporary bans and why.
- **Routed packets to one link:** a packet with a fixed route goes only to the link of its next hop (learned from the traffic: a node's own hello or announce, or the previous hop of a routed packet); before, it went out on every backbone link. A retry of the same packet within 10 s goes to every link, so a wrong guess never costs a message.
- A LoRa modem can be attached as on the host (`--serial COM6`); a VPS has none.
- **Says what it offers.** Every host and server tells the backbone what it offers: post office (and room left), relay for hosts behind NAT, radio, regions and version. Directly connected peers hear it in the link hello; others ask over the DHT every 30 minutes. Replica post offices are only picked among hosts that keep mail; an older host that does not say is treated as before (post office and relay).
- **Choices:** `--no-post-office` (keeps no mail for others; mail from its own side still goes to the recipient's replica post offices), `--mail-quota-mb <N>` (at most N MB of mail for others; full = refused and told), `--no-relay` (publishes only the addresses it reaches over its own radio). The same options exist on `maarse-host`.

**Telemetry dashboard (admin login)**

`http://<server>:8080/` asks for the admin password. At the first start a random password is printed once and written to `admin-password.txt` in the data folder (delete the file once you have it); `maarse-server --set-admin-password <password>` sets another. After 5 wrong passwords from one address it is locked for 15 minutes. The dashboard is plain HTTP: over the internet put it behind HTTPS (Caddy or nginx as a reverse proxy) or use an SSH tunnel (`ssh -L 8080:127.0.0.1:8080 server`, and start with `--http 127.0.0.1:8080`).

What it collects (from everything passing through; message contents stay encrypted):

- **Overview:** packets in and out per minute, unique packets, packet types, nodes heard, delivery (messages seen and receipts), routes found, radio vs backbone with average SNR, work done (forwarded, delivered, DHT, mail), for the last hour, 6 hours or 24 hours.
- **Live traffic:** every packet in and out (link, type, plain-language meaning, destination with name, hops, size, route, SNR), every router decision, DHT request and lookup, post office action, modem poll, admin login and log line.
- **Nodes & map:** a census of every node ever heard from announces: name, role, last and first heard, announces, minimum and last hops, the link it came in on, SNR, distance in km from the server, battery, external power, energy, temperature, uptime, whether it hears a repeater, mobile flag, post office class, capabilities; on a map (Leaflet with OpenStreetMap) when nodes share a position.
- **Destinations:** where traffic goes: per address the messages, file pieces and post office packets towards it, route searches and key requests for it, receipts, last route.
- **Routes & repeaters:** route-length and hop histograms; every repeater path ID seen on routes with how often, how often as the first repeater after the sender, average position, and the node behind it.
- **Delivery & latency:** for direct messages passing through, the time until their receipt passes back (median, 90 %, slowest, against hops), the share with a receipt, and route search times against route length.
- **Journeys & carry:** carried messages (handoffs seen, messages carried, delivered notices, time from the first handoff seen to the notice), the time mail waited at this server's post office, and a journey per direct message: every time it, a carried copy, its mail entry or its receipt passed here, with link, hops, route or handoff (giver → taker, tokens, lifetime left) and the time between steps. The sender is never known (it is inside the encrypted message), and only what passes this server is seen.
- **Radio & modem:** the attached modem's own counters every 30 s (radio rx/tx and errors, channel busy, duty cycle, airtime, RSSI, SNR, noise floor, battery, memory, queue, crashes), the radio settings, SNR and packet-size histograms, radio neighbours.
- **Links, Backbone (DHT), Post office, Counters, Log:** per-link packet mix, DHT operations, nodes and records, mail waiting and post office operations, packets by type, router decisions and counters, regions, and the server log.

Totals, histograms, the 24-hour series and the census are saved to `telemetry.json` every 5 minutes and kept across restarts.

**Settings in the dashboard.** The *Settings* tab sets the public address, backbone port, peers, DNS seeds, minimum links, regions, strict regions, post office and quota, relay, LoRa modem and trunks, and the dashboard address, and changes the admin password. "Save and restart" writes `settings.txt` in the data folder (validated first) and restarts the server; the login survives the restart (sessions are kept as hashes in `sessions.txt`). Options given on the command line or in the start script win and are shown in the fields. The command line is only needed for the very first start.

**Installing on Debian or Ubuntu**

```text
curl -fsSL https://raw.githubusercontent.com/trixxor/maarse/main/scripts/install-server.sh -o install-server.sh
sudo bash install-server.sh
```

The script installs the packages and Rust, builds `maarse-server` from source (5-15 minutes the first time; on a small VPS without swap it offers a 2 GB swap file), and asks for the public address, port, peers, DNS seed, regions, post office and quota, relay, a USB LoRa modem if one is plugged in, and how to reach the dashboard (HTTPS through Caddy with Let's Encrypt, an SSH tunnel, or plain HTTP). It sets the admin password (yours or a random one, shown once), creates the system user `maarse`, installs a hardened systemd service that starts at boot, and opens the ports in ufw if it is active. Files: `/usr/local/bin/maarse-server`, options in `/etc/default/maarse-server`, data in `/var/lib/maarse-server`, source in `/opt/maarse/src`. Run it again to update; the settings are kept unless you choose to change them.

**Running it**

```text
maarse-server --public-addr server.example.org:4242 [--peer other-server:4242] [--seed-dns seed.example.org]
```

Or fill in the settings at the top of `scripts/start-server.bat` (Windows) or `scripts/start-server.sh` (Linux) and run it: public address, port, peers, DNS seed, regions, dashboard address, admin password, optional modem and data folder. It uses `maarse-server.exe` next to the script, or the one built in the repository (building it the first time).

`--public-addr` is required. Open TCP 4242 (backbone) and 8080 (dashboard, unless you use a proxy or tunnel). As a service: Linux `packaging/systemd/maarse-server.service` (instructions at the top; options in `/etc/default/maarse-server`); Windows from an elevated PowerShell: `.\packaging\windows\install-service.ps1 -Server -HostArgs '--public-addr server.example.org:4242'`. Test: `python tools/server_test.py` (two hosts behind "NAT" write to each other through a server, the dashboard needs the login, a second server finds the first again after a restart).

## Wi-Fi backbone on ESP32-S3 nodes

A Heltec V4 or XIAO ESP32-S3 can join a Wi-Fi network (for example a phone hotspot on a hilltop) and keep a TCP backbone link to a maarse-host. Everyone who can reach the node over LoRa then reaches the rest of the network: the home host, the DHT and anyone behind it.

- **Setup:** connect to the node over USB or Bluetooth, open *Radio* in the web client and fill in *Wi-Fi backbone*: network name, password and host (`host:port`, port 4242 by default). Saving stores the settings in flash and restarts the node. Leaving the password empty for the same network keeps the saved one.
- **The host side:** any maarse-host listening on its backbone port (`--listen`, default `0.0.0.0:4242`). The node connects *out*, so it works behind a hotspot's NAT, but the host must be reachable from that network: a public address, a forwarded port or a VPN.
- **How it works** (`firmware/esp/src/wifi.rs`): esp-radio's Wi-Fi driver with embassy-net (DHCP, DNS, TCP). Frames are the backbone's `length (u16 LE) | packet`; a host's region greeting is skipped. The node sends every packet over both LoRa and the link and treats packets from the link as a perfect radio link (SNR 10), like maarse-host does with TCP. The router's duplicate filter stops loops. The link reconnects every 5 s after a failure; joining is retried the same way.
- **Edge bridge:** the host's announce carries the role *repeater*, so the node hears a repeater over the link. A node with the role *relay* then becomes an edge bridge for its LoRa group automatically (see "Edge bridge").
- **Protocol:** command `0x28 SetWifi` (`enabled | ssid_len ssid | pass_len pass | peer`, local only: it is refused through `Remote`, as it carries the password), `0x29 GetWifi`, and event `0xa6 Wifi` (`enabled | state | ip (4) | ssid_len ssid | peer`; state 0 off, 1 joining, 2 online, 3 linked). The password is never sent back. Flash: `WIF1` at offset `0x340` of the settings page. nRF52840 boards and maarse-host answer `SetWifi` with an error.
- **Two modes** (*Mode* in the Wi-Fi form; bit 1 of the flags byte in `SetWifi`): *mesh node* (default) is a node of its own that peers with the host over the backbone and keeps working if the host goes away. *LoRa modem for the host* makes the node the host's antenna over Wi-Fi, like `--serial` over USB: it sends `MM`, then forwards raw LoRa packets with their SNR to the host and sends what the host asks for; it does not route or announce. The host names the link `wifi-modem <address>` and treats it as LoRa (8 KB file limit, no calls). The display shows `<ip> modem`. In both modes the node greets the host at once, so it shows up before it has anything to send.
- **When the link is lost:** as the host's modem the node does not route, announce or send hellos (the host does). If its Wi-Fi or internet link has been down for 60 s, it works as an **ordinary node** with its own identity and saved role: it routes, forwards, announces and sends hellos, so the LoRa group around it keeps working and messages are carried and kept until the link is back. The display shows `no host: node`, and stat 102 is 1. When the link is up again, it is the host's modem again within 5 s. Meanwhile the server keeps mail for the nodes behind the modem in its post office (no receipt within 45 s) and delivers it when they are heard again.
- **Modem at a fixed address:** set the node's Wi-Fi host to `listen:<port>` (e.g. `listen:4243`): the modem then waits for the host instead of connecting out, and the host connects with `--modem-ip <address>:<port>` (again every 10 s if the link drops). Handy for a radio in a mast on the LAN. The modem key applies the same way.
- **Telemetry over Wi-Fi:** a Wi-Fi modem sends its own counters (both `Stats` frames and its radio settings) to the host every 30 s (marker `0x7E` in the snr byte), so the server's *Radio & modem* tab shows the same as for a USB modem.
- **Modem key:** anyone who reaches the backbone port could otherwise send `MM` and be taken for the host's own radio. With `--modem-key <passphrase>` on the host (or *Wi-Fi modem key* in the server's Settings tab) a Wi-Fi modem must prove the same passphrase, set in the node's Wi-Fi form under *Modem key*: the host sends a random challenge and the modem answers with a hash of key and challenge, so the passphrase never crosses the network (`maarse_core::modemauth`). A modem with the wrong key, no key or older firmware is refused and logged. **maarse-server accepts no Wi-Fi modem without a key**; an ordinary maarse-host without `--modem-key` accepts them as before (a LAN). The node keeps the derived key in flash (`MKY1` at offset `0x420`); it is set with the local-only command `0x2d SetModemKey` and never sent back.
- **Devices in the host:** `GET /api/devices` (and *Devices* under host settings in the web client) lists every link: backbone peers, USB modems and Wi-Fi modems, with uptime, packets in and out, last heard and the last SNR from a modem.
- **One radio at a time:** the firmware contains both Bluetooth and Wi-Fi, but runs only one. With Wi-Fi enabled, Bluetooth is not started (the phone app then reaches the node through the host instead). **Hold the PRG button for 3 s** to switch between them (the node saves the choice and restarts), or use `SetWifi` over USB. The heap then only holds one driver (Bluetooth about 34 KB, Wi-Fi about 37 KB), so `coex` is not needed. `--no-default-features --features board-heltec-v4,ble` (or `,wifi`) builds a firmware with just one of them.
- **Display (Heltec V4):** a line with the links in use (`USB`, `BT` connected or `(BT)` advertising, `WiFi`), the IP address in Wi-Fi mode (or `WiFi joining...`, `no host`), and a short note after a long press.
- **RAM (no PSRAM on the Heltec V4):** the node runs in its own task and the router (about 53 KB) has its own static. Before, the main task's future held the whole node (67 KB), and the stack frame that builds it stayed reserved for the whole run; with both radio drivers built in, the stack overflowed 0.2 s after boot. Boot now uses about 45 KB of stack and leaves about 52 KB unused.
- **Safety:** Wi-Fi starts 10 s after boot, so USB is always reachable first. If starting it crashes the board twice in a row, it is skipped (state 10) until the board has run stably for 60 s. Panics are printed over USB (`PANIC: …`). The status page shows free stack and heap and the crash count (keys 87–92).
- **Tested on hardware** (Heltec V4.3, 14 Nov 2026): joins a WPA2 network, gets an address by DHCP, answers ping (6–20 ms), keeps the link to a maarse-host (`LINKED`) and joins again after the access point restarts; switching between Bluetooth and Wi-Fi with the button works both ways. Not yet tested: messages from LoRa through the Wi-Fi link, DNS names as host, and the XIAO ESP32-S3.
- **Host:** backbone links use TCP keepalive (30 s idle, then every 10 s), so a node that drops off its hotspot without closing the link is noticed and the link is closed.

## Other links: serial, KISS and UDP

Besides TCP peers, the serial LoRa modem and Wi-Fi modems, maarse-host can carry Må:rse packets over a few more link types, in the spirit of Reticulum's interfaces. Every flag can be repeated, every link shows up in `GET /api/devices` (kinds `serial-link`, `kiss`, `kiss-tcp`, `udp`) with packets in and out, and the code is in `maarse-host/src/extra_links.rs` with the pure framing in `maarse-host/src/framing.rs`.

**Serial backbone link** (`--serial-link <port>[:baud]`, default 115200 baud). A direct host-to-host link over a serial line: a null-modem cable, two USB-serial adapters, or radio modems that act as a transparent serial pipe. Both ends run `--serial-link`. Framing: `FEND | stuffed(packet | CRC-16/CCITT-FALSE, big endian) | FEND`, with KISS byte stuffing (`0xC0` as `0xDB 0xDC`, `0xDB` as `0xDB 0xDD`). Since `0xC0` never appears inside a frame, the reader resynchronises at the next frame after line noise; frames with a bad CRC, a bad escape or more than the largest packet are dropped. The port is reopened every 3 s after it disappears, like `--serial`.

*Fast or slow:* at **230400 baud or more** the link is named `serial-link fast <port>` and counts as backbone, like TCP (file limit `--max-file`, region hello, calls). Below that it is named `serial-link <port>` and counts as a slow radio link (8 KB file limit; file chunks for a TCP recipient are not sent on it). For a cable between two PCs, give a high baud rate (e.g. `--serial-link COM5:921600`); for a radio modem whose air rate is low, keep the port speed below 230400 even if the USB side could go faster.

**KISS TNC** (`--kiss <port>[:baud]`, default 9600 baud). Talks to a standard KISS TNC: a hardware packet-radio TNC, Direwolf's serial KISS, or an RNode-like KISS device. Every Må:rse packet goes unchanged into a KISS data frame on port 0 (`FEND 0x00 data FEND`, with FEND/FESC/TFEND/TFESC escaping); frames for other ports and command frames are ignored. The TNC does its own checksum on the air, so no CRC is added. Optional parameters are sent to the TNC when the link opens: `--kiss-txdelay <N>` (10 ms units), `--kiss-persist <0-255>`, `--kiss-slottime <N>` (10 ms units). A KISS link counts as a slow radio link.

**KISS over TCP** (`--kiss-tcp <host:port>`), e.g. Direwolf's KISS port: `--kiss-tcp 127.0.0.1:8001`. Same framing and parameters as `--kiss`; reconnects every 5 s.

**Callsign (amateur radio).** On amateur bands the station must identify itself. `--kiss-callsign <CALL[-SSID]>` wraps every packet in an AX.25 UI frame from that callsign to `QST` (control `0x03`, PID `0xF0`), which other packet stations and monitors can show. Received frames then must be AX.25 UI frames (digipeater fields are skipped); the info field is the Må:rse packet. Note that Må:rse payloads are encrypted, which amateur rules in many countries do not allow: check your licence before using KISS on amateur bands.

**UDP** (`--udp-listen <addr:port>`, `--udp-peer <host:port>`, `--udp-broadcast <port>`). One Må:rse packet per datagram, prefixed with 7 bytes: magic `4D A5`, version `01` and a random instance id (u32 BE) of the sending socket. Datagrams without the prefix are dropped at once. Every remote address is its own link, `udp <addr>`: configured peers stay, peers learned from received datagrams are forgotten after 3 minutes of silence. UDP links count as backbone (like TCP) and get the same region hello.

- `--udp-listen 0.0.0.0:4243` binds the socket others send to; `--udp-peer host:4243` sends to a peer (without `--udp-listen` an ephemeral port is used, and the peer answers to it).
- `--udp-broadcast 4243` is simple LAN discovery: the host binds port 4243, sends every packet to `255.255.255.255:4243` (link `udp broadcast 4243`) and accepts datagrams from any sender on the LAN, which then become `udp <addr>` peers. A host drops its own broadcasts by the instance id. Peers on the LAN get each packet both by broadcast and unicast; the router's duplicate filter drops the second copy.
- *Loss:* UDP gives no delivery guarantee. Routed packets already have per-hop retransmission and end-to-end acknowledgements, and file chunks are resent until they are acknowledged (see "File limit by route"), so a lost datagram costs a retry, not the message. Floods (announces, channel messages) are not retransmitted on any backbone link, but are heard again from other neighbours or at the next announce.

**Tested** without hardware: the codec (stuffing, CRC, resync after garbage, split reads, AX.25), two links over an in-memory serial pipe with line noise, KISS over TCP against a fake TNC on loopback, two hosts over UDP on 127.0.0.1, and a fuzz target (`cargo test -p maarse-host --release fuzz_link_framing`). **Not yet tested** on real serial ports, hardware TNCs, Direwolf or a LAN broadcast.

## Android app (APK)

The app (`android/`) is a native Kotlin/Jetpack Compose app. It has four connection modes:

- **Radio over Bluetooth:** the app is a companion to a Må:rse radio (Nordic UART service), as before.
- **Phone as node:** the whole maarse-host runs inside the app, so the phone is a Må:rse node with its own identity on the backbone. It joins the open backbone by default (`--public`, `--listen off`).
- **Host elsewhere:** follow a maarse-host on another machine (home PC, over VPN) with its access key.
- **Server on this phone:** maarse-server for an old phone on Wi-Fi and a charger (backbone node and post office, dashboard on port 8080). It stops by itself when the phone leaves Wi-Fi or the charger, because mobile networks cannot be reached from outside (CGNAT).

maarse-host is built as a library (`maarse-host/src/lib.rs`, the same `app.rs` as the programs) and embedded through a small JNI crate, `maarse-android`. The app runs it in a foreground service in a process of its own (`:host`), so a fatal error or a settings restart ends only that process. The UI talks to it like the web client does: the companion protocol over WebSocket on `127.0.0.1:47080`, with the access key (`--require-key`, because every app on a phone can reach 127.0.0.1). Its settings are `settings.txt` in the app's data folder, edited on the Settings page. A second foreground service keeps the connection while the screen is off, and messages and alerts arrive as Android notifications (severe, actual alerts on their own loud channel).

`python tools/build_apk.py` builds `dist/maarse.apk` with Gradle (`android/gradlew assembleRelease`); Gradle builds the native library with cargo-ndk first. Needs the Android SDK (platform 35, NDK 27.2.12479018 in `<sdk>/ndk/`), a JDK 17+, Rust with the `aarch64-linux-android` and `x86_64-linux-android` targets, and `cargo install cargo-ndk`. The APK is signed with the debug key, or with the release key from `tools/release.py`. Install with `adb install dist/maarse.apk` or by opening the file on the phone (allow unknown sources).

Files and pictures (pictures are made small enough for LoRa), voice messages (Codec2 through `maarse-voice` over JNI, the same `audio/x-codec2` format as the web client) and calls (Opus with Android's MediaCodec, the web client's call frames, only over the internet) work in the app. **Not in the app yet** (use the web client): the map, remote administration of other nodes, the range test, firmware updates and the alert issuing form.

## The web client as an app (PWA)

The web client can be installed as an app, so you can use a radio via Bluetooth or USB directly from the phone or computer, without a PC running maarse-host. It also works offline once it has been opened.

- **Android (Chrome):** open the client over https (e.g. from GitHub Pages or another static host) or from maarse-host. Tap the menu ⋮ → **Install app** (or "Add to home screen"), or tap **Install app** in the client. Then connect with **Bluetooth** (nRF nodes) or **USB** (needs a USB OTG cable).
- **Windows, Linux and macOS (Chrome or Edge):** click the install icon in the address bar or **Install app** in the client.
- **iPhone/iPad:** Safari has neither Web Bluetooth nor Web Serial. Only the connection via WebSocket to a maarse-host works (e.g. on a PC or Raspberry Pi on the same network).
- **Notifications:** tap **Turn on notifications** in the side panel. The client then shows a notification when a direct message or a channel message arrives while it is in the background. The number of unread messages is in the tab title.
- **Updates:** when a new version exists, the client shows "New version – reload".

Offline cache, installation and notifications need the page to be loaded over http(s) (localhost counts as secure). Opened directly as a file (`file://`) the client works as before, just without those things.

**Hosting the client yourself:** put the files in `maarse-client/` (`index.html`, `manifest.webmanifest`, `sw.js`, `icon.svg`, `icon-192.png`, `icon-512.png`) on any static https host, e.g. GitHub Pages. Bump `VERSION` in `sw.js` when the files change, so users are told about the new version (maarse-host does it by itself from the content). The icons are made with `python tools/make_icons.py`.

## One inbox on several screens (history on maarse-host)

A maarse-host at home can work as one shared inbox that the phone, the laptop and the desktop follow at the same time. The host stores the message history, and every screen that connects fetches it:

- Stored: received direct messages and channel messages, your own sent messages (from all screens), delivery state (sent, acknowledged, in mailbox) and file metadata (name, type, size). **The content of files is not stored** – a file is only shown on the screens that were connected when it arrived.
- If you send from one screen, the message shows up right away on the others. If you read a conversation on one screen, the unread counter disappears on the others too.
- The client fetches the latest 200 messages on connect, and older ones when you scroll up in a conversation.
- `--history-days N` (default 90) decides how long messages are kept; older ones are deleted at start-up and every hour. `--no-history` turns the history off completely.
- Firmware nodes (Bluetooth/USB) have no history; the client works there as before.

**Privacy:** the messages are end-to-end encrypted on radio and backbone, but the host decrypts them, and the history is stored **unencrypted** in `<data>/history/log.txt`. Anyone with access to the host's disk can read it. Use `--no-history` on a shared machine, and delete the folder to remove the history. The format and the protocol (commands 0x30–0x32, events 0xB0–0xB3) are described in `maarse-host/src/history.rs`.

## Contents

| Part | What |
| --- | --- |
| `docs/protocol-spec.md` | the protocol specification (packet format, routing, cryptography, backbone, companion protocol), extracted from the code. CC-BY 4.0. See [protocol-spec.md](protocol-spec.md). |
| `maarse-core` | packet format, routing engine, route table, duty cycle, companion protocol and content types. `no_std`, without heap. |
| `maarse-crypto` | identities, announces, encryption of direct messages (with forward secrecy sessions, see `ratchet.rs`), channels and file chunks. `no_std`. |
| `maarse-host` | gateway and test node: one file, no installation. |
| `maarse-client` | web client in one HTML file. |
| `maarse-sim` | simulator with a radio model, collisions, duty cycle, a motorway scenario and flood mode for comparison. |
| `maarse-uf2` | makes UF2 files from the firmware (no external tools). |
| `maarse-fuzz` | portable fuzzing of all decoders that read bytes from outside. |
| `firmware/common` | shared firmware logic: node, radio loop, statistics and flash storage (settings and mailbox). |
| `firmware/nrf` | nRF52840 firmware: WisMesh Tag and XIAO nRF52840 + Wio-SX1262, with Bluetooth. |
| `firmware/esp` | ESP32-S3 firmware: Heltec V4 (with OLED) and XIAO ESP32-S3 + Wio-SX1262. |
| `scripts/` | starting the host and building firmware (Windows and Linux). |
| `tools/e2e_test.py` | end-to-end test of three maarse-host nodes. |
| `tools/dht_test.py` | test of the backbone DHT with five hosts. |
| `tools/range_test.py` | range test with two radios over USB (bench test or a laptop as receiver), see [FIELD-TEST.md](FIELD-TEST.md). |

## Routing

- Routes are found with a flood through repeaters. The first message travels along in the request and is acknowledged with the reply.
- Repeaters with worse SNR send first, and repeaters with low battery wait longer.
- Flood suppression (a repeater stays quiet when a nearby neighbour with good SNR has sent the same thing), per-hop acknowledgement with retransmission, and end-to-end acknowledgement.
- **Route repair:** without an acknowledgement within 20 s the route is forgotten, and the message is sent again with a new path request (up to 2 times). Unanswered path requests are repeated after 45 s (the reply can take a long time in a large net), and a late reply to an older request still counts.
- **Mobile nodes** (`mobile = true`) do not trust routes older than 30 s.
- **Local repair:** a repeater whose next hop on a path stops answering (no forward after 4 tries) sends the packet straight to a node further along the path that it has heard in the last 2 minutes, or to the recipient itself, with the dead hop cut out. The dead hop is remembered for 5 minutes (or until heard again), so later packets skip it at once. The packet format is unchanged. `maarse-sim --kill N [--kill-at s]` switches N repeaters off mid-run, and `--repair off` compares with the old behaviour (120 nodes, 3 repeaters killed, 8 seeds: 99.2 % → 99.8 % acked, same airtime).
- **Send priorities** (firmware radio queue, `maarse-core/src/qos.rs`): of the packets that are due, acks, path responses and carry notices go first, then direct messages and bulletins, then channel messages, then fragments (files, voice), and path requests and announces last. A full queue drops the least urgent packet. The flood slot follows the SF (`qos::slot_ms`, about 200 ms at SF9 and 1.3 s at SF12), and the ack timeout (20 s), path request retry interval (45 s) and hop timeout (3 s) grow with it (`qos::scale_timeout`: `base * slot / 200`, never below the base; `Router::set_slot`). The firmware applies this at startup and on every `SetRadio`. In `maarse-sim` (5 seeds each, delivered / acknowledged) the SF slot alone was worse, and only together with scaled timeouts did it help: 120 nodes on 30 km, SF11 79/48 % fixed, 77/43 % SF slot, 90/73 % SF slot + scaled timeouts; SF12 65/26, 59/18, 70/37 %. 200 nodes on 12 km, SF11 94/80, 97/93, 99/96 %; SF12 79/29, 74/47, 86/74 %. SF9 is unchanged (99.7-100 %). Try it with `--slot sf [--timeouts scaled]`.
- **Load control** (experimental, off): `qos::ChannelLoad` measures airtime heard and sent over 60 s, and with `Router::load_control` a node at more than 25 % load originates or forwards at most 6 path requests per minute (`qos::PathQuota`; its own requests wait for the next minute), doubles its flood delay and path retry interval; `qos::announce_interval_ms` gives a longer announce interval. In `maarse-sim --load-control on` (200 nodes on 12 km, scaled timeouts) it helped under stress at SF11 (200 messages: 64 -> 75 % delivered, 36 -> 56 % acknowledged; 400 messages: 39 -> 51 %, 8 -> 22 %) but not at SF12 (43 -> 39 % and 25 -> 25 % delivered), and at normal load it cost deliveries (SF11 120 nodes 90 -> 85 %, SF12 70 -> 59 %; 200 nodes SF12 86 -> 74 %) for 10-20 % less airtime. A 40 % threshold with 10 requests per minute was no better. So the firmware does not use it.
- Roles: `Client`, `Repeater`, `Relay` (forest mode, at most 4 hops) and `Anchor`. Relays carry messages they cannot deliver (4 copies, see *Carry*); repeaters and clients do not.
- **Role "auto"** (stored role value 4, for handhelds): automatic forest mode. The node is a client while a repeater or anchor is in range and becomes a relay when none has been in range for 2 minutes (a repeater counts as in range for 5 minutes after it was last heard); after 2 minutes of hearing one again it is a client again. The node logs each switch, and its status reports role 4. In the Denmark simulator (forest and a town with patchy coverage) this beat both "all clients" and "all relays": all relays swamp a town, while auto only turns the handhelds without coverage into relays (see `docs/denmark/README.md`, "Automatic forest mode").
- **Edge bridges in role "auto":** a node that hears a repeater while a neighbor without coverage has no bridge yet becomes a relay for it (after 2 minutes of steady coverage, with chance max(1 / (1 + neighbors that hear a repeater too), 1/4) per 15 s check, so a person on a hill takes the job at once and nodes along a town's coverage edge do not all do it). Nodes without coverage choose one bridge, the nearest one heard announcing (lowest ID among equally near ones), and name it in their announce; a bridge stays while a neighbor names it and steps back 2 minutes after none does. Bridges announce every 10 minutes; a node with a bridge announces at the long interval and soon after its choice changes. In auto mode the router uses boundary forwarding: a bridge forwards a path request from a covered neighbor not at all, and one from a repeater only when the destination is known to have no coverage. Simulated: forest 97 % delivered (77 % without bridges), town 94 % of messages into and out of the uncovered part (78 %) at 32 % more airtime (`docs/denmark/README.md`, "Edge bridges").

### Edge bridge (relays at the edge of repeater coverage)

A relay that can reach a repeater (say, someone who climbs a hill) is often the group's only way into the repeater network. Normal flood suppression silenced it: it heard the group's own copies of a flood with good SNR, concluded that "the area is covered" and stayed quiet, so about half of all routes were lost.

- **Role field in announces:** app-data field `0x06 | 2 | role | flags` (`maarse-core/src/appdata.rs`). Flag bit 0 (`ROLE_HEARS_REPEATER`) means "I hear a repeater or anchor directly". Older nodes skip the field.
- **Neighbor table:** each neighbor stores its role and its own report, when it last forwarded a flood, and when a repeater was heard echoing it ("covered"). Repeaters and anchors are kept when the table is full.
- **Rule** (`Router::is_edge_bridge`): a relay that hears a repeater and has a recent neighbor that does not (by its own report, or, for older firmware, a forwarding neighbor no repeater has echoed) ignores duplicate floods from non-repeaters. A copy from a repeater still silences it, so it does not add traffic where the repeater already covers the group. Observations expire after 5 minutes (`EDGE_WINDOW_MS`).
- **Simulation** (`maarse-sim --hill`, 10 seeds each): with 10–40 people on 3–8 km and one person on a hill that reaches a repeater 12 km away, delivery to the base rises from 55–82 % to 96–100 %, with less airtime. Where repeaters stand inside the forest, the rule costs 0–3 points of acks and up to about 30 % more airtime with partial coverage; with dense coverage it is close to unchanged. `--suppress legacy` runs the old behaviour for comparison. The simulator does not send announces; it hands each node what a neighbor's announce would say.

### Store-carry-forward ("carry") and the delivered notice

With `carry_copies > 0` (on in firmware for the relay role with 4 copies; off for repeaters, clients and the host; `maarse-sim --carry N` to try it), a direct message the router gives up on is carried instead of dropped: the node keeps it with N tokens and hands half of them to new encounters ("binary spray and wait"), and to the destination when it is heard. Copies live 30 minutes. Details are in `maarse-core/src/router.rs` ("Carry").

- **Carry packet** (type 11): a one-element path naming the receiving neighbour, payload `tokens (1) | ttl left in s (u16 BE) | from (PathId, u16 BE) | message`. The message stays end-to-end encrypted.
- **Delivered notice:** when the destination gets a carried message (also a repeated copy), it sends a type 11 packet *without* a path, addressed to `ack_token(message id)` (the same 16-byte token as an ordinary acknowledgement), with no payload: 18 bytes on the air. Nodes without the notice drop Carry packets without a path, so it is backward tolerant.
- A carrier that hears the notice deletes its copy, stops handing out copies, passes the notice on once and remembers it for the rest of the copy's lifetime. It repeats it when it meets a neighbour it gave a copy to (or got its copy from), and answers a handoff of a delivered message with the notice instead of storing it. The notice thus walks back along the spray tree to the sender.
- **Sender:** the router reports `Action::CarryAcked { msg_id }`; the host and firmware send companion event `0xa7 CarryAcked` (`msg_id` u64 LE), and the web client shows "delivered (carried)" (history state 4). A sender that gets an ordinary acknowledgement for a message it still carries (the outbox retried it near a repeater) also stops and tells its carriers.
- **Simulation** (10 seeds, 18 people in relay mode, 60 direct messages in 30 min): forest 8 km, delivered 60 → 76 % (4 copies) / 78 % (8 copies) at +9 % / +13 % airtime (+24 % / +29 % without the notice), and acks now follow delivery (74 / 77 % instead of 58 %). Forest 10 km: 30 → 44 / 45 % at +11 % / +15 %. E45, low repeaters every 15 km: 85 → 94 % at +27 % airtime (+150 % without the notice). E45 with a 100 km gap: no gain, +19–36 % airtime.

## Post offices and LoRa trunks (2026-11-16)

- **Post offices** (`maarse-core/src/mail.rs`, protocol-spec section 8.1): hosts keep mail for nodes that are offline; radio nodes only carry it. Every node picks the best post office it hears (hellos carry the class) as its home and names it in its announce. A message that has not been acknowledged within 45 s goes to the **sender's** home post office, which keeps it and sends copies to the recipient's three replica hosts (rendezvous hashing over the hosts in the DHT, over TCP). The recipient asks for its mail (`MailSync`, 2 hops) when it starts, regains a post office, sees its ID in a post office's hello ("mail waiting") or every 6 hours; its home post office fetches the copies and delivers. A node that has moved also asks its previous home office, which delivers over LoRa if TCP is down. An office without the backbone retries a delivery after 5, 15 and 35 minutes. In maarse-sim, a traveller who switches off in Aalborg and on again in Roenne gets all of the messages written meanwhile, 14 s after switching on (4 % before post offices); with the internet down and LoRa trunks to Zealand and Bornholm, 98 % arrive before the internet returns (`docs/denmark/README.md`, "Post offices and the journey to Bornholm").
- **LoRa trunks:** `maarse-host --trunk <port>[:baud]` adds a LoRa modem with a directional antenna as a long-distance link (e.g. Aarhus - Zealand, Zealand - Bornholm). It is a backup: routed packets cross only when the trunk is their next hop, alerts always, the host's own hellos and announces, and path or key requests only when TCP cannot reach the other end and the destination is not known on this side (else on a second request). The modem enforces the duty cycle. It uses the same frequency, spreading factor and settings as every other node, so every node can always hear every other; trunks are improved with antennas and smarter software, not other channels. Every 10 minutes each end sends a summary of who is behind it (packet type 15, a Bloom filter), so requests cross only for nodes on the far side.

## Neighbors, contacts and keys (2026-11-16)

- **Hello:** every node (firmware and host) tells its neighbors its role, whether it hears a repeater, its chosen edge bridge and whether it moves, in a 22-byte packet that is never forwarded (packet type 12). It goes out on a Trickle timer: 10 s after something changed, then at doubling intervals up to 4 min (repeaters, anchors), 10 min (relays, nodes without coverage) or 30 min (covered clients). Before this, neighbors learned roles only from announces 12 h apart, so a node that had just started did not know a repeater was near and automatic forest mode could turn it into a relay in the middle of a town.
- **Pinned contacts:** the firmware keeps 128 contacts in RAM. Everyone you write to or receive a message from is pinned (up to 32): never evicted, and the only contacts written to flash, so the flash hardly wears. When the table is full, the oldest contact that is not pinned goes. Contacts stored by older firmware are loaded as pinned.
- **Key requests:** a message to a recipient whose key the node does not have waits up to 60 s while the node asks the network (packet type 13); it is sent as soon as the announce comes, or reported as failed. A message from an unknown sender is kept and the sender's key is asked for. A node asked for answers with its announce; a host answers from the announces it keeps.
- **Mobile mode:** a node whose best repeater changes 3 times within 10 minutes (a car passing repeaters) says so in its hellos and announces (role flag bit 3). Routes to and from it are looked up afresh after 5 minutes, and it never acts as an edge bridge; it is off again after 15 minutes without a change.
- **Host:** an anchor by default (`--repeater-role` for the old behaviour), so a route may be 12 hops on each side of the backbone. **Bordercast** (after ZRP): a path or key request that arrives over TCP goes out on the host's radio only when the destination was heard over that radio, or on a second request within 2 minutes (`--no-bordercast` puts every request on the air as before). The host keeps every announce it hears and answers key requests heard over its radio. The mailbox was already kept on disk.
- Measured in maarse-sim before and after (all of it together): national 3 % of messages shown today, 96 % after; forest group 67 → 99.5 % delivered; town with patchy coverage 33 → 81 %; car on the E45 65 → 94 %. See `docs/denmark/README.md`, "The whole chain, before and after".

## Smaller and fewer packets (2026-11-16)

- **Announces go 3 hops** (`ANNOUNCE_SCOPE`), not 8; a moving node's announces still go 8. Further away, a node gets a key with a key request when it needs it (answered by the node or a host's cache). In maarse-sim this cut airtime by 16-32 % with the same or better delivery.
- **Expanding ring search** (after AODV): the first path request goes at most 4 hops when the target was heard that close in the last 10 minutes, and everywhere 10 s later if nothing came back.
- **Compressed text:** short chat messages are compressed with a codebook of common Danish and English words (about 56 % of the size), only to nodes whose announce says they can read it (capability field `0x07`).
- **Mail bundles:** a post office with several messages for a node that can read bundles sends them in one packet with one acknowledgement.
- **Proof of work** on key requests, mail syncs and deposits too, so post offices cannot be filled and requests not flooded for free.
- **Radio queue fix:** the firmware no longer queues a second copy of a packet already waiting (behind the duty cycle such copies piled up, and each made the previous hop retransmit again; in maarse-sim the car on the E45 went from 70 % to 96 % delivered).
- Measured all together, against the chain fixes before this round: national 91 → 96 % shown and 34 % less airtime; car 70 → 97 % and 57 % less; the outage journey to Bornholm 83 → 98 % of the mail and 32 % less; forest 99.7 → 99.1 % (unchanged within noise) and 17 % less; town 59 → 61 %. See `docs/denmark/README.md`, "Smaller and fewer packets".

## Announces

A node tells the network it exists with a signed announce (flooded up to 8
hops). How often:

- **After a start** the first announce goes out 5-35 s later (at random, so
  many nodes that start together after a power cut do not collide), then
  every 15 minutes, so a new or moved node is found quickly.
- **Once it hears a repeater or an anchor** it switches to the interval in
  the radio settings ("Announce every", default **12 hours**, in steps of
  15 minutes up to about 64 hours; byte 15 of the radio settings, 0 = the
  default). The first announce after a start is always sent.
- Every interval varies by ±10 % at random, so nodes do not fall into step.
- Contacts heard in announces are kept in flash (up to 63), so a reboot does
  not forget them; a message from a sender not known yet is kept until its
  announce arrives.

Why so rarely: in the Denmark simulator (about 800 nodes, 20 minutes) an
announce every 15 minutes took more airtime than all path requests and
delivery fell from 99 % to 79 %; every 12 hours it was 98 %. See
`docs/denmark/README.md` ("Reboots and known contacts").

## Spam protection (proof of work)

Announces and path requests carry a small proof of work (a "stamp", inspired by Reticulum 1.6), so nobody can flood the net cheaply. The stamp is a nonce such that `SHA-256("maarse/stamp" | version | type | destination | binding | nonce)` has enough leading zero bits. Checking costs one hash.

| Packet | Binding | Difficulty (v1) | nRF52840 (estimate) | PC (measured) |
| --- | --- | --- | --- | --- |
| Announce | identity (destination) + timestamp | 12 bits, 4,096 attempts on average | about 0.3 s | about 0.2 ms |
| Path request | target + reply token | 8 bits, 256 attempts on average | about 20 ms | about 0.01 ms |

- Repeaters and clients drop packets without a valid stamp: they are not handled and not forwarded. They are counted in the statistics as "Without proof of work" (key 77).
- The firmware computes the announce stamp in slices of 256 attempts (about 20 ms) and yields to the executor in between. The path request's stamp is so cheap that the router computes it right away.
- The difficulty is fixed per packet type and stamp version, so it can be raised with a new version. Details in `maarse-core/src/stamp.rs`.

**Change to the packet format (v0.4):** announces and path requests now have 5 extra bytes at the end of the payload: stamp version (1) and nonce (4, big-endian). The largest app data in an announce is therefore 5 bytes smaller (99 bytes). Older nodes without a stamp are not heard by new nodes, and new announces are rejected by old nodes (the length does not fit). The companion frames are unchanged; the app sees announces without the stamp.

## Regions

Channel messages can be kept within an area. A region is a lowercase name, e.g. `dk` or `nordjylland`.

- **Channel with a region:** join `name@region`, e.g. `public@dk` or `#trip@nordjylland`. What you send to the channel is tagged with the region. `#trip` without `@` is global as before. The channel's key depends only on the name, so `#trip` and `#trip@nordjylland` are the same conversation.
- **Nodes with regions:** every node has a list of up to 8 regions it forwards for. Hierarchy is handled by configuration: a repeater in Aalborg has `dk, nordjylland`, one in Aarhus has `dk, østjylland`. A message in `#trip@nordjylland` then reaches all of North Jutland, but stops at the Aarhus repeater; a message in `public@dk` goes through both.
- **Without regions** a node forwards everything, so the net works before anyone has set up regions. With **strict** (`strict`) a node without regions only forwards messages without a region.
- Only **forwarding** is filtered. Messages to your own node are always delivered, and direct messages (which follow routes) are not affected.
- **Setup:** in the web client under *Radio* (comma separated list + "Strict"), or for maarse-host with `--region dk --region nordjylland` and `--strict-region`. Firmware stores the list in flash, maarse-host in `regions.txt`.
- **Backbone:** gateways exchange their regions when a TCP connection comes up, and only send messages with a region to peers that serve the region. A North Jutland gateway and another North Jutland gateway therefore share `#trip@nordjylland` over the internet, while a gateway in Copenhagen only gets `dk` messages.
- **On the air:** bit 0 in the flags byte means "region follows", and the region is 2 bytes (a 16-bit hash of the name) after the path. Packets without a region are unchanged; older firmware does not forward packets with a region.

## Backbone DHT

- Kademlia-like, with XOR distance on 16-byte addresses, K = 8 and 3 queries per round.
- An entry says: "address X can be reached via gateway G at host:port". The entry contains X's signed announce, so nobody can invent addresses.
- Gateways publish every 30 minutes, and entries expire after 2 hours. If the DHT knows several gateways for the same address, the one with the fewest hops wins.
- When a host sees a path request to an unknown address, it looks the address up. That also happens when a client writes to an unknown address; the message then waits for the answer.
- When the address is found, the host connects directly to the gateway, and ordinary routing takes care of the rest.
- DHT traffic uses the same TCP port as the backbone. It is recognised by the first two bytes (`D7 D7`).
- **Full replication in small nets:** with at most 64 known DHT hosts (`dht::FULL_REPLICA_MAX`), a gateway stores its entries on *every* known host, not only the 8 closest. Every host then holds the whole table: lookups are answered locally, and if the backbone splits, each side keeps its copy until the entries expire (2 hours). When a backbone link comes up, the host republishes all its entries at once, so a healed split learns the other side's addresses again within a maintenance round.
- Mailboxes need no sync after a split: a message waits on the *sender's* host (or one repeater's flash) and is delivered when the recipient's announce reaches it again; the duplicate filter stops double delivery.

## Battery and energy-driven routing

The firmware measures the battery every 60 seconds (average of 8 measurements) and sets the router's energy status from the voltage (`maarse-core/src/battery.rs`):

| Voltage | Status | Forwards |
| --- | --- | --- |
| ≥ 3.90 V or USB power | full | everything |
| 3.60–3.90 V | ok | everything, a little later in floods |
| 3.40–3.60 V | low | only direct messages and path requests |
| < 3.40 V | critical | nothing |

The thresholds follow the Li-ion curve (RAK's OCV table in Meshtastic): 3.9 V ≈ 75 %, 3.6 V is the "knee" (≈ 10–15 %), below 3.4 V the cell is almost empty. To go one step up the voltage must be 50 mV above the threshold (hysteresis). Voltage, percent, USB power and energy status are on the status page (keys 49, 85, 82, 81).

| Board | Measurement | Source |
| --- | --- | --- |
| XIAO nRF52840 | P0.31 (AIN7), divider 1 MΩ/510 kΩ (× 2.961), switched on with P0.14 low; 3.0 V reference, 12 bits | Meshtastic `seeed_xiao_nrf52840_kit`, MeshCore `XiaoNrf52Board` |
| WisMesh Tag (RAK4630) | P0.05 (AIN3), divider 1 MΩ/1.5 MΩ (× 1.73); 3.0 V reference, 12 bits | Meshtastic `rak_wismeshtag` |
| Heltec V4 | GPIO1 (ADC1), divider switched on with GPIO37 high, × 5.12 (4.9 × 1.045), 2.5 dB attenuation | Meshtastic `heltec_v4` |
| XIAO ESP32-S3 + Wio | no battery divider: "unknown" | – |

USB power is seen on the nRF52840 in the POWER register (VBUSDETECT); the ESP32 boards report "unknown".

## Diagnostics

The web client has a **Diagnostics** page (the button next to Status/Radio):

- **Trace route** (command `0x20`, reply `0xA0`): a `Probe` packet follows the known route (or finds one first), and every repeater appends the SNR it heard the packet with. The target answers along the reverse way, and the repeaters again append their SNR. The result is the SNR for every hop both ways. The packet ID is computed over the fixed part only, so implicit per-hop acknowledgements work as for other packets. Older nodes forward the packet unchanged. Format in `maarse-core/src/diag.rs`.
- **Neighbour list** (`0x21`, reply `0xA1`): nodes heard directly (announces without hops and the previous hop in source paths and path requests) with SNR, RSSI, time and packet count.
- **Noise floor** (status key 80): lora-phy does not give the instantaneous RSSI, so the noise floor is estimated from received packets: N = RSSI − 10·log10(1 + 10^(SNR/10)), moving average.

## Range test (field measurement)

The firmware has a range test mode for measuring real range, loss and power use (all earlier comparisons come from the simulator). One node is a beacon that sends numbered packets, optionally stepping through SF7–SF12. The other is a receiver that follows the beacon's SF schedule and reports every beacon with RSSI and SNR. The web client's **Range test** page (connect to the node directly over USB or Bluetooth) adds the phone's GPS position and distance, draws RSSI/SNR/loss and exports CSV. `tools/range_test.py` does the same from a PC. Both nodes estimate the charge used per mode from TX on-time and typical board currents. The step-by-step procedure for a test drive, how to read the results and a results template are in **[FIELD-TEST.md](FIELD-TEST.md)**. Protocol: commands `0x2a`/`0x2b`, events `0xA8`–`0xAB` (`maarse-core/src/rangetest.rs`, `companion.rs`).

## Position and telemetry

Position is **not** shared by default. Under Diagnostics you can choose the precision (off / about 10 km / about 1 km / precise) and telemetry (battery, uptime, energy status); command `0x22`/`0x23`, reply `0xA2`. The node stores the position, but sends it rounded to the centre of a grid cell (0.1° or 0.01°); only "precise" sends the exact position (the client asks first). The fields are in the announce's app data after the FS field (`0x03` position, `0x04` telemetry, type | length | data, at most 41 of 99 bytes in total), so older nodes ignore them. Received positions and telemetry are shown as a list (event `0xA3`), without maps from outside. Temperature is part of the format, but the nRF52840's TEMP peripheral is owned by Bluetooth (MPSL), so it is not sent yet.

## Alerts (signed public warnings)

Public warnings from trusted issuers, like cell broadcast (EU-Alert) but over the mesh, that anyone can check offline. Format and threat model: protocol-spec section 6.6.1 and `docs/maybe/alerting-review.html`.

- **Unforgeable:** an alert is signed with an **issuer key**, a separate key used only for alerts (never the node identity). A receiving host shows it only if the issuer key is trusted (`--bulletin-key <hex>`) or has a certificate signed by a trusted root key (`--alert-root <hex>`). A certificate names the issuer (e.g. "Beredskab Nord") and limits which events it may send, its highest severity and how long it is valid.
- **Not replayable:** every alert has a serial, an issue time and an expiry. Each alert is shown once; an old alert sent again after its expiry is dropped, and so is one issued far in the future. A node without a clock uses the newest verified alert's time as a lower bound.
- **Cannot be faked as text:** the host hands the client a separate alert event; the web client draws it as a banner and in Network → Alerts, apart from all chat. Nothing typed in a chat can look like an alert, and typing on `#bulletin` is refused.
- **Status:** Actual, Exercise or Test. Only an actual, severe or extreme alert that is still valid becomes a banner; exercises and tests are striped and quiet. An update or a cancel refers to the alert's number and can only come from the same issuer.
- **Issuing:** `maarse-host --new-alert-key issuer.key` creates an issuer key (and the same for a root key; keep the root offline). `maarse-host --sign-issuer-cert root.key <issuer hex> "Beredskab Nord" 90 issuer.cert [events mask] [max severity]` makes a certificate. Start the issuing host with `--issuer-key issuer.key --issuer-cert issuer.cert` and use the form in Network → Alerts; an actual alert needs a tick that a second person has checked it (the Hawaii lesson). The certificate is sent before the first alert and then at most hourly; receivers cache it.
- **On the air:** `#bulletin` packets go before other channel traffic, at most 8 per minute per node (the channel key is public, so junk must not crowd out other traffic). Old v1 bulletins (`0xB1`) are still accepted from `--bulletin-key` senders, valid for 24 hours and shown once.
- **Not yet:** repeaters verifying alerts before boosting them (they have no trust anchors), checking in the firmware and Android app without a host, areas drawn on a map in the form, revocation lists, and cryptographic two-person signatures.

## Backup and recovery phrase

Under **Backup** in the web client you can make a new identity from a **24-word recovery phrase** or restore an identity from a phrase. The phrase is made and shown only in the browser (never in localStorage), and the user must confirm three random words before it is installed on the node.

**There is no back door: whoever has the phrase IS you.** If you lose it, nobody can restore the identity.

Derivation (`maarse-crypto/src/recovery.rs`, the same in the web client):

```text
32 bytes of random entropy -> 24 words (BIP39's English word list, 8-bit checksum)
seed   = PBKDF2-HMAC-SHA512(phrase, "mnemonic" + passphrase, 2048)   # exactly BIP39; passphrase optional
master = HKDF-SHA256(salt "maarse/master/v1", seed, info "") -> 64 bytes
identity = Identity::from_secret_bytes(master)   # Ed25519 seed | X25519 key
```

Test vector: "abandon × 23 art" without a passphrase gives the address `6ef182b74279e05122977ced600ae3e6`.

The client installs the identity with `SetIdentity` (`0x34`: flags (1 = confirmed) | secret (64)), reply `IdentityChanged` (`0xB4`: address | public identity). The command only works locally (USB, Bluetooth, maarse-host's WebSocket on 127.0.0.1) and is rejected via `Remote`. The node stores the key (firmware: the identity field in flash; maarse-host: `identity.key` is written atomically, the old one is kept as `identity.old`), forgets routes and forward secrecy sessions and sends a new announce. `0x35`–`0x37` and `0xB5`–`0xB7` are kept free for linked devices.

The node can **not** hand out its secret key (no `GetIdentitySecret`): an identity that was created at random (before the phrase existed) therefore cannot be backed up. The way forward is to create a new identity with a phrase. The address changes, and contacts must add you and confirm the safety number again.

**Transition rule:** today the node's identity IS the phrase's identity, so the same phrase gives the same address and safety number. Later, with *linked devices*, the phrase becomes your master identity, and every device gets its own key, which the master signs (the same derivation, so today's phrases remain valid). Until then: **only use the identity on one device at a time.** Two nodes with the same identity disturb routes and forward secrecy sessions, so messages can be lost.

**Encrypted export (`.maarse-backup`)** contains the client's state: nicknames, the contacts' keys and confirmations, private channels (with keys), regions, role and sharing. The identity is not included (it is restored with the phrase). The file is JSON: `{format: "maarse-backup", v: 1, kdf: "PBKDF2-SHA256", iter: 600000, salt, cipher: "AES-256-GCM", iv, ct}` (base64). The header is AAD, so the parameters cannot be changed unnoticed. The password must have at least 12 characters. We use PBKDF2 (600,000 rounds, OWASP's recommendation for PBKDF2-HMAC-SHA256) in WebCrypto instead of Argon2id, because Argon2id needs WASM or a slow JS implementation of several hundred lines. If the file is small enough, it is also shown as a QR code. On import the state is merged in, and private channels, regions, sharing and role are sent to the node again.

## MQTT and Home Assistant

```
maarse-host --mqtt 127.0.0.1:1883 [--mqtt-prefix maarse] [--mqtt-user name --mqtt-pass password]
```

The host publishes received telemetry, positions and channel messages as JSON (QoS 0, with a small built-in MQTT 3.1.1 client without extra dependencies):

| Topic | Content |
| --- | --- |
| `maarse/status` | `online`/`offline` (will, retained) |
| `maarse/node/<address>/telemetry` | `{"battery_mv":3870,"battery_v":3.870,"battery_percent":70,"ext_power":false,"energy":2,"uptime_s":61,...}` |
| `maarse/node/<address>/position` | `{"lat":56.155,"lon":10.215,"precision":"1km",...}` |
| `maarse/channel/<channel>` | `{"channel":"#aarhus","from":"…","hops":1,"text":"…"}` |

The first time a node sends telemetry, Home Assistant discovery is published (`homeassistant/sensor/maarse_<short address>/<sensor>/config`) for battery voltage, battery %, energy status, uptime, temperature and hops, grouped as one device per node.

## Display name

Under **Diagnostics → Your name** in the web client you can choose a name of at most 24 bytes (command `0x24 SetName`, `0x25 GetName`, reply `0xa4 Name`). The name is sent in the announce's app data as field `0x05 | length | UTF-8` (`maarse-core/src/appdata.rs`), so older nodes skip it. maarse-host stores it in `<data>/name.txt`, the firmware in flash.

Other people's names come with `NodeInfo` (`0xa3`) and are shown in the web client with `~` in front, e.g. `~Dennis`. A nickname you have given the contact yourself wins. Control characters are removed, and the name is truncated without cutting a character in half. The MeshCore bridge shows the name as `[Må Dennis]`, with brackets removed from the name.

**A name proves nothing.** Anyone can call themselves anything. Use the safety number when it matters who you are writing with.

## Voice messages

Tap 🎤 in a direct conversation to record and again to send (at most 20 seconds). The web client downsamples to 8 kHz and encodes with **Codec2 at 1200 bit/s**: 10 seconds take about 1.5 KB and can be sent over LoRa as an ordinary file (MIME `audio/x-codec2`). The recipient decodes and shows a player.

Codec2 comes from `maarse-voice`, a pure Rust implementation (the `codec2` crate) compiled to WebAssembly and embedded in `index.html` (about 96 KB base64). Build it again with `python tools/build_voice.py` if `maarse-voice` changes. Format: `"C2" | mode (5 = 1200 bit/s) | frame count (u16) | frames` (6 bytes per 40 ms at 1200 bit/s).

Host and firmware do not know the format; they just send the file.

## Calls (Opus over TCP)

In a direct conversation via maarse-host the web client shows **Call**. The audio is encoded with WebCodecs (Opus, 16 kbit/s, 20 ms frames, three per message) and sent encrypted as content type `0x06` (command `0x27 SendCall`, event `0xa5 Call`). The host only sends calls when the recipient is reached over TCP all the way; otherwise it answers `NOT_OVER_TCP` (error code 9), so calls over LoRa are not possible. Firmware nodes reject the command. The content: type (1 call, 2 answer, 3 hang up, 4 audio) | call ID (u32) | data; audio is a sequence number (u16) and frames with a length in front. Needs Chrome or Edge (WebCodecs).

## IP tunnel

Every node has an IPv6 address derived from its Må:rse address (`fd` + the first 15 bytes); maarse-host prints it at start-up (`IP: fd…`). Details in `maarse-host/src/tunnel.rs`.

```text
cargo build --release -p maarse-host --features tun
maarse-host --tun                         # Linux (root); /dev/net/tun
maarse-host --wintun C:\path\wintun.dll    # Windows (administrator); wintun.dll from wintun.net
maarse-host ... --tun-lora                # also allow the tunnel over LoRa (otherwise TCP only)
```

Optimised for LoRa:

- **Session:** a random key is sent once encrypted (content type `0x07`); after that the IP traffic goes as `Datagram` packets (packet type 10) with 28 bytes of overhead and without end-to-end acknowledgement. The key changes every 5 minutes, and repeated packets are rejected.
- **Header compression:** the IPv6 header (40 bytes) becomes 2-7 bytes, the UDP header 6.
- **MSS clamping:** TCP SYN gets MSS 142, so a segment fits in one LoRa packet.
- **Fragmentation and NACK:** larger packets are split into pieces; if pieces are missing after 2 s, the receiver asks for exactly those.

A host without an adapter answers ping to its own address itself. Only packets from the sender's own address to ours are let through.

Measured 14 November 2026 (Windows, Wintun; Heltec V4 ↔ XIAO nRF52840 on the same table, SF9/125 kHz): `ping -6` over LoRa about **1.45 s** round trip (the first 4.2 s incl. route and key), 300 bytes (2 pieces each way) about 5.5 s. Over TCP: 3 ms. At SF7/250 kHz the Heltec did not hear the XIAO (only the other way), so the fast setting has not been measured yet. Use mosh rather than SSH over LoRa; web and file transfer are not realistic.

## `maarse://` pages

A maarse-host can share small pages, like NomadNet. Put files in `<data>/pages/`:

```text
pages/index.mu          maarse://<address>/
pages/about.mu          maarse://<address>/about
pages/weather/index.mu  maarse://<address>/weather
```

The web client has a **Pages** button with an address bar, links, a back button and a cache (the latest 30 pages are kept in the browser and can be read without a connection).

How it works (`maarse-core/src/content.rs`, `maarse-host/src/node/pages.rs`):

- The client sends `0x26 RequestPage {req, dest, path}`. The node sends an encrypted direct message with content type `0x05` and the path. Paths may only contain `a-z 0-9 - _ . /`, at most 60 bytes, without `..` and empty segments, so they cannot point outside the folder.
- The host answers with an ordinary file: MIME `text/x-maarse-page;deflate`, the path as name and the markup compressed with raw deflate. The page therefore uses the same per-chunk acknowledgement and the same limit as files (8 KB compressed over LoRa, 1 MB over TCP). A page over the limit is replaced by a short error page.
- If the page does not exist, the answer is a "Not found" page. The same node gets at most 5 pages per 30 seconds.
- The client only shows files as pages when it has itself asked for exactly that page from exactly that node. Other files with the same MIME are shown as ordinary files.
- Firmware nodes can fetch pages (the client is connected over USB or Bluetooth), but do not serve any themselves.

**Designing pages.** A page's whole look is written in its own `.mu` file: page settings (`@bg`, `@fg`, `@font`, `@grid`, `@ticker` …), columns and cards, colours and effects, widgets, notice boards (`@board`) and locked pages (`@allow`). The full reference with examples is [docs/site/pages.html](site/pages.html) (also on GitHub Pages), and five demo designs are in [sample-pages/backbone](sample-pages/backbone/) (live on the backbone node nj01 under `/demo`).

Markup (`.mu`, UTF-8) is deliberately not HTML, so a page from a stranger cannot run code in the client:

```text
# Heading (also ## and ###)
Ordinary text. **bold**, *italic*, ~~struck~~ and `code`.
- item
> quote (lines in a row make one quote)
---
[Link to another page here](/about)
[Link to another node](maarse://0123456789abcdef0123456789abcdef/)
[Link out of the net](https://example.org)   shown as text, not opened
```

Code blocks are written between lines with three backticks.

**Tables, colours and callouts.** A little more like HTML, but still plain text that compresses well (the page goes over LoRa, where every byte costs air time):

```text
| Day | High  | Height |       a pipe table; the second row makes the first a header
|-----|:-----:|-------:|       :--: centres a column, --: aligns it right
| Mon | 06:10 | 0.4 m  |
{red:text}                     text in a colour
{!yellow:text}                 text on a coloured background (highlight)
>!red Gale warning tonight     a coloured box (callout); more > lines continue it
-> Centred line <-             centred; also on headings: # -> Title <-
```

- Colours: `red`, `orange`, `yellow`, `green`, `blue`, `purple`, `grey` (`gray` works too). Each has its own shade in the light and the dark theme, so a page reads well in both. An unknown colour shows the text without colour.
- Colours, bold, italic and struck text may hold other markup (`{blue:**bold blue**}`); colours cannot be nested in colours.
- Tables: every row starts with `|`; the trailing `|` is optional and `\|` is a bar inside a cell. Without the `|---|` row there is no header. Cells may hold inline markup and links. At most 20 columns. A wide table scrolls sideways inside its own box, never the page.
- Byte cost: a colour costs its name plus 3 bytes (`{red:…}` = 6, `{!red:…}` = 7), a callout its name plus 3 (`>!red `), a centred line 6 bytes, and a table 3 bytes per cell plus the `|---|` row. Repeated markup like `| ` and `{green:` compresses to almost nothing: `docs/sample-pages/formatting.mu`, which uses all of it, is 919 bytes and 579 bytes deflated (copy it into `<data>/pages/` to try it) (about 6 s of air, where a 1.3 KB grey picture takes 10–15 s). Keep the separator row short (`|---|---|`); the dashes are only there to be read.
- Older clients show the new markup as its plain text (`{red:18 m/s}`, `| a | b |`), which stays readable.
- Web client and Android app render the same markup the same way (`renderMarkup`/`pageInline` in `maarse-client/index.html`, `parseMarkup`/`Inline` in `android/.../ui/PagesScreen.kt`).

**Pictures.** A line of its own shows a picture from the pages folder (path from `pages/`, lower case):

```text
![Harbour at dawn](harbour.jpg)            grey (the default)
![Map of the trail](trail/map.png "dots")  1-bit ordered dots, best for drawings and maps
![Logo](logo.png "colour")                 colour, up to 8 KB over LoRa
```

The client asks for the picture like a page, with the step as the first path segment. A picture shows "Click here to see the picture"; the click brings the smallest size, and a button under the picture then asks for the next size, one at a time: **Greyscale** (`_grey/harbour.jpg`, about 1.3 KB), **Colour 4 KB** (`_c4/…`), **Colour 8 KB** (`_c8/…`) and, over the internet only, **Full size** (`_full/…`, the original). Over LoRa Colour 8 KB is the largest; nothing over 8 KB is ever fetched. The host makes the smaller sizes itself as lossy WebP (the format with the most picture per byte in our format test). Tapping a picture opens it full screen: pinch or scroll to zoom, drag to move, double-tap to zoom in and out. Pictures are fetched one at a time per node (`maarse-host/src/node/pages.rs`).

**The browser.** Pages open in a small browser: back, forward, reload (stop while loading), an address bar that shows the page title, the node and whether it comes over LoRa or the internet (tap it to type an address), a bookmark star and a home button. Home is the start page with your bookmarks, nodes sharing pages and recently visited pages. A status bar at the bottom shows the route, size and when the page was fetched.

**Forcing a quality.** An `!` after the style makes the page decide: `![map](kort.png "dots!")` always comes as dots, `"grey!"` as Greyscale, `"colour!"` as Colour 8 KB. This holds over LoRa and over the internet, there are no further steps, and over LoRa nothing is ever over 8 KB (`_fdots/…`, `_fgrey/…`, `_fcolour/…`).

**Title.** A line `@title Hals Havn BBS` (anywhere, usually first) is the page's title, like `<title>` on a web page: shown above the page and in the lists of pages, not on the page itself. Without it the first heading is used.

**Favourites and search.** Nodes › Announces has a Users and a Pages tab, each with a search field; a star makes a user or a node's pages a favourite (shown first). The star is also on a node's page and above an open page.

**Pictures in chats.** When a picture goes over LoRa, the web client and the Android app ask how to send it: colour (up to 8 KB), grey (about 1.3 KB) or dots, each shown with its real size and air time.

## Gateway to MeshCore

`maarse-host` can bridge to an existing [MeshCore](https://github.com/meshcore-dev/MeshCore) net, so Må:rse and MeshCore users can write to each other. The host does not reimplement MeshCore's encryption. Instead it drives an ordinary MeshCore node over USB with the same companion protocol MeshCore apps use.

Setup:

1. Flash a MeshCore node (e.g. XIAO nRF52840, Heltec V3 or RAK4631) with the official **Companion Radio (USB)** firmware (`companion_radio_usb`) from MeshCore's web flasher. The BLE variant does not work, because the host talks over serial.
2. Set the radio settings (frequency, SF, BW) like the local MeshCore net, e.g. with the MeshCore app, and give the node a name like `Må:rse-gateway`. Then close the app, because only one can use the port at a time.
3. Start the host:

```
maarse-host --meshcore COM5
maarse-host --meshcore /dev/ttyACM0 --meshcore-bridge public=public --meshcore-bridge #aarhus=#aarhus
```

- `--meshcore <port>`: the port with the MeshCore node.
- `--meshcore-baud <speed>`: default 115200.
- `--meshcore-bridge <maarse-channel>=<meshcore-channel>`: links two channels (can be repeated). `public` is MeshCore's public channel, `#name` a hashtag channel. Without `=` the same name is used on both sides. If the option is not given, `public=public` is linked.

How the bridge works:

- Messages get the sender in front: `[MC Alice] hi` on Må:rse and `[Må 1a2b3c4d] hi` in MeshCore (the Må:rse sender is shown with the first 8 hex characters of the address). In MeshCore the gateway node's name is also in front, because MeshCore always puts the sender's name on channel messages.
- If a hashtag channel is missing on the MeshCore node, it is created in a free channel slot with the key SHA-256("#name")[0..16], which MeshCore apps use. Occupied slots are not touched.
- Loops are avoided: text that has already been bridged (`[MC …`/`[Må …`) is not passed on, and the same text in the same channel is bridged at most once in 5 minutes. So two gateways on the same net cannot send a message round in a circle.
- Direct messages: send a Må:rse DM to the gateway's address with `@Name text`, and it goes to the MeshCore contact `Name`. The contact must be known to the MeshCore node, i.e. it has heard its advert. Replies go back to the latest Må:rse sender. A MeshCore user can also write `@<Må:rse address> text` to the gateway node.

Limitations:

- Messages written in the web client on the gateway host itself are not bridged, because the node does not send your own channel messages back to clients. Write from another Må:rse node.
- Only public channels and hashtag channels. Private channels, files, location and MeshCore's room servers and repeater administration are not supported.
- MeshCore text is at most 160 bytes including the gateway name, so long Må:rse messages are truncated.
- Acknowledgements do not cross the bridge. A DM that has reached the gateway says nothing about whether it has arrived on the other side.
- Tried on hardware 14 November 2026: Heltec V4.3 with MeshCore `companion_radio_usb` (firmware 15 Oct 2026), channel messages both ways between `public` (Må:rse) and `public` (MeshCore), also with a display name. Direct messages and hashtag channels are only tested against frames derived from MeshCore's source code.

## Tests

```sh
cargo test --workspace                  # 180 tests
cargo build --release -p maarse-host
python tools/e2e_test.py target/release/maarse-host
python tools/dht_test.py target/release/maarse-host
node tools/safety_number_test.mjs        # safety number in JS = Rust
node tools/recovery_phrase_test.mjs      # recovery phrase and backup in JS = Rust
node tools/client_syntax_test.mjs        # the web client's JavaScript parses
node tools/client_browser_test.mjs target/release/maarse-host   # the web client in Edge/Chrome: loading, connection, pages, voice
cargo run --release -p maarse-sim -- --help
```

The end-to-end test starts three nodes (A, B and a mailbox P) and tests:
- messages both ways,
- a channel message,
- a message in a private channel (random key) that B still knows after a restart,
- a 20 KB file that is verified,
- a too large file that is rejected,
- that B goes offline, the message goes into the mailbox, and B fetches it when it comes online again,
- that a 20 KB file for B while B is offline goes into A's file mailbox and is delivered and verified when B comes back (and that the limit is then 1 MB over TCP),
- that a 20 KB file is rejected when the route must be counted as LoRa (8 KB),
- remote administration: an administrator gets access, and another node is rejected,
- that a foreign web site cannot connect.
- history with several screens on the same host: a message sent from screen 1 shows up live on screen 2, a new screen 3 fetches the history (in pages, with acknowledgement), and "read" is sent to all screens.

The DHT test starts three backbone hosts (B1, B2 and B3) without packet connection to each other, only with B1 as DHT bootstrap, plus A on B2 and C on B3. A writes to C, which A has never heard, and the message arrives with an acknowledgement. The reply comes back too.

The web client has also been tested in a headless Chrome: text, picture and channel between two nodes, the status and radio pages, and the mobile layout. Safety numbers (the same on both sides, mark as confirmed) and private channels (create, share invitation, join, send) have been tested in a headless Edge.

### Fuzzing

All decoders that read bytes from outside are fuzzed with a small, portable engine in `maarse-fuzz` (deterministic PRNG and mutator, no external tools; also works on Windows/MSVC, where cargo-fuzz/libFuzzer is not reliable). Every target runs half completely random inputs and half mutations of valid frames, and where it makes sense it is also checked that decoded input can be encoded and decoded again unchanged.

Targets: `Packet::decode`, companion `Command::decode`/`Event::decode`, `FrameDecoder` (stream cut into random pieces), `content::parse`, `verify_announce`, `open_direct`, `open_channel`, `open_chunk`, and the DHT's `decode_req`/`decode_resp` (incl. validation and storing of STORE).

```sh
cargo test -p maarse-fuzz --release                     # 100 000 inputs per target
cargo test -p maarse-host --release fuzz_
MAARSE_FUZZ_ITERS=20000000 cargo test -p maarse-fuzz --release -- --nocapture   # longer run
MAARSE_FUZZ_SEED=42 cargo test -p maarse-fuzz --release                         # another seed
```

On a failure the seed, iteration and input are written in hex, so it can be reproduced and added as a regression test.

## Simulator

| Scenario | Mode | Delivered | Acknowledged | Median | Airtime |
| --- | --- | --- | --- | --- | --- |
| 120 nodes, 30×30 km (3 seeds) | Må:rse | 100 % | 98–100 % | 2.3–2.6 s | 341–363 s |
| | flood, 7 hops | 92–100 % | 73–83 % | 5.0–6.5 s | 1,703–1,789 s |
| 200 nodes (40 repeaters), 60×60 km (3 seeds) | Må:rse | 92–95 % | 73–93 % | 5.2–9.2 s | 889–995 s |
| | flood, 7 hops | 65–83 % | 57–70 % | 5.2–7.0 s | 2,887–3,170 s |
| Motorway 100 km, car at 110 km/h (3 seeds) | Må:rse | 100 % | 100 % | 3.8–3.9 s | 313–318 s |
| | flood, 7 hops | 88–95 % | 52–63 % | 4.7–5.4 s | 1,573–1,643 s |

```sh
cargo run --release -p maarse-sim -- --seed 1                                    # 120 nodes
cargo run --release -p maarse-sim -- --nodes 200 --repeaters 40 --area-km 60 --seed 1
cargo run --release -p maarse-sim -- --highway --seed 1
# add --mode flood for the comparison; DEBUG_ACK=1 shows where replies are lost
```

Må:rse delivers more messages than flood with a third to a fifth of the airtime, and handles the car on the motorway.

**Large, sparse nets (v0.5).** Before, the 200-node net delivered 78–92 % and acknowledged only 35–53 % (average over 24 seeds: 83 % / 50 %, 1,453 s airtime). The diagnosis (`DEBUG_ACK=1`): almost all messages travel with the path request, so the acknowledgement is the path request's reply. The reply was lost already on the first hop back, in 60 % of the attempts in a collision with *other path requests*. The sender repeated its request after 15 s while the reply was still on its way over many hops with retransmissions; every new flood drowned the reply and caused yet another repeat. Two changes, both without changing the packet format or the router's RAM use:

1. **Path requests are repeated after 45 s instead of 15 s.** Average over 24 seeds: 92 % delivered, 82 % acknowledged.
2. **Flood suppression only counts strong copies.** A repeater now stays quiet as soon as it has heard one copy with good SNR (above `weak_snr_db`) from a neighbour; weak copies come from far away and do not count. Before, it stayed quiet after two copies regardless of SNR. That halves floods in dense nets and dampens them in sparse ones, and channel messages reach further (average over 6 seeds: 65 → 74 % of the nodes in the sparse net, 84 → 98 % in the dense one). Staying quiet after any single copy, on the other hand, made the flood die out on the motorway.

Together, average over 24 seeds for the sparse net: 94 % delivered, 89 % acknowledged, 901 s airtime. The rest of the loss is hops over weak links (below a 5 dB margin, 40–70 % of them are lost to collisions). Delaying requests over weak links, so strong links are chosen, gave no more acknowledgements, gave longer routes and broke the motorway, so it is not included. The same goes for a longer wait before the reply, a growing pause between hop retransmissions and more hop retransmissions (6 instead of 4): no measurable difference.

The radio model is rough (log-distance, no terrain). The numbers are for comparing variants.

## Test lab (maarse-lab)

All tests, fuzzing, simulator scenarios and benchmarks can be run and watched live in the browser:

```sh
cargo run --release -p maarse-lab                 # opens http://127.0.0.1:8090
cargo run --release -p maarse-lab -- --no-browser --addr 127.0.0.1:8095
cargo run --release -p maarse-lab -- --port COM6 --port COM7   # real boards too
```

- **Test overview:** every suite with a short description of what it tests: `cargo test` per crate (with all test names), `maarse-fuzz`, `e2e_test.py`, `dht_test.py`, `radio_test.py` (needs boards and `--port`), the simulator scenarios (Må:rse and flood) and the stamp benchmark. "Run" per suite or "Run all". Output is streamed live, and every test turns green or red with its time.
- **Simulator live:** runs `maarse-sim --trace-json -` and animates the net on a map: nodes (coloured by role or by airtime used), radio links (stronger for better SNR), transmissions as rings in the packet type's colour, collisions, deliveries and the car on the motorway. Speed 0.25×–50×, pause and step, and Må:rse and flood side by side with the same seed.
- **Hardware:** with `--port` the boards' RSSI/SNR, tx/rx, radio step, role and radio settings are shown live via the companion protocol. Without `--port` no ports are opened. Polling pauses while `radio_test.py` runs.
- **History:** the latest runs are stored in `maarse-lab-data/history.json` and shown as a trend over time (e.g. delivered % per scenario and attempts/s for the proof of work).

The simulator's event trace (`--trace-json <file|->`) is one JSON line per event: `setup` (positions and roles), `links` (audible pairs with SNR), `tx`, `rx` (receivers and collisions), `defer` (duty cycle or busy channel), `send`, `deliver`, `ack`, `move` and `end`. The format is described in `maarse-sim/src/trace.rs`.

## Firmware: status and what is missing

- **Nothing has been tried on hardware yet.**
- **nRF52840:** compiled for both boards (about 270 KB flash, 95 KB RAM). Bluetooth (Nordic UART) is included.
  - WisMesh Tag: app at 0x26000 (S140 v6).
  - XIAO: app at 0x27000 (S140 v7).
  - Flash placement: app at most up to 0x88000, staging area for updates 0x88000–0xE8FFF, OTA meta page 0xE9000, mailbox at 0xEA000–0xEBFFF, settings at 0xEC000 (see "Firmware updates").
- **ESP32-S3:** built for both boards with Espressif's Xtensa toolchain (about 550 KB app on the Heltec V4, of which about 220 KB for Bluetooth; the app partition is 2 MB). `--features check-esp32c6` still type-checks the code against the ESP32-C6.
  - Settings and mailbox are in the partition `maarse` at 0x210000 (see `firmware/esp/partitions.csv`). The app is in `ota_0`/`ota_1` (see "Firmware updates").
  - Heltec V4: the default transmit power is 10 dBm, because the PA adds about 7–10 dB on top. The PA control (GPIO7/2/46) follows Meshtastic for the GC1109 version. If your board has the KCT8103L (newer V4), the control may need adjusting.
  - Bluetooth (Nordic UART) is included: esp-radio's BLE controller with TrouBLE as host, the same GATT service, frames and name (`Maarse-xxxx`) as on nRF. The controller uses a 72 KB heap (esp-alloc). Not tried on hardware, especially not the interplay with LoRa and USB.
- **Missing:** files over LoRa on firmware and segmented paths over 12 hops.
- **Try on hardware (battery and diagnostics):** compare the battery voltage on the status page with a multimeter on each board (adjust the factor in `firmware/*/src/battery.rs` if it is off by more than about 50 mV); unplug USB and watch USB power change (nRF); watch the energy status drop below 3.6/3.4 V (e.g. with a bench supply); trace route over 2–3 repeaters and check that the SNR values match the status page's; neighbour list after an announce; noise floor in a quiet and a noisy environment; position with 1 km precision shown rounded at another node.
- The pin assignments are taken from Meshtastic's board files.

## Firmware updates (signed, over USB, Bluetooth and the net)

So solar repeaters in the field can be updated without being brought in. **Not tried on hardware yet** (see the checklist below).

**Signing.** A release is an image and a 112-byte manifest: board ID, version, size, SHA-256 and an Ed25519 signature (the format is in `maarse-crypto/src/ota.rs`). The node only installs when the signature matches the release key in the firmware, the board ID matches, and the version is newer than the running one (anti-rollback). Only locally (USB/Bluetooth) can an older version be allowed with a flag; via the net the flag is ignored.

```sh
cargo run -p maarse-uf2 --bin maarse-sign -- keygen release.key       # once; prints the public key
export MAARSE_RELEASE_PUBKEY=<hex>  MAARSE_FW_VERSION=0.2.0           # built into the firmware
MAARSE_SIGN_KEY=release.key ./scripts/build-firmware.sh xiao-nrf52840  # -> dist/maarse-xiao-nrf52840-ota.bin + .manifest
```

- Without `MAARSE_RELEASE_PUBKEY` the firmware trusts a **public development key** (`maarse-sign --dev-key`, `MAARSE_SIGN_KEY=dev`). The build warns, and the node writes a warning on `OtaBegin`. Never build releases like that.
- `maarse-sign sign` takes an ELF, `.uf2` or `.bin`. On the nRF52840 the image is the app from its start address (0x27000 XIAO / 0x26000 WisMesh; a wrong board is rejected); on the ESP32-S3 an ESP-IDF app image from `espflash save-image` (the build script makes `dist/maarse-<board>-app.bin`).
- The version is `major.minor.patch` (u32: major << 16 | minor << 8 | patch). Without `MAARSE_FW_VERSION` the crate version is used.

**Companion commands** (0x14–0x17 kept free for parallel work):

| Code | Name | Content |
|---|---|---|
| 0x18 | `OtaBegin` | req (u16), flags (bit 0: allow older, local only), manifest (112) |
| 0x19 | `OtaChunk` | req, offset (u32), data (≤ 240 locally, 128 over radio; offset and length multiples of 4 except the last chunk) |
| 0x1a | `OtaEnd` | req, apply (u8): install and restart |
| 0x1b | `OtaStatus` | – |
| 0x9a | `OtaState` | req, state (0 ready, 1 receiving, 2 verified, 3 installing, 4 error, 5 unsupported), next offset, size, version, running version, text |

All four can be sent via `Remote` (0x0c) to a node you are an administrator of; the reply comes as `RemoteReply`. The transfer is stop-and-wait: the client sends the next chunk when the reply with the next offset has arrived, so duty cycle and loss are handled by themselves. Progress is stored per 4 KB block on a meta page in flash, so an interrupted transfer resumes from the first unfinished block – even after a restart. SHA-256 is checked when the last chunk has been written, and again right before installation. The control counter in `Remote` now always increases (also several times per second).

**The web client:** the *Firmware* button – choose `.bin`/`.uf2` + `.manifest`, this node or a remote node, progress with estimated time, *Stop*, *Resume* and *Get status*. The client checks size and SHA-256 against the manifest before it sends. A `.uf2` is turned into the same image as `maarse-sign` makes (all 256-byte pages from the lowest to the highest, gaps = 0xFF). Without a reply within 5 s (local) / 60 s (radio) the client asks for the status, and the transfer continues from there. maarse-host is not updated itself, but forwards to radio nodes.

**nRF52840 (Adafruit UF2 bootloader).** The bootloader's dual bank DFU is controlled by its own settings during a BLE/serial DFU session and cannot be used for an image the app has written itself. Therefore:

```text
0x00000  MBR            0x26000/0x27000  app (at most to 0x87FFF)  0xE9000  OTA meta page
0x01000  S140           0x88000          staging (388 KB)           0xEA000  mailbox
                                                                    0xEC000  settings
                                                                    0xF4000  bootloader (never touched)
```

The new app (about 300 KB today) is written to the staging area. On installation the vector table is checked, interrupts are disabled, and a small routine in RAM (`firmware/nrf/src/ota.rs`, calls nothing in flash) erases the app's pages, copies the image page by page with a verify read and restarts. It takes about 10 seconds. Bootloader, SoftDevice, identity and settings are not touched. **Risk:** a power loss during those 10 seconds leaves half an app; the bootloader is intact, so the node can always be rescued with a double press on reset and a `.uf2` – but that needs physical access. The app is now limited to 388 KB (WisMesh 392 KB).

**ESP32-S3** (`esp-bootloader-esp-idf` 0.6 has `OtaUpdater`): a new partition table with `otadata` (0xD000, 8 KB; `nvs` shrunk to 16 KB), `ota_0` at 0x10000 (where `factory` was), `maarse` unchanged at 0x210000 (OTA meta page at 0x213000) and `ota_1` at 0x220000 – 4.1 MB in total, so the board must have at least 8 MB flash. The new app is written to the partition that is not running; `otadata` points to it, and the node restarts. If the new app is invalid, the bootloader starts the old one. At start-up the running app is marked as valid. **Transition:** nodes with the old table (`factory`) must be flashed once over USB with the new table (`build-firmware.sh <board> flash`, which also erases `otadata`); identity and settings are kept, because `maarse` does not move. With the old table the node answers "unsupported".

**Airtime over LoRa** for a 300 KB image (2,378 chunks of 128 bytes; per chunk a packet of about 230 bytes, a reply of about 113 bytes and two acknowledgements; one hop):

| Radio | Chunk | Reply | Total airtime | Sender's share | At least at 10 % duty | at 1 % |
|---|---|---|---|---|---|---|
| SF9, 125 kHz (default) | 1.18 s | 0.65 s | 1.6 h | 0.95 h | about 10 h | about 95 h |
| SF7, 125 kHz | 0.37 s | 0.20 s | 0.5 h | 0.3 h | about 3 h | about 30 h |
| SF7, 250 kHz | 0.19 s | 0.10 s | 0.24 h | 0.15 h | about 1.5 h | about 15 h |
| SF11, 125 kHz | 4.7 s | 2.6 s | 6.2 h | 3.8 h | about 38 h | – |

**Shelved (November 2026):** updating over LoRa takes too long (about 1.6 h for the nRF52840 and 3 h for the Heltec V4 at SF7/250 kHz, many hours at SF9) and is not used. Update over USB or Bluetooth. The code is kept.

Every extra hop repeats all of it. **Original recommendation:** update neighbours directly (one hop) on 869.4–869.65 MHz (10 %), temporarily switch both the repeater and the sender to "Fast (SF7, 250 kHz)" during the update, and switch back afterwards. The transfer can be stopped and resumed, e.g. over several nights.

**Must be tested on hardware before use in the field:**

1. nRF52840 (XIAO and WisMesh): USB update to a newer version; the node restarts in the new version with the same identity, settings and mailbox.
2. The bootloader starts the app after the copy (check that the Adafruit bootloader does not require a CRC in its settings for an app it has not written itself – otherwise it stays in DFU mode; rescued with a `.uf2`).
3. An interrupted transfer (unplug USB, restart the node) resumes from the first unfinished block.
4. Rejection: wrong board, same/older version, wrong key, corrupt image (SHA-256).
5. Bluetooth update (Nordic UART, 240-byte chunks).
6. ~~Remote update over LoRa~~ (shelved).
7. ESP32-S3: the first USB flash with the new partition table keeps the identity; OTA to `ota_1`, then `ota_0`; an invalid image in the other partition makes the bootloader start the old one.
8. Bluetooth/LoRa keep running during flash writes (the staging area is written while the radio is active).

**Rescue if something goes wrong:** nRF52840: double press on reset → the USB drive appears → drag `dist/maarse-<board>.uf2` over (the bootloader is never touched). ESP32-S3: hold BOOT down, press RESET, and flash with `espflash flash --erase-parts otadata --partition-table firmware/esp/partitions.csv <elf>` (or `esptool.py write_flash 0x0 dist/maarse-<board>.bin`).

## Releases (signed)

```text
python tools/release.py 0.5.0                    # builds and signs dist/release-0.5.0/
python tools/release.py --verify dist/release-0.5.0
maarse-sign verify-release --pubkey <hex> <dir>    # check a release without the script
```

The first time, the keys are made in `~/.maarse/release/` (never in git): `release.key` (Ed25519) and `release.pub`, plus `android.keystore` with its password in `android.pass`. **Back up the folder.** Without `release.key` nodes in the field cannot be updated over the air with new releases, and without the Android key the app cannot be updated over an installed version.

A release contains firmware for all four boards built with the release key (`MAARSE_RELEASE_PUBKEY`), so they only accept updates signed with it, plus the signed OTA packages (`-ota.bin` + `.manifest`), `maarse-host-<v>.exe` (with IP tunnel), `maarse-<v>.apk` signed with the Android key and `SHA256SUMS` with `SHA256SUMS.sig`. The signature is Ed25519 over `"maarse-release-v1
"` and SHA-256 of `SHA256SUMS` (`maarse-crypto/src/ota.rs`).

## License

The code is released under either MIT or Apache-2.0, at your option. The specification is released under CC-BY 4.0.
