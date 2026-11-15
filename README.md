# Må:rse – udkast 4 (v0.4)

Må:rse er et krypteret mesh-netværk til LoRa, der kan række ud i verden via TCP og internet.

## Kom i gang (Windows og Linux)

1. Installér Rust fra <https://rustup.rs>.
2. Start en node med webklient:
   - Windows: dobbeltklik på `scripts\start-host.bat`
   - Linux: `./scripts/start-host.sh`
3. Browseren åbner `http://127.0.0.1:8080` og forbinder automatisk.

Første gang tager opstarten et par minutter, fordi programmet bygges. Derefter starter det med det samme.

**To computere:** start den ene som ovenfor. Start den anden med `--peer <første computers IP>:4242`.

**Backbone med DHT:** en host med offentlig adresse startes med `--public-addr <vært:port>` (og `--dht <en anden backbone-host>`). Den udgiver de adresser, den kan nå, i DHT'en. Andre hosts slår ukendte adresser op og forbinder direkte til den rigtige gateway. DHT'en findes kun over TCP, aldrig over LoRa.

**Firmware til en radio:**

```text
Windows:  .\scripts\build-firmware.ps1 <board>  [-Flash]
Linux:    ./scripts/build-firmware.sh <board>  [flash]

board:    wismesh-tag | xiao-nrf52840 | heltec-v4 | xiao-esp32s3
```

- **nRF52840** (WisMesh Tag, XIAO nRF52840): scriptet laver `dist/maarse-<board>.uf2`. Dobbeltklik på boardets reset-knap og træk filen over på USB-drevet, der dukker op.
- **ESP32-S3** (Heltec V4, XIAO ESP32-S3): første gang installerer scriptet Espressifs værktøjskæde (`espup`) og `espflash`, hvilket tager et par minutter. Scriptet laver `dist/maarse-<board>.bin` og flasher boardet direkte, hvis du tilføjer `flash`.

Forbind derefter med webklienten via **USB / seriel** (kræver Chrome eller Edge) eller **Bluetooth** (nRF og ESP32-S3).

## Hvad kan det

| Funktion | maarse-host (PC) | nRF52840 | ESP32-S3 |
| --- | --- | --- | --- |
| Krypterede direkte beskeder med "leveret" | ja | ja | ja |
| Forward secrecy for samtaler (ratchet, udkast) | ja, sessioner gemt på disk | ja, 8 sessioner i RAM | ja, 8 sessioner i RAM |
| Kanaler (offentlig, #hashtag) | ja | ja (højst 4) | ja (højst 4) |
| Billeder og filer (krypteret, med kvittering pr. bid) | ja, op til 1 MB over internettet, 8 KB over LoRa | ja, op til 8 KB (ikke testet på hardware endnu) | ja, op til 8 KB (ikke testet på hardware endnu) |
| Postkasse til modtagere, der er offline | ja (disk, 7 dage; også filer op til 1 MB) | ja (8 KB flash) | ja (8 KB flash) |
| Fjernadministration (status og radio på en anden node) | ja | ja | ja |
| Status-side (radio, routing, postkasse, trafik, DHT) | ja | ja, med RSSI/SNR, duty cycle, CAD og fejl | ja, samme |
| Radioindstillinger (MHz, båndbredde, SF, CR, dBm, duty %) | – | ja, gemmes i flash | ja, gemmes i flash |
| Batterimåling og energistyret routing | – (altid fuld) | ja, begge boards, + USB-strøm | Heltec V4 ja; XIAO ESP32-S3: ukendt (ingen deler) |
| Diagnose: trace route, naboliste, støjgulv | trace og naboer | ja | ja |
| Position (opt-in, afrundet) og telemetri i announces | ja | ja, gemmes i flash | ja, gemmes i flash |
| MQTT-bro med Home Assistant discovery | ja (`--mqtt`) | – | – |
| Visningsnavn i announces | ja (`navn.txt`) | ja, gemmes i flash (ikke afprøvet på hardware) | ja, gemmes i flash (ikke afprøvet på hardware) |
| Talebeskeder (Codec2, 1200 bit/s) | ja, i webklienten | ja, som fil (kodes i webklienten) | ja, som fil (kodes i webklienten) |
| `maarse://`-sider | serverer `<data>/sider/` og henter sider | henter sider (serverer ikke) | henter sider (serverer ikke) |
| Forbindelse | TCP-backbone + DHT, WebSocket, seriel LoRa-modem | USB og Bluetooth | USB og Bluetooth (ja, ikke afprøvet på hardware) |
| Display | – | – | Heltec V4: 3 sider, skift med PRG-knappen |

Webklienten (`maarse-client/index.html`) virker over WebSocket (maarse-host), USB/seriel og Bluetooth (Nordic UART). Den skalerer billeder automatisk ned, så de passer under grænsen: 8 KB over LoRa og 1 MB over internettet.

**Filgrænse efter rute.** Før en fil sendes, spørger klienten maarse-host om grænsen for den valgte kontakt (kommando `0x14 GetFileLimit {dest}`, svar `0x96 FileLimit {dest, max_bytes (u32), flag (u8: bit 0 = LoRa/ukendt, bit 1 = postkasse), est_airtime_s (u32)}`) og viser fx *"Billedet sendes over LoRa og skaleres ned til 8 KB (ca. 2 min. sendetid)"* eller *"Sendes over internettet (op til 1 MB)"*. Billeder skaleres ned til grænsen; andre filer over grænsen afvises med en forklaring. Firmware-noder kender ikke kommandoen, og så bruges `Info.max_file`. Hosten håndhæver også grænsen på `FileBegin`. Reglerne (`maarse-host/src/filebox.rs`):

- **TCP hele vejen** (modtageren er hørt over et TCP-link, og den og alle led på ruten er kendte backbone-hosts: direkte TCP-naboer eller DHT-kontakter; en webklient hos en host har hostens adresse): op til `--max-file` (1 MB).
- **LoRa** (hørt over et serielt modem, eller gennem et led, hosten ikke ved er en host, fx en radionode bag en anden hosts modem): 8 KB, samme som firmwaren. Sendetiden anslås ud fra EU-standardindstillingerne (SF9, 125 kHz, 10 % duty cycle), da hosten ikke kender modemets radio.
- **Ukendt** (aldrig hørt, eller linket er gået ned): modtageren regnes som offline, og filen må være op til 1 MB, fordi den lægges i filpostkassen. Uden filpostkasse gælder 8 KB.
- Er hosten i tvivl, er svaret aldrig TCP. Filer over 8 KB sendes aldrig ad en rute, der ikke er TCP: bidderne holdes tilbage, og filen ender i filpostkassen. Bidder til en modtager over TCP sendes ikke ud på et LoRa-modem.

**Filpostkasse (kun maarse-host).** Svarer modtageren ikke (som for beskeder efter 45 s, eller når overførslen går i stå), gemmer afsenderens host filen på disken i `filpostkasse/`: det forseglede tilbud og de forseglede bidder, så hosten ikke kan læse den. Afsenderen får `FileDone` med teksten *"lagt i postkasse"*. Når modtageren dukker op igen (announce eller ny rute, samme mekanisme som postkassen for beskeder), leveres filen, og afsenderen får *"leveret fra postkasse"*. Højst 1 MB pr. fil, 20 MB pr. modtager og 200 MB i alt (`--filpostkasse-modtager-mb`, `--filpostkasse-mb`; 0 slår den fra). Filer udløber efter 7 dage. Filer lægges aldrig i radionoders flash-postkasse, og en fil over 8 KB leveres kun, når modtageren nås over TCP; nås den kun over LoRa, venter filen (eller udløber). Filpostkassen ligger hos afsenderens host, så den skal være online, når modtageren kommer tilbage.

Filer på firmware-noder (`firmware/common/src/node/files.rs`, logikken i `maarse-crypto/src/files.rs`) bruger samme format som maarse-host, så de to kan sende filer til hinanden. Noden sender og modtager én fil ad gangen i hver retning (to statiske buffere på 8 KB). En modtaget fil sendes til klienten, når den er hel og tjekket; er ingen klient forbundet, bliver den liggende i RAM, til en klient forbinder (den overskrives af næste fil og forsvinder ved genstart). Filer lægges ikke i postkassen: én fil ville fylde de 8 KB flash helt.

## Fjernadgang (fx telefonen over VPN)

maarse-host kan nås fra andre maskiner, fx telefonen over en VPN hjem, når den lytter på en anden adresse end `127.0.0.1`:

```text
maarse-host --http 0.0.0.0:8080 --serial COM6 --meshcore COM9
```

Ved opstart skriver hosten en **adgangsnøgle** (gemt i `<data>/adgangsnoegle.txt`) og en færdig adresse til browseren (`http://<pc>:8080/#token=…`). Forbindelser fra PC'en selv virker uden nøgle som før; alle andre skal have nøglen med (`ws://…/ws?token=…`), ellers afvises de (HTTP 403). I Android-appen: tryk **maarse-host**, skriv adressen (fx PC'ens VPN-adresse `10.8.0.2:8080`) og nøglen; appen husker dem og forbinder selv igen. Telefonen bruger så PC'ens node: MeshCore-gateway, LoRa via modemet, TCP-backbone og historik.

Lad kun hosten lytte på et netværk, du stoler på (VPN eller hjemmenet). Nøglen beskytter adgangen, men forbindelsen selv er ikke krypteret uden VPN.

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

## Wi-Fi backbone on ESP32-S3 nodes

A Heltec V4 or XIAO ESP32-S3 can join a Wi-Fi network (for example a phone hotspot on a hilltop) and keep a TCP backbone link to a maarse-host. Everyone who can reach the node over LoRa then reaches the rest of the network: the home host, the DHT and anyone behind it.

- **Setup:** connect to the node over USB or Bluetooth, open *Radio* in the web client and fill in *Wi-Fi backbone*: network name, password and host (`host:port`, port 4242 by default). Saving stores the settings in flash and restarts the node. Leaving the password empty for the same network keeps the saved one.
- **The host side:** any maarse-host listening on its backbone port (`--listen`, default `0.0.0.0:4242`). The node connects *out*, so it works behind a hotspot's NAT, but the host must be reachable from that network: a public address, a forwarded port or a VPN.
- **How it works** (`firmware/esp/src/wifi.rs`): esp-radio's Wi-Fi driver with embassy-net (DHCP, DNS, TCP). Frames are the backbone's `length (u16 LE) | packet`; a host's region greeting is skipped. The node sends every packet over both LoRa and the link and treats packets from the link as a perfect radio link (SNR 10), like maarse-host does with TCP. The router's duplicate filter stops loops. The link reconnects every 5 s after a failure; joining is retried the same way.
- **Edge bridge:** the host's announce carries the role *repeater*, so the node hears a repeater over the link. A node with the role *relay* then becomes an edge bridge for its LoRa group automatically (see "Edge bridge").
- **Protocol:** command `0x28 SetWifi` (`enabled | ssid_len ssid | pass_len pass | peer`, local only: it is refused through `Remote`, as it carries the password), `0x29 GetWifi`, and event `0xa6 Wifi` (`enabled | state | ip (4) | ssid_len ssid | peer`; state 0 off, 1 joining, 2 online, 3 linked). The password is never sent back. Flash: `WIF1` at offset `0x340` of the settings page. nRF52840 boards and maarse-host answer `SetWifi` with an error.
- **Two modes** (*Mode* in the Wi-Fi form; bit 1 of the flags byte in `SetWifi`): *mesh node* (default) is a node of its own that peers with the host over the backbone and keeps working if the host goes away. *LoRa modem for the host* makes the node the host's antenna over Wi-Fi, like `--serial` over USB: it sends `MM`, then forwards raw LoRa packets with their SNR to the host and sends what the host asks for; it does not route or announce. The host names the link `wifi-modem <address>` and treats it as LoRa (8 KB file limit, no calls). The display shows `<ip> modem`. In both modes the node greets the host at once, so it shows up before it has anything to send.
- **Devices in the host:** `GET /api/enheder` (and *Devices* under host settings in the web client) lists every link: backbone peers, USB modems and Wi-Fi modems, with uptime, packets in and out, last heard and the last SNR from a modem.
- **One radio at a time:** the firmware contains both Bluetooth and Wi-Fi, but runs only one. With Wi-Fi enabled, Bluetooth is not started (the phone app then reaches the node through the host instead). **Hold the PRG button for 3 s** to switch between them (the node saves the choice and restarts), or use `SetWifi` over USB. The heap then only holds one driver (Bluetooth about 34 KB, Wi-Fi about 37 KB), so `coex` is not needed. `--no-default-features --features board-heltec-v4,ble` (or `,wifi`) builds a firmware with just one of them.
- **Display (Heltec V4):** a line with the links in use (`USB`, `BT` connected or `(BT)` advertising, `WiFi`), the IP address in Wi-Fi mode (or `WiFi joining...`, `no host`), and a short note after a long press.
- **RAM (no PSRAM on the Heltec V4):** the node runs in its own task and the router (about 53 KB) has its own static. Before, the main task's future held the whole node (67 KB), and the stack frame that builds it stayed reserved for the whole run; with both radio drivers built in, the stack overflowed 0.2 s after boot. Boot now uses about 45 KB of stack and leaves about 52 KB unused.
- **Safety:** Wi-Fi starts 10 s after boot, so USB is always reachable first. If starting it crashes the board twice in a row, it is skipped (state 10) until the board has run stably for 60 s. Panics are printed over USB (`PANIC: …`). The status page shows free stack and heap and the crash count (keys 87–92).
- **Tested on hardware** (Heltec V4.3, 14 Nov 2026): joins a WPA2 network, gets an address by DHCP, answers ping (6–20 ms), keeps the link to a maarse-host (`LINKED`) and joins again after the access point restarts; switching between Bluetooth and Wi-Fi with the button works both ways. Not yet tested: messages from LoRa through the Wi-Fi link, DNS names as host, and the XIAO ESP32-S3.
- **Host:** backbone links use TCP keepalive (30 s idle, then every 10 s), so a node that drops off its hotspot without closing the link is noticed and the link is closed.

## Android-app (APK)

`python tools/build_apk.py` bygger `dist/maarse.apk` direkte med Android SDK'ens værktøjer (aapt2, javac, d8, zipalign, apksigner), uden Gradle. Kræver Android SDK (build-tools 34+, platform 35+) og en JDK; APK'en signeres med debug-nøglen.

Appen (`android/`) er webklienten i en WebView plus en Bluetooth-bro i Java til nodens Nordic UART-tjeneste, fordi WebView ikke har Web Bluetooth. Webklienten ser broen (`window.MaarseAndroid`) og viser så kun Bluetooth. Appen beder om Bluetooth- og mikrofontilladelse, når de skal bruges. Installér med `adb install dist/maarse.apk` eller ved at åbne filen på telefonen (tillad ukendte kilder).

## Webklienten som app (PWA)

Webklienten kan installeres som en app, så du kan bruge en radio via Bluetooth eller USB direkte fra telefonen eller computeren, uden en PC med maarse-host. Den virker også uden net, når den først er åbnet én gang.

- **Android (Chrome):** åbn klienten over https (fx fra GitHub Pages eller en anden statisk vært) eller fra maarse-host. Tryk på menuen ⋮ → **Installér app** (eller "Føj til startskærm"), eller tryk på **Installér app** i klienten. Forbind derefter med **Bluetooth** (nRF-noder) eller **USB** (kræver USB-OTG-kabel).
- **Windows, Linux og macOS (Chrome eller Edge):** klik på installér-ikonet i adresselinjen eller på **Installér app** i klienten.
- **iPhone/iPad:** Safari har hverken Web Bluetooth eller Web Serial. Der virker kun forbindelsen via WebSocket til en maarse-host (fx på en PC eller Raspberry Pi på samme net).
- **Notifikationer:** tryk på **Slå notifikationer til** i sidepanelet. Så viser klienten en notifikation, når der kommer en direkte besked eller en kanalbesked, mens den ligger i baggrunden. Antallet af ulæste står i fanens titel.
- **Opdatering:** når der findes en ny version, viser klienten "Ny version – genindlæs".

Offline-cache, installation og notifikationer kræver, at siden hentes over http(s) (localhost tæller som sikker). Åbnet direkte som fil (`file://`) virker klienten som før, bare uden de ting.

**Selv hoste klienten:** læg filerne i `maarse-client/` (`index.html`, `manifest.webmanifest`, `sw.js`, `icon.svg`, `icon-192.png`, `icon-512.png`) på en vilkårlig statisk https-vært, fx GitHub Pages. Hæv `VERSION` i `sw.js`, når filerne ændres, så brugerne får besked om den nye version (maarse-host gør det selv ud fra indholdet). Ikonerne laves med `python tools/make_icons.py`.

## Én indbakke på flere skærme (historik på maarse-host)

En maarse-host derhjemme kan fungere som én fælles indbakke, som telefonen, den bærbare og den stationære følger samtidig. Hosten gemmer beskedhistorikken, og hver skærm, der forbinder, henter den:

- Gemmes: modtagne direkte beskeder og kanalbeskeder, egne sendte beskeder (fra alle skærme), leveringstilstand (sendt, kvitteret, i postkasse) og filers metadata (navn, type, størrelse). **Filernes indhold gemmes ikke** – en fil vises kun på de skærme, der var forbundet, da den kom.
- Sender du fra én skærm, vises beskeden med det samme på de andre. Læser du en samtale på én skærm, forsvinder ulæst-tælleren også på de andre.
- Klienten henter de seneste 200 beskeder ved forbindelse og ældre, når du scroller op i en samtale.
- `--history-days N` (standard 90) bestemmer, hvor længe beskeder gemmes; ældre slettes ved opstart og hver time. `--no-history` slår historikken helt fra.
- Firmware-noder (Bluetooth/USB) har ingen historik; klienten virker der som før.

**Privatliv:** beskederne er ende-til-ende-krypteret på radio og backbone, men hosten dekrypterer dem, og historikken ligger **ukrypteret** i `<data>/historik/log.txt`. Alle med adgang til hostens disk kan læse den. Brug `--no-history` på en delt maskine, og slet mappen for at fjerne historikken. Formatet og protokollen (kommandoer 0x30–0x32, hændelser 0xB0–0xB3) er beskrevet i `maarse-host/src/history.rs`.

## Indhold

| Del | Hvad |
| --- | --- |
| `maarse-core` | pakkeformat, routing-motor, rutetabel, duty cycle, companion-protokol og indholdstyper. `no_std`, uden heap. |
| `maarse-crypto` | identiteter, announces, kryptering af direkte beskeder (med forward secrecy-sessioner, se `ratchet.rs`), kanaler og fil-bidder. `no_std`. |
| `maarse-host` | gateway og testnode: én fil, ingen installation. |
| `maarse-client` | webklient i én HTML-fil. |
| `maarse-sim` | simulator med radiomodel, kollisioner, duty cycle, motorvejsscenarie og flood-tilstand til sammenligning. |
| `maarse-uf2` | laver UF2-filer af firmwaren (ingen eksterne værktøjer). |
| `maarse-fuzz` | portabel fuzzing af alle dekodere, der læser bytes udefra. |
| `firmware/common` | fælles firmware-logik: node, radio-løkke, statistik og flash-lager (indstillinger og postkasse). |
| `firmware/nrf` | nRF52840-firmware: WisMesh Tag og XIAO nRF52840 + Wio-SX1262, med Bluetooth. |
| `firmware/esp` | ESP32-S3-firmware: Heltec V4 (med OLED) og XIAO ESP32-S3 + Wio-SX1262. |
| `scripts/` | start af host og bygning af firmware (Windows og Linux). |
| `tools/e2e_test.py` | ende-til-ende-test af tre maarse-host-noder. |
| `tools/dht_test.py` | test af backbone-DHT'en med fem hosts. |

## Routing

- Ruter findes med et flood gennem repeatere. Første besked rejser med i forespørgslen og bliver kvitteret med svaret.
- Repeatere med dårligere SNR sender først, og repeatere med lavt batteri venter længere.
- Flood-dæmpning (en repeater tier, når en nær nabo med god SNR har sendt det samme), kvittering pr. hop med gensendelse, og kvittering fra ende til ende.
- **Reparation af ruter:** uden kvittering inden for 20 s glemmes ruten, og beskeden sendes igen med en ny ruteforespørgsel (op til 2 gange). Ubesvarede ruteforespørgsler gentages efter 45 s (svaret kan være længe undervejs i et stort net), og et sent svar på en ældre forespørgsel gælder stadig.
- **Mobile noder** (`mobile = true`) stoler ikke på ruter, der er ældre end 30 s.
- Roller: `Client`, `Repeater` og `Relay` (skov-tilstand, højst 4 hop).

### Edge bridge (relays at the edge of repeater coverage)

A relay that can reach a repeater (say, someone who climbs a hill) is often the group's only way into the repeater network. Normal flood suppression silenced it: it heard the group's own copies of a flood with good SNR, concluded that "the area is covered" and stayed quiet, so about half of all routes were lost.

- **Role field in announces:** app-data field `0x06 | 2 | role | flags` (`maarse-core/src/appdata.rs`). Flag bit 0 (`ROLE_HEARS_REPEATER`) means "I hear a repeater or anchor directly". Older nodes skip the field.
- **Neighbor table:** each neighbor stores its role and its own report, when it last forwarded a flood, and when a repeater was heard echoing it ("covered"). Repeaters and anchors are kept when the table is full.
- **Rule** (`Router::is_edge_bridge`): a relay that hears a repeater and has a recent neighbor that does not (by its own report, or, for older firmware, a forwarding neighbor no repeater has echoed) ignores duplicate floods from non-repeaters. A copy from a repeater still silences it, so it does not add traffic where the repeater already covers the group. Observations expire after 5 minutes (`EDGE_WINDOW_MS`).
- **Simulation** (`maarse-sim --hill`, 10 seeds each): with 10–40 people on 3–8 km and one person on a hill that reaches a repeater 12 km away, delivery to the base rises from 55–82 % to 96–100 %, with less airtime. Where repeaters stand inside the forest, the rule costs 0–3 points of acks and up to about 30 % more airtime with partial coverage; with dense coverage it is close to unchanged. `--suppress legacy` runs the old behaviour for comparison. The simulator does not send announces; it hands each node what a neighbor's announce would say.

### Store-carry-forward ("carry") and the delivered notice

With `carry_copies > 0` (off by default; not enabled in host or firmware yet, `maarse-sim --carry N` to try it), a direct message the router gives up on is carried instead of dropped: the node keeps it with N tokens and hands half of them to new encounters ("binary spray and wait"), and to the destination when it is heard. Copies live 30 minutes. Details are in `maarse-core/src/router.rs` ("Carry").

- **Carry packet** (type 11): a one-element path naming the receiving neighbour, payload `tokens (1) | ttl left in s (u16 BE) | from (PathId, u16 BE) | message`. The message stays end-to-end encrypted.
- **Delivered notice:** when the destination gets a carried message (also a repeated copy), it sends a type 11 packet *without* a path, addressed to `ack_token(message id)` (the same 16-byte token as an ordinary acknowledgement), with no payload: 18 bytes on the air. Nodes without the notice drop Carry packets without a path, so it is backward tolerant.
- A carrier that hears the notice deletes its copy, stops handing out copies, passes the notice on once and remembers it for the rest of the copy's lifetime. It repeats it when it meets a neighbour it gave a copy to (or got its copy from), and answers a handoff of a delivered message with the notice instead of storing it. The notice thus walks back along the spray tree to the sender.
- **Sender:** the router reports `Action::CarryAcked { msg_id }`; the host and firmware send companion event `0xa7 CarryAcked` (`msg_id` u64 LE), and the web client shows "delivered (carried)" (history state 4). A sender that gets an ordinary acknowledgement for a message it still carries (the outbox retried it near a repeater) also stops and tells its carriers.
- **Simulation** (10 seeds, 18 people in relay mode, 60 direct messages in 30 min): forest 8 km, delivered 60 → 76 % (4 copies) / 78 % (8 copies) at +9 % / +13 % airtime (+24 % / +29 % without the notice), and acks now follow delivery (74 / 77 % instead of 58 %). Forest 10 km: 30 → 44 / 45 % at +11 % / +15 %. E45, low repeaters every 15 km: 85 → 94 % at +27 % airtime (+150 % without the notice). E45 with a 100 km gap: no gain, +19–36 % airtime.

## Spam-beskyttelse (arbejdsbevis)

Announces og ruteforespørgsler bærer et lille arbejdsbevis (et "stempel", inspireret af Reticulum 1.6), så ingen kan oversvømme nettet billigt. Stemplet er en nonce, så `SHA-256("maarse/stamp" | version | type | destination | binding | nonce)` har nok foranstillede nul-bits. At tjekke koster én hash.

| Pakke | Binding | Sværhedsgrad (v1) | nRF52840 (skøn) | PC (målt) |
| --- | --- | --- | --- | --- |
| Announce | identitet (destination) + tidsstempel | 12 bit, 4.096 forsøg i snit | ca. 0,3 s | ca. 0,2 ms |
| Ruteforespørgsel | mål + svar-token | 8 bit, 256 forsøg i snit | ca. 20 ms | ca. 0,01 ms |

- Repeatere og klienter smider pakker uden gyldigt stempel væk: de behandles ikke og sendes ikke videre. De tælles i statistikken som "Uden arbejdsbevis" (nøgle 77).
- Firmwaren regner announce-stemplet ud i bidder på 256 forsøg (ca. 20 ms) og giver executoren fri imellem. Ruteforespørgslens stempel er så billigt, at routeren regner det ud med det samme.
- Sværhedsgraden ligger fast pr. pakketype og stempel-version, så den kan hæves med en ny version. Detaljer i `maarse-core/src/stamp.rs`.

**Ændring af pakkeformatet (v0.4):** announces og ruteforespørgsler har nu 5 bytes ekstra sidst i payloaden: stempel-version (1) og nonce (4, big-endian). Største app-data i et announce er derfor 5 bytes mindre (99 bytes). Ældre noder uden stempel bliver ikke hørt af nye noder, og nye announces afvises af gamle noder (længden passer ikke). Companion-rammerne er uændrede; appen ser announces uden stempel.

## Regioner

Kanalbeskeder kan holdes inden for et område. En region er et navn med små bogstaver, fx `dk` eller `nordjylland`.

- **Kanal med region:** deltag i `navn@region`, fx `offentlig@dk` eller `#tur@nordjylland`. Det, du sender til kanalen, mærkes med regionen. `#tur` uden `@` er global som før. Kanalens nøgle afhænger kun af navnet, så `#tur` og `#tur@nordjylland` er samme samtale.
- **Noder med regioner:** hver node har en liste med op til 8 regioner, den videresender for. Hierarki klares med konfiguration: en repeater i Aalborg har `dk, nordjylland`, en i Aarhus har `dk, østjylland`. En besked i `#tur@nordjylland` når så hele Nordjylland, men stopper ved Aarhus-repeateren; en besked i `offentlig@dk` går gennem dem begge.
- **Uden regioner** videresender en node alt, så nettet virker, før nogen har sat regioner op. Med **streng** (`strict`) videresender en node uden regioner kun beskeder uden region.
- Det er kun **videresendelse**, der filtreres. Beskeder til din egen node leveres altid, og direkte beskeder (der følger ruter) er ikke berørt.
- **Opsætning:** i webklienten under *Radio* (kommasepareret liste + "Streng"), eller for maarse-host med `--region dk --region nordjylland` og `--strict-region`. Firmware gemmer listen i flash, maarse-host i `regions.txt`.
- **Backbone:** gateways udveksler deres regioner, når en TCP-forbindelse kommer op, og sender kun beskeder med region til peers, der betjener regionen. En Nordjylland-gateway og en anden Nordjylland-gateway deler altså `#tur@nordjylland` over internettet, mens en gateway i København kun får `dk`-beskeder.
- **På luften:** bit 0 i flag-byten betyder "region følger", og regionen er 2 bytes (16-bit hash af navnet) efter stien. Pakker uden region er uændrede; ældre firmware videresender ikke pakker med region.

## Backbone-DHT

- Kademlia-agtig, med XOR-afstand på 16-byte-adresser, K = 8 og 3 forespørgsler pr. runde.
- En post siger: "adresse X kan nås via gateway G på vært:port". Posten indeholder X's signerede announce, så ingen kan opfinde adresser.
- Gateways udgiver hver 30. minut, og posterne udløber efter 2 timer. Kender DHT'en flere gateways til samme adresse, vinder den med færrest hop.
- Når en host ser en ruteforespørgsel til en ukendt adresse, slår den adressen op. Det sker også, når en klient skriver til en ukendt adresse; så venter beskeden på svaret.
- Når adressen er fundet, forbinder hosten direkte til gatewayen, og den almindelige routing klarer resten.
- DHT-trafikken bruger samme TCP-port som backbone. Den kendes på de første to bytes (`D7 D7`).

## Batteri og energistyret routing

Firmwaren måler batteriet hvert 60. sekund (8 målinger i gennemsnit) og sætter routerens energistatus ud fra spændingen (`maarse-core/src/battery.rs`):

| Spænding | Status | Videresender |
| --- | --- | --- |
| ≥ 3,90 V eller USB-strøm | fuld | alt |
| 3,60–3,90 V | ok | alt, lidt senere i floods |
| 3,40–3,60 V | lav | kun direkte beskeder og ruteforespørgsler |
| < 3,40 V | kritisk | intet |

Tærsklerne følger Li-ion-kurven (RAK's OCV-tabel i Meshtastic): 3,9 V ≈ 75 %, 3,6 V er "knæet" (≈ 10–15 %), under 3,4 V er cellen næsten tom. For at gå et trin op skal spændingen være 50 mV over tærsklen (hysterese). Spænding, procent, USB-strøm og energistatus står på status-siden (nøgle 49, 85, 82, 81).

| Board | Måling | Kilde |
| --- | --- | --- |
| XIAO nRF52840 | P0.31 (AIN7), deler 1 MΩ/510 kΩ (× 2,961), tændt med P0.14 lav; 3,0 V reference, 12 bit | Meshtastic `seeed_xiao_nrf52840_kit`, MeshCore `XiaoNrf52Board` |
| WisMesh Tag (RAK4630) | P0.05 (AIN3), deler 1 MΩ/1,5 MΩ (× 1,73); 3,0 V reference, 12 bit | Meshtastic `rak_wismeshtag` |
| Heltec V4 | GPIO1 (ADC1), deler tændt med GPIO37 høj, × 5,12 (4,9 × 1,045), 2,5 dB dæmpning | Meshtastic `heltec_v4` |
| XIAO ESP32-S3 + Wio | ingen batterideler: "ukendt" | – |

USB-strøm ses på nRF52840 i POWER-registeret (VBUSDETECT); ESP32-boardene rapporterer "ukendt".

## Diagnose

Webklienten har en **Diagnose**-side (knappen ved Status/Radio):

- **Trace route** (kommando `0x20`, svar `0xA0`): en `Probe`-pakke følger den kendte rute (eller finder en først), og hver repeater lægger den SNR, den hørte pakken med, til. Målet svarer ad den omvendte vej, og repeaterne lægger igen deres SNR til. Resultatet er SNR for hvert spring begge veje. Pakke-ID'et regnes kun over den faste del, så implicitte kvitteringer pr. hop virker som for andre pakker. Ældre noder sender pakken uændret videre. Format i `maarse-core/src/diag.rs`.
- **Naboliste** (`0x21`, svar `0xA1`): noder hørt direkte (announces uden hop og forrige hop i kildestier og ruteforespørgsler) med SNR, RSSI, tid og antal pakker.
- **Støjgulv** (status-nøgle 80): lora-phy giver ikke den øjeblikkelige RSSI, så støjgulvet anslås ud fra modtagne pakker: N = RSSI − 10·log10(1 + 10^(SNR/10)), glidende gennemsnit.

## Position og telemetri

Position deles **ikke** som standard. Under Diagnose kan man vælge præcision (slukket / ca. 10 km / ca. 1 km / præcis) og telemetri (batteri, oppetid, energistatus); kommando `0x22`/`0x23`, svar `0xA2`. Noden gemmer positionen, men sender den afrundet til midten af en gittercelle (0,1° eller 0,01°); kun "præcis" sender den nøjagtige position (klienten spørger først). Felterne ligger i announcets app-data efter FS-feltet (`0x03` position, `0x04` telemetri, type | længde | data, i alt højst 41 af 99 bytes), så ældre noder ignorerer dem. Modtagne positioner og telemetri vises som en liste (hændelse `0xA3`), uden kort udefra. Temperatur er med i formatet, men nRF52840's TEMP-enhed ejes af Bluetooth (MPSL), så den sendes ikke endnu.

## Sikkerhedskopi og gendannelsesfrase

Under **Sikkerhedskopi** i webklienten kan man lave en ny identitet ud fra en **gendannelsesfrase på 24 ord** eller gendanne en identitet fra en frase. Frasen laves og vises kun i browseren (aldrig i localStorage), og brugeren skal bekræfte tre tilfældige ord, før den installeres på noden.

**Der er ingen bagdør: den, der har frasen, ER dig.** Mister du den, kan ingen gendanne identiteten.

Afledning (`maarse-crypto/src/recovery.rs`, samme i webklienten):

```text
32 bytes tilfældig entropi -> 24 ord (BIP39's engelske ordliste, 8 bit tjeksum)
seed   = PBKDF2-HMAC-SHA512(frase, "mnemonic" + adgangsord, 2048)   # præcis BIP39; adgangsord valgfrit
master = HKDF-SHA256(salt "maarse/master/v1", seed, info "") -> 64 bytes
identitet = Identity::from_secret_bytes(master)   # Ed25519-frø | X25519-nøgle
```

Testvektor: "abandon × 23 art" uden adgangsord giver adressen `6ef182b74279e05122977ced600ae3e6`.

Klienten installerer identiteten med `SetIdentity` (`0x34`: flag (1 = bekræftet) | hemmelighed (64)), svar `IdentityChanged` (`0xB4`: adresse | offentlig identitet). Kommandoen virker kun lokalt (USB, Bluetooth, maarse-hosts WebSocket på 127.0.0.1) og afvises via `Remote`. Noden gemmer nøglen (firmware: identitetsfeltet i flash; maarse-host: `identity.key` skrives atomisk, den gamle gemmes som `identity.old`), glemmer ruter og forward secrecy-sessioner og sender et nyt announce. `0x35`–`0x37` og `0xB5`–`0xB7` er holdt fri til tilknyttede enheder.

Noden kan **ikke** udlevere sin hemmelige nøgle (ingen `GetIdentitySecret`): en identitet, der er oprettet tilfældigt (før frasen fandtes), kan derfor ikke sikkerhedskopieres. Vejen videre er at oprette en ny identitet med frase. Adressen skifter, og kontakter skal tilføje dig og bekræfte sikkerhedsnummeret igen.

**Overgangsregel:** i dag ER nodens identitet frasens identitet, så samme frase giver samme adresse og sikkerhedsnummer. Senere, med *tilknyttede enheder*, bliver frasen din master-identitet, og hver enhed får sin egen nøgle, som master underskriver (samme afledning, så nutidens fraser stadig gælder). Indtil da: **brug kun identiteten på én enhed ad gangen.** To noder med samme identitet forstyrrer ruter og forward secrecy-sessioner, så beskeder kan gå tabt.

**Krypteret eksport (`.maarse-backup`)** indeholder klientens tilstand: kaldenavne, kontakternes nøgler og bekræftelser, private kanaler (med nøgler), regioner, rolle og deling. Identiteten er ikke med (den gendannes med frasen). Filen er JSON: `{format: "maarse-backup", v: 1, kdf: "PBKDF2-SHA256", iter: 600000, salt, cipher: "AES-256-GCM", iv, ct}` (base64). Hovedet er AAD, så parametrene ikke kan ændres ubemærket. Adgangskoden skal have mindst 12 tegn. Vi bruger PBKDF2 (600.000 runder, OWASP's anbefaling for PBKDF2-HMAC-SHA256) i WebCrypto i stedet for Argon2id, fordi Argon2id kræver WASM eller en langsom JS-implementering på flere hundrede linjer. Er filen lille nok, vises den også som QR-kode. Ved import flettes tilstanden ind, og private kanaler, regioner, deling og rolle sendes til noden igen.

## MQTT og Home Assistant

```
maarse-host --mqtt 127.0.0.1:1883 [--mqtt-prefix maarse] [--mqtt-user navn --mqtt-pass kode]
```

Hosten sender modtaget telemetri, positioner og kanalbeskeder som JSON (QoS 0, med en lille indbygget MQTT 3.1.1-klient uden ekstra afhængigheder):

| Emne | Indhold |
| --- | --- |
| `maarse/status` | `online`/`offline` (will, bevares) |
| `maarse/node/<adresse>/telemetry` | `{"battery_mv":3870,"battery_v":3.870,"battery_percent":70,"ext_power":false,"energy":2,"uptime_s":61,...}` |
| `maarse/node/<adresse>/position` | `{"lat":56.155,"lon":10.215,"precision":"1km",...}` |
| `maarse/channel/<kanal>` | `{"channel":"#aarhus","from":"…","hops":1,"text":"…"}` |

Første gang en node sender telemetri, udgives Home Assistant discovery (`homeassistant/sensor/maarse_<kort adresse>/<sensor>/config`) for batterispænding, batteri %, energistatus, oppetid, temperatur og hop, samlet som én enhed pr. node.

## Visningsnavn

Under **Diagnose → Dit navn** i webklienten kan man vælge et navn på højst 24 bytes (kommando `0x24 SetName`, `0x25 GetName`, svar `0xa4 Name`). Navnet sendes i announcets app-data som felt `0x05 | længde | UTF-8` (`maarse-core/src/appdata.rs`), så ældre noder springer det over. maarse-host gemmer det i `<data>/navn.txt`, firmwaren i flash.

Andres navne kommer med i `NodeInfo` (`0xa3`) og vises i webklienten med `~` foran, fx `~Dennis`. Et kaldenavn, man selv har givet kontakten, vinder. Styretegn fjernes, og navnet afkortes uden at skære et tegn over. MeshCore-broen viser navnet som `[Må Dennis]`, med klammer fjernet fra navnet.

**Et navn beviser ikke noget.** Alle kan kalde sig hvad som helst. Brug sikkerhedsnummeret, når det er vigtigt, hvem man skriver med.

## Talebeskeder

Tryk på 🎤 i en direkte samtale for at optage og igen for at sende (højst 20 sekunder). Webklienten nedsampler til 8 kHz og koder med **Codec2 ved 1200 bit/s**: 10 sekunder fylder ca. 1,5 KB og kan sendes over LoRa som en almindelig fil (MIME `audio/x-codec2`). Modtageren afkoder og viser en afspiller.

Codec2 kommer fra `maarse-voice`, en ren Rust-implementering (`codec2`-crate'en) kompileret til WebAssembly og lagt ind i `index.html` (ca. 96 KB base64). Byg den igen med `python tools/build_voice.py`, hvis `maarse-voice` ændres. Format: `"C2" | tilstand (5 = 1200 bit/s) | antal rammer (u16) | rammer` (6 bytes pr. 40 ms ved 1200 bit/s).

Host og firmware kender ikke formatet; de sender bare filen.

## Opkald (Opus over TCP)

I en direkte samtale via maarse-host viser webklienten **Ring op**. Lyden kodes med WebCodecs (Opus, 16 kbit/s, 20 ms-rammer, tre pr. besked) og sendes krypteret som indholdstype `0x06` (kommando `0x27 SendCall`, hændelse `0xa5 Call`). Hosten sender kun opkald, når modtageren nås over TCP hele vejen; ellers svarer den `NOT_OVER_TCP` (fejlkode 9), og opkald over LoRa er derfor ikke muligt. Firmware-noder afviser kommandoen. Indholdet: type (1 ring op, 2 svar, 3 læg på, 4 lyd) | opkalds-ID (u32) | data; lyd er løbenummer (u16) og rammer med længde foran. Kræver Chrome eller Edge (WebCodecs).

## IP-tunnel

Hver node har en IPv6-adresse udledt af sin Må:rse-adresse (`fd` + de første 15 bytes); maarse-host skriver den ved opstart (`IP: fd…`). Detaljer i `maarse-host/src/tunnel.rs`.

```text
cargo build --release -p maarse-host --features tun
maarse-host --tun                         # Linux (root); /dev/net/tun
maarse-host --wintun C:\sti\wintun.dll     # Windows (administrator); wintun.dll fra wintun.net
maarse-host ... --tun-lora                # tillad også tunnel over LoRa (ellers kun TCP)
```

Optimeret til LoRa:

- **Session:** en tilfældig nøgle sendes én gang krypteret (indholdstype `0x07`); derefter går IP-trafikken som `Datagram`-pakker (pakketype 10) med 28 bytes overhead og uden kvittering fra ende til ende. Nøglen skiftes hvert 5. minut, og gentagne pakker afvises.
- **Header-komprimering:** IPv6-headeren (40 bytes) bliver 2-7 bytes, UDP-headeren 6.
- **MSS-clamping:** TCP-SYN får MSS 142, så et segment passer i én LoRa-pakke.
- **Opdeling og NACK:** større pakker deles i stykker; mangler der stykker efter 2 s, beder modtageren om netop dem.

En host uden adapter svarer selv på ping til sin adresse. Kun pakker fra afsenderens egen adresse til vores slippes igennem.

Målt 14. november 2026 (Windows, Wintun; Heltec V4 ↔ XIAO nRF52840 på samme bord, SF9/125 kHz): `ping -6` over LoRa ca. **1,45 s** tur-retur (første 4,2 s inkl. rute og nøgle), 300 bytes (2 stykker hver vej) ca. 5,5 s. Over TCP: 3 ms. Ved SF7/250 kHz hørte Heltec'en ikke XIAO'en (kun den anden vej), så den hurtige indstilling er ikke målt endnu. Brug mosh frem for SSH over LoRa; web og filoverførsel er ikke realistisk.

## `maarse://`-sider

En maarse-host kan dele små sider, ligesom NomadNet. Læg filer i `<data>/sider/`:

```text
sider/index.mu          maarse://<adresse>/
sider/om.mu             maarse://<adresse>/om
sider/vejr/index.mu     maarse://<adresse>/vejr
```

Webklienten har en **Sider**-knap med adresselinje, links, tilbage-knap og cache (de seneste 30 sider gemmes i browseren og kan læses uden forbindelse).

Sådan virker det (`maarse-core/src/content.rs`, `maarse-host/src/node/pages.rs`):

- Klienten sender `0x26 RequestPage {req, dest, path}`. Noden sender en krypteret direkte besked med indholdstype `0x05` og stien. Stier må kun indeholde `a-z 0-9 - _ . /`, højst 60 bytes, uden `..` og tomme led, så de ikke kan pege uden for mappen.
- Hosten svarer med en almindelig fil: MIME `text/x-maarse-page;deflate`, stien som navn og markup komprimeret med rå deflate. Siden bruger derfor samme kvittering pr. bid og samme grænse som filer (8 KB komprimeret over LoRa, 1 MB over TCP). En side over grænsen erstattes af en kort fejlside.
- Findes siden ikke, svares med en "Ikke fundet"-side. Samme node får højst 5 sider pr. 30 sekunder.
- Klienten viser kun filer som sider, når den selv har bedt om netop den side fra netop den node. Andre filer med samme MIME vises som almindelige filer.
- Firmware-noder kan hente sider (klienten er forbundet over USB eller Bluetooth), men serverer ikke selv.

Markup (`.mu`, UTF-8) er bevidst ikke HTML, så en side fra en fremmed ikke kan køre kode i klienten:

```text
# Overskrift (også ## og ###)
Almindelig tekst. **fed** og `kode`.
- punkt
> citat
---
[Link til en anden side her](/om)
[Link til en anden node](maarse://0123456789abcdef0123456789abcdef/)
[Link ud af nettet](https://example.org)   vises som tekst, åbnes ikke
```

Kodeblokke skrives mellem linjer med tre backticks.

## Gateway til MeshCore

`maarse-host` kan bygge bro til et eksisterende [MeshCore](https://github.com/meshcore-dev/MeshCore)-net, så Må:rse- og MeshCore-brugere kan skrive sammen. Hosten genimplementerer ikke MeshCores kryptering. Den styrer i stedet en almindelig MeshCore-node over USB med samme companion-protokol, som MeshCore-apps bruger.

Opsætning:

1. Flash en MeshCore-node (fx XIAO nRF52840, Heltec V3 eller RAK4631) med den officielle **Companion Radio (USB)**-firmware (`companion_radio_usb`) fra MeshCores web-flasher. BLE-varianten virker ikke, for hosten taler over seriel.
2. Sæt radioindstillingerne (frekvens, SF, BW) som det lokale MeshCore-net, fx med MeshCore-appen, og giv noden et navn som `Må:rse-gateway`. Luk så appen, for kun én kan bruge porten ad gangen.
3. Start hosten:

```
maarse-host --meshcore COM5
maarse-host --meshcore /dev/ttyACM0 --meshcore-bridge offentlig=public --meshcore-bridge #aarhus=#aarhus
```

- `--meshcore <port>`: porten med MeshCore-noden.
- `--meshcore-baud <hastighed>`: standard 115200.
- `--meshcore-bridge <maarse-kanal>=<meshcore-kanal>`: kobler to kanaler sammen (kan gentages). `public` er MeshCores offentlige kanal, `#navn` en hashtagkanal. Uden `=` bruges samme navn på begge sider. Står valget der ikke, kobles `offentlig=public`.

Sådan virker broen:

- Beskeder får afsenderen foran: `[MC Alice] hej` på Må:rse og `[Må 1a2b3c4d] hej` i MeshCore (Må:rse-afsenderen vises med de første 8 hex-tegn af adressen). I MeshCore står gateway-nodens navn også foran, fordi MeshCore altid sætter afsenderens navn på kanalbeskeder.
- Mangler en hashtagkanal på MeshCore-noden, oprettes den på en ledig kanalplads med nøglen SHA-256("#navn")[0..16], som MeshCore-apps bruger. Optagne pladser røres ikke.
- Løkker undgås: tekst, der allerede er brobygget (`[MC …`/`[Må …`), sendes ikke videre, og samme tekst i samme kanal brobygges højst én gang på 5 minutter. Så kan to gateways på samme net ikke sende en besked i ring.
- Direkte beskeder: send en Må:rse-DM til gatewayens adresse med `@Navn tekst`, så går den til MeshCore-kontakten `Navn`. Kontakten skal være kendt af MeshCore-noden, dvs. den har hørt dens advert. Svar går tilbage til den seneste Må:rse-afsender. En MeshCore-bruger kan også skrive `@<Må:rse-adresse> tekst` til gateway-noden.

Begrænsninger:

- Beskeder, der skrives i webklienten på selve gateway-hosten, brobygges ikke, fordi noden ikke sender ens egne kanalbeskeder tilbage til klienter. Skriv fra en anden Må:rse-node.
- Kun offentlige kanaler og hashtagkanaler. Private kanaler, filer, lokation og MeshCores rum-servere og repeater-administration understøttes ikke.
- MeshCore-tekst er højst 160 bytes inklusive gateway-navnet, så lange Må:rse-beskeder afkortes.
- Kvitteringer krydser ikke broen. En DM, der er nået frem til gatewayen, siger ikke noget om, om den er nået frem på den anden side.
- Afprøvet på hardware 14. november 2026: Heltec V4.3 med MeshCore `companion_radio_usb` (firmware 15. okt. 2026), kanalbeskeder begge veje mellem `offentlig` og `public`, også med visningsnavn. Direkte beskeder og hashtagkanaler er kun testet mod rammer udledt af MeshCores kildekode.

## Test

```sh
cargo test --workspace                  # 180 tests
cargo build --release -p maarse-host
python tools/e2e_test.py target/release/maarse-host
python tools/dht_test.py target/release/maarse-host
node tools/safety_number_test.mjs        # sikkerhedsnummer i JS = Rust
node tools/recovery_phrase_test.mjs      # gendannelsesfrase og sikkerhedskopi i JS = Rust
node tools/client_syntax_test.mjs        # webklientens JavaScript kan parses
node tools/client_browser_test.mjs target/release/maarse-host   # webklienten i Edge/Chrome: indlæsning, forbindelse, sider, tale
cargo run --release -p maarse-sim -- --help
```

Ende-til-ende-testen starter tre noder (A, B og en postkasse P) og afprøver:
- beskeder begge veje,
- en kanalbesked,
- en besked i en privat kanal (tilfældig nøgle), som B stadig kender efter genstart,
- en fil på 20 KB, der bliver verificeret,
- en for stor fil, der bliver afvist,
- at B går offline, beskeden lægges i postkassen, og B henter den, når den kommer online igen,
- at en fil på 20 KB til B, mens B er offline, lægges i A's filpostkasse og leveres og verificeres, når B kommer tilbage (og at grænsen så er 1 MB over TCP),
- at en fil på 20 KB afvises, når ruten må regnes som LoRa (8 KB),
- fjernadministration: en administrator får adgang, og en anden node bliver afvist,
- at en fremmed hjemmeside ikke kan forbinde.
- historik med flere skærme på samme host: en besked sendt fra skærm 1 vises live på skærm 2, en ny skærm 3 henter historikken (i sider, med kvittering), og "læst" sendes til alle skærme.

DHT-testen starter tre backbone-hosts (B1, B2 og B3) uden pakkeforbindelse til hinanden, kun med B1 som DHT-startpunkt, samt A på B2 og C på B3. A skriver til C, som A aldrig har hørt, og beskeden kommer frem med kvittering. Svaret kommer også tilbage.

Webklienten er også testet i en headless Chrome: tekst, billede og kanal mellem to noder, status- og radiosiden, og mobillayout. Sikkerhedsnumre (ens på begge sider, markér som bekræftet) og private kanaler (opret, del invitation, deltag, send) er testet i en headless Edge.

### Fuzzing

Alle dekodere, der læser bytes udefra, fuzzes med en lille, portabel motor i `maarse-fuzz` (deterministisk PRNG og mutator, ingen eksterne værktøjer; virker også på Windows/MSVC, hvor cargo-fuzz/libFuzzer ikke er pålidelig). Hvert mål kører halvt helt tilfældige input og halvt mutationer af gyldige rammer, og hvor det giver mening tjekkes det også, at dekodet input kan kodes og dekodes igen uændret.

Mål: `Packet::decode`, companion `Command::decode`/`Event::decode`, `FrameDecoder` (strøm klippet i tilfældige stykker), `content::parse`, `verify_announce`, `open_direct`, `open_channel`, `open_chunk`, og DHT'ens `decode_req`/`decode_resp` (inkl. validering og lagring af STORE).

```sh
cargo test -p maarse-fuzz --release                     # 100 000 input pr. mål
cargo test -p maarse-host --release fuzz_
MAARSE_FUZZ_ITERS=20000000 cargo test -p maarse-fuzz --release -- --nocapture   # længere kørsel
MAARSE_FUZZ_SEED=42 cargo test -p maarse-fuzz --release                         # andet frø
```

Ved en fejl skrives frø, iteration og input i hex, så den kan genskabes og lægges ind som regressionstest.

## Simulator

| Scenarie | Tilstand | Leveret | Kvitteret | Median | Sendetid |
| --- | --- | --- | --- | --- | --- |
| 120 noder, 30×30 km (3 frø) | Må:rse | 100 % | 98–100 % | 2,3–2,6 s | 341–363 s |
| | flood, 7 hop | 92–100 % | 73–83 % | 5,0–6,5 s | 1.703–1.789 s |
| 200 noder (40 repeatere), 60×60 km (3 frø) | Må:rse | 92–95 % | 73–93 % | 5,2–9,2 s | 889–995 s |
| | flood, 7 hop | 65–83 % | 57–70 % | 5,2–7,0 s | 2.887–3.170 s |
| Motorvej 100 km, bil i 110 km/t (3 frø) | Må:rse | 100 % | 100 % | 3,8–3,9 s | 313–318 s |
| | flood, 7 hop | 88–95 % | 52–63 % | 4,7–5,4 s | 1.573–1.643 s |

```sh
cargo run --release -p maarse-sim -- --seed 1                                    # 120 noder
cargo run --release -p maarse-sim -- --nodes 200 --repeaters 40 --area-km 60 --seed 1
cargo run --release -p maarse-sim -- --highway --seed 1
# tilføj --mode flood for sammenligningen; DEBUG_ACK=1 viser, hvor svar går tabt
```

Må:rse leverer flere beskeder end flood med en tredjedel til en femtedel af sendetiden, og håndterer bilen på motorvejen.

**Store, tynde net (v0.5).** Før leverede 200-node-nettet 78–92 % og kvitterede kun 35–53 % (snit over 24 frø: 83 % / 50 %, 1.453 s sendetid). Diagnosen (`DEBUG_ACK=1`): næsten alle beskeder rejser med ruteforespørgslen, så kvitteringen er ruteforespørgslens svar. Svaret gik tabt allerede på første hop tilbage, i 60 % af forsøgene i kollision med *andre ruteforespørgsler*. Afsenderen gentog nemlig sin forespørgsel efter 15 s, mens svaret stadig var på vej over mange hop med gensendelser; hvert nyt flood overdøvede svaret og gav anledning til endnu en gentagelse. To ændringer, begge uden ændring af pakkeformatet eller af routerens RAM-forbrug:

1. **Ruteforespørgsler gentages efter 45 s i stedet for 15 s.** Snit over 24 frø: 92 % leveret, 82 % kvitteret.
2. **Flood-dæmpning tæller kun stærke kopier.** En repeater tier nu, så snart den har hørt én kopi med god SNR (over `weak_snr_db`) fra en nabo; svage kopier kommer langvejsfra og tæller ikke. Før tav den efter to kopier uanset SNR. Det halverer floods i tætte net og dæmper dem i tynde, og kanalbeskeder når længere ud (snit over 6 frø: 65 → 74 % af noderne i det tynde net, 84 → 98 % i det tætte). At tie efter én vilkårlig kopi fik derimod floodet til at dø ud på motorvejen.

Samlet, snit over 24 frø for det tynde net: 94 % leveret, 89 % kvitteret, 901 s sendetid. Resten af tabet er hop over svage led (under 5 dB margen går 40–70 % af dem tabt i kollision). At udskyde forespørgsler over svage led, så stærke led vælges, gav ikke flere kvitteringer, gav længere ruter og ødelagde motorvejen, så det er ikke med. Det samme gælder længere ventetid før svaret, voksende pause mellem hop-gensendelser og flere hop-gensendelser (6 i stedet for 4): ingen målbar forskel.

Radiomodellen er grov (log-distance, ingen terræn). Tallene er til at sammenligne varianter.

## Testlaboratorium (maarse-lab)

Alle tests, fuzzing, simulatorscenarier og benchmarks kan køres og ses live i browseren:

```sh
cargo run --release -p maarse-lab                 # åbner http://127.0.0.1:8090
cargo run --release -p maarse-lab -- --no-browser --addr 127.0.0.1:8095
cargo run --release -p maarse-lab -- --port COM6 --port COM7   # også rigtige boards
```

- **Testoversigt:** hver suite med en kort beskrivelse af, hvad den tester: `cargo test` pr. crate (med alle testnavne), `maarse-fuzz`, `e2e_test.py`, `dht_test.py`, `radio_test.py` (kræver boards og `--port`), simulatorscenarierne (Må:rse og flood) og stempel-benchmarket. "Kør" pr. suite eller "Kør alt". Output streames live, og hver test bliver grøn eller rød med tid.
- **Simulator live:** kører `maarse-sim --trace-json -` og animerer nettet på et kort: noder (farvet efter rolle eller brugt sendetid), radioforbindelser (tydeligere ved bedre SNR), udsendelser som ringe i pakketypens farve, kollisioner, leveringer og bilen på motorvejen. Hastighed 0,25×–50×, pause og trin, og Må:rse og flood side om side med samme frø.
- **Hardware:** med `--port` vises boardenes RSSI/SNR, tx/rx, radiotrin, rolle og radioindstillinger live via companion-protokollen. Uden `--port` åbnes ingen porte. Pollingen holder pause, mens `radio_test.py` kører.
- **Historik:** de seneste kørsler gemmes i `maarse-lab-data/history.json` og vises som udvikling over tid (fx leveret % pr. scenarie og forsøg/s for arbejdsbeviset).

Simulatorens hændelsesspor (`--trace-json <fil|->`) er én JSON-linje pr. hændelse: `setup` (positioner og roller), `links` (hørbare par med SNR), `tx`, `rx` (modtagere og kollisioner), `defer` (duty cycle eller optaget kanal), `send`, `deliver`, `ack`, `move` og `end`. Formatet er beskrevet i `maarse-sim/src/trace.rs`.

## Firmware: status og hvad mangler

- **Intet er afprøvet på hardware endnu.**
- **nRF52840:** kompileret for begge boards (ca. 270 KB flash, 95 KB RAM). Bluetooth (Nordic UART) er med.
  - WisMesh Tag: app ved 0x26000 (S140 v6).
  - XIAO: app ved 0x27000 (S140 v7).
  - Flash-placering: app højst til 0x88000, mellemlager til opdatering 0x88000–0xE8FFF, OTA-metaside 0xE9000, postkasse ved 0xEA000–0xEBFFF, indstillinger ved 0xEC000 (se "Firmwareopdatering").
- **ESP32-S3:** bygget for begge boards med Espressifs Xtensa-værktøjskæde (ca. 550 KB app på Heltec V4, heraf ca. 220 KB til Bluetooth; app-partitionen er 2 MB). `--features check-esp32c6` type-tjekker stadig koden mod ESP32-C6.
  - Indstillinger og postkasse ligger i partitionen `maarse` ved 0x210000 (se `firmware/esp/partitions.csv`). Appen ligger i `ota_0`/`ota_1` (se "Firmwareopdatering").
  - Heltec V4: standard-sendestyrken er 10 dBm, fordi PA'en lægger ca. 7–10 dB oveni. PA-styringen (GPIO7/2/46) følger Meshtastic for GC1109-udgaven. Har dit board KCT8103L (nyere V4), skal styringen måske justeres.
  - Bluetooth (Nordic UART) er med: esp-radios BLE-controller med TrouBLE som host, samme GATT-tjeneste, rammer og navn (`Maarse-xxxx`) som på nRF. Controlleren bruger en heap på 72 KB (esp-alloc). Ikke afprøvet på hardware, især ikke samspillet med LoRa og USB.
- **Mangler:** filer over LoRa på firmware og segmenterede stier over 12 hop.
- **Afprøv på hardware (batteri og diagnose):** sammenlign batterispændingen på status-siden med et multimeter på hvert board (justér faktoren i `firmware/*/src/battery.rs` ved afvigelse over ca. 50 mV); træk USB ud og se USB-strøm skifte (nRF); se energistatus falde under 3,6/3,4 V (fx med et laboratorieaggregat); trace route over 2–3 repeatere og tjek, at SNR-værdierne svarer til status-sidens; naboliste efter et announce; støjgulv i et stille og et støjende miljø; position med 1 km-præcision vist afrundet hos en anden node.
- Ben-tildelingen er hentet fra Meshtastics board-filer.

## Firmwareopdatering (signeret, over USB, Bluetooth og nettet)

Så sol-repeatere i marken kan opdateres uden at blive hentet ind. **Ikke afprøvet på hardware endnu** (se tjeklisten nedenfor).

**Signering.** En udgivelse er et image og et manifest på 112 bytes: board-ID, version, størrelse, SHA-256 og en Ed25519-signatur (formatet står i `maarse-crypto/src/ota.rs`). Noden installerer kun, når signaturen passer med udgivelsesnøglen i firmwaren, board-ID passer, og versionen er nyere end den kørende (anti-rollback). Kun lokalt (USB/Bluetooth) kan en ældre version tillades med et flag; via nettet ignoreres flaget.

```sh
cargo run -p maarse-uf2 --bin maarse-sign -- keygen release.key       # én gang; skriver den offentlige nøgle
export MAARSE_RELEASE_PUBKEY=<hex>  MAARSE_FW_VERSION=0.2.0           # bygges ind i firmwaren
MAARSE_SIGN_KEY=release.key ./scripts/build-firmware.sh xiao-nrf52840  # -> dist/maarse-xiao-nrf52840-ota.bin + .manifest
```

- Uden `MAARSE_RELEASE_PUBKEY` stoler firmwaren på en **offentlig udviklingsnøgle** (`maarse-sign --dev-key`, `MAARSE_SIGN_KEY=dev`). Byggeriet advarer, og noden skriver en advarsel ved `OtaBegin`. Byg aldrig udgivelser sådan.
- `maarse-sign sign` tager en ELF, `.uf2` eller `.bin`. På nRF52840 er imaget appen fra dens startadresse (0x27000 XIAO / 0x26000 WisMesh; forkert board afvises); på ESP32-S3 et ESP-IDF app-image fra `espflash save-image` (bygge-scriptet laver `dist/maarse-<board>-app.bin`).
- Versionen er `major.minor.patch` (u32: major << 16 | minor << 8 | patch). Uden `MAARSE_FW_VERSION` bruges crate-versionen.

**Companion-kommandoer** (0x14–0x17 holdt fri til parallelt arbejde):

| Kode | Navn | Indhold |
|---|---|---|
| 0x18 | `OtaBegin` | req (u16), flag (bit 0: tillad ældre, kun lokalt), manifest (112) |
| 0x19 | `OtaChunk` | req, offset (u32), data (≤ 240 lokalt, 128 over radio; offset og længde delelige med 4 undtagen sidste bid) |
| 0x1a | `OtaEnd` | req, apply (u8): installér og genstart |
| 0x1b | `OtaStatus` | – |
| 0x9a | `OtaState` | req, tilstand (0 klar, 1 modtager, 2 verificeret, 3 installerer, 4 fejl, 5 ikke understøttet), næste offset, størrelse, version, kørende version, tekst |

Alle fire kan sendes via `Remote` (0x0c) til en node, man er administrator for; svaret kommer som `RemoteReply`. Overførslen er stop-and-wait: klienten sender næste bid, når svaret med næste offset er kommet, så duty cycle og tab klares af sig selv. Fremskridtet gemmes pr. 4 KB-blok på en metaside i flash, så en afbrudt overførsel genoptages fra første ufærdige blok – også efter genstart. SHA-256 tjekkes, når sidste bid er skrevet, og igen lige før installation. Kontroltælleren i `Remote` stiger nu altid (også flere gange pr. sekund).

**Webklienten:** knappen *Firmware* – vælg `.bin`/`.uf2` + `.manifest`, denne node eller en fjern node, fremskridt med anslået tid, *Stop*, *Genoptag* og *Hent status*. Klienten tjekker størrelse og SHA-256 mod manifestet, før den sender. En `.uf2` omsættes til samme image som `maarse-sign` (alle 256-byte-sider fra laveste til højeste, huller = 0xFF). Uden svar i 5 s (lokalt) / 60 s (radio) spørges der om status, og overførslen fortsætter derfra. maarse-host opdateres ikke selv, men videresender til radio-noder.

**nRF52840 (Adafruit UF2-bootloader).** Bootloaderens dual bank-DFU styres af dens egne indstillinger under en BLE/seriel DFU-session og kan ikke bruges til et image, appen selv har skrevet. Derfor:

```text
0x00000  MBR            0x26000/0x27000  app (højst til 0x87FFF)   0xE9000  OTA-metaside
0x01000  S140           0x88000          mellemlager (388 KB)       0xEA000  postkasse
                                                                    0xEC000  indstillinger
                                                                    0xF4000  bootloader (røres aldrig)
```

Den nye app (ca. 300 KB i dag) skrives til mellemlageret. Ved installation tjekkes vektortabellen, interrupts slås fra, og en lille rutine i RAM (`firmware/nrf/src/ota.rs`, kalder intet i flash) sletter appens sider, kopierer imaget side for side med kontrollæsning og genstarter. Det tager ca. 10 sekunder. Bootloader, SoftDevice, identitet og indstillinger røres ikke. **Risiko:** strømsvigt i de 10 sekunder efterlader en halv app; bootloaderen er intakt, så noden reddes altid med dobbelttryk på reset og en `.uf2` – men det kræver fysisk adgang. Appen er nu begrænset til 388 KB (WisMesh 392 KB).

**ESP32-S3** (`esp-bootloader-esp-idf` 0.6 har `OtaUpdater`): ny partitionstabel med `otadata` (0xD000, 8 KB; `nvs` krympet til 16 KB), `ota_0` ved 0x10000 (hvor `factory` lå), `maarse` uændret ved 0x210000 (OTA-metaside ved 0x213000) og `ota_1` ved 0x220000 – i alt 4,1 MB, så boardet skal have mindst 8 MB flash. Den nye app skrives til den partition, der ikke kører; `otadata` peger på den, og noden genstarter. Er den nye app ugyldig, starter bootloaderen den gamle. Ved opstart markeres den kørende app som gyldig. **Overgang:** noder med den gamle tabel (`factory`) skal flashes én gang over USB med den nye tabel (`build-firmware.sh <board> flash`, som også sletter `otadata`); identitet og indstillinger bevares, fordi `maarse` ikke flytter. Med den gamle tabel svarer noden "ikke understøttet".

**Sendetid over LoRa** for et image på 300 KB (2.378 bidder à 128 bytes; pr. bid en pakke på ca. 230 bytes, et svar på ca. 113 bytes og to kvitteringer; ét hop):

| Radio | Bid | Svar | Sendetid i alt | Afsenderens del | Mindst ved 10 % duty | ved 1 % |
|---|---|---|---|---|---|---|
| SF9, 125 kHz (standard) | 1,18 s | 0,65 s | 1,6 t | 0,95 t | ca. 10 t | ca. 95 t |
| SF7, 125 kHz | 0,37 s | 0,20 s | 0,5 t | 0,3 t | ca. 3 t | ca. 30 t |
| SF7, 250 kHz | 0,19 s | 0,10 s | 0,24 t | 0,15 t | ca. 1,5 t | ca. 15 t |
| SF11, 125 kHz | 4,7 s | 2,6 s | 6,2 t | 3,8 t | ca. 38 t | – |

**Skrinlagt (november 2026):** opdatering over LoRa tager for lang tid (ca. 1,6 t for nRF52840 og 3 t for Heltec V4 på SF7/250 kHz, mange timer på SF9) og bruges ikke. Opdatér over USB eller Bluetooth. Koden er bevaret.

Hvert ekstra hop gentager det hele. **Oprindelig anbefaling:** opdatér naboer direkte (ét hop) på 869,4–869,65 MHz (10 %), skift både repeater og afsender midlertidigt til "Hurtig (SF7, 250 kHz)" under opdateringen, og skift tilbage bagefter. Overførslen kan stoppes og genoptages, fx over flere nætter.

**Skal testes på hardware før brug i marken:**

1. nRF52840 (XIAO og WisMesh): USB-opdatering af en nyere version; noden genstarter i ny version med samme identitet, indstillinger og postkasse.
2. Bootloaderen starter appen efter kopien (tjek, at Adafruit-bootloaderen ikke kræver en CRC i sine indstillinger for en app, der ikke er skrevet af den selv – ellers bliver den i DFU-tilstand; reddes med en `.uf2`).
3. Afbrudt overførsel (træk USB, genstart noden) genoptages fra første ufærdige blok.
4. Afvisning: forkert board, samme/ældre version, forkert nøgle, ødelagt image (SHA-256).
5. Bluetooth-opdatering (Nordic UART, 240-byte bidder).
6. ~~Fjernopdatering over LoRa~~ (skrinlagt).
7. ESP32-S3: første USB-flash med ny partitionstabel bevarer identitet; OTA til `ota_1`, derefter `ota_0`; ugyldigt image i den anden partition får bootloaderen til at starte den gamle.
8. Bluetooth/LoRa kører videre under flash-skrivning (mellemlageret skrives, mens radioen er aktiv).

**Redning, hvis noget går galt:** nRF52840: dobbelttryk på reset → USB-drevet kommer frem → træk `dist/maarse-<board>.uf2` over (bootloaderen røres aldrig). ESP32-S3: hold BOOT nede, tryk RESET, og flash med `espflash flash --erase-parts otadata --partition-table firmware/esp/partitions.csv <elf>` (eller `esptool.py write_flash 0x0 dist/maarse-<board>.bin`).

## Udgivelser (signerede)

```text
python tools/release.py 0.5.0                    # bygger og signerer dist/release-0.5.0/
python tools/release.py --verify dist/release-0.5.0
maarse-sign verify-release --pubkey <hex> <mappe>  # tjek en udgivelse uden scriptet
```

Første gang laves nøglerne i `~/.maarse/release/` (aldrig i git): `release.key` (Ed25519) og `release.pub`, samt `android.keystore` med adgangskode i `android.pass`. **Tag backup af mappen.** Uden `release.key` kan noder i marken ikke opdateres over luften med nye udgivelser, og uden Android-nøglen kan appen ikke opdateres over en installeret version.

En udgivelse indeholder firmware til alle fire boards bygget med udgivelsesnøglen (`MAARSE_RELEASE_PUBKEY`), så de kun tager imod opdateringer signeret med den, samt de signerede OTA-pakker (`-ota.bin` + `.manifest`), `maarse-host-<v>.exe` (med IP-tunnel), `maarse-<v>.apk` signeret med Android-nøglen og `SHA256SUMS` med `SHA256SUMS.sig`. Signaturen er Ed25519 over `"maarse-release-v1
"` og SHA-256 af `SHA256SUMS` (`maarse-crypto/src/ota.rs`).

## Licens

Koden er udgivet under enten MIT eller Apache-2.0, efter eget valg. Specifikationen er udgivet under CC-BY 4.0.
