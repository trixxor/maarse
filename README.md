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

Forbind derefter med webklienten via **USB / seriel** (kræver Chrome eller Edge) eller **Bluetooth** (kun nRF).

## Hvad kan det

| Funktion | maarse-host (PC) | nRF52840 | ESP32-S3 |
| --- | --- | --- | --- |
| Krypterede direkte beskeder med "leveret" | ja | ja | ja |
| Kanaler (offentlig, #hashtag) | ja | ja (højst 4) | ja (højst 4) |
| Billeder og filer (krypteret, med kvittering pr. bid) | ja, op til 1 MB | ikke endnu | ikke endnu |
| Postkasse til modtagere, der er offline | ja (disk, 7 dage) | ja (8 KB flash) | ja (8 KB flash) |
| Fjernadministration (status og radio på en anden node) | ja | ja | ja |
| Status-side (radio, routing, postkasse, trafik, DHT) | ja | ja, med RSSI/SNR, duty cycle, CAD og fejl | ja, samme |
| Radioindstillinger (MHz, båndbredde, SF, CR, dBm, duty %) | – | ja, gemmes i flash | ja, gemmes i flash |
| Forbindelse | TCP-backbone + DHT, WebSocket, seriel LoRa-modem | USB og Bluetooth | USB |
| Display | – | – | Heltec V4: 3 sider, skift med PRG-knappen |

Webklienten (`maarse-client/index.html`) virker over WebSocket (maarse-host), USB/seriel og Bluetooth (Nordic UART). Den skalerer billeder automatisk ned, så de passer under grænsen: 8 KB over LoRa og 1 MB over maarse-host.

## Indhold

| Del | Hvad |
| --- | --- |
| `maarse-core` | pakkeformat, routing-motor, rutetabel, duty cycle, companion-protokol og indholdstyper. `no_std`, uden heap. |
| `maarse-crypto` | identiteter, announces, kryptering af direkte beskeder, kanaler og fil-bidder. `no_std`. |
| `maarse-host` | gateway og testnode: én fil, ingen installation. |
| `maarse-client` | webklient i én HTML-fil. |
| `maarse-sim` | simulator med radiomodel, kollisioner, duty cycle, motorvejsscenarie og flood-tilstand til sammenligning. |
| `maarse-uf2` | laver UF2-filer af firmwaren (ingen eksterne værktøjer). |
| `firmware/common` | fælles firmware-logik: node, radio-løkke, statistik og flash-lager (indstillinger og postkasse). |
| `firmware/nrf` | nRF52840-firmware: WisMesh Tag og XIAO nRF52840 + Wio-SX1262, med Bluetooth. |
| `firmware/esp` | ESP32-S3-firmware: Heltec V4 (med OLED) og XIAO ESP32-S3 + Wio-SX1262. |
| `scripts/` | start af host og bygning af firmware (Windows og Linux). |
| `tools/e2e_test.py` | ende-til-ende-test af tre maarse-host-noder. |
| `tools/dht_test.py` | test af backbone-DHT'en med fem hosts. |

## Routing

- Ruter findes med et flood gennem repeatere. Første besked rejser med i forespørgslen og bliver kvitteret med svaret.
- Repeatere med dårligere SNR sender først, og repeatere med lavt batteri venter længere.
- Flood-dæmpning, kvittering pr. hop med gensendelse, og kvittering fra ende til ende.
- **Reparation af ruter:** uden kvittering inden for 20 s glemmes ruten, og beskeden sendes igen med en ny ruteforespørgsel (op til 2 gange). Ubesvarede ruteforespørgsler gentages efter 15 s, og et sent svar på en ældre forespørgsel gælder stadig.
- **Mobile noder** (`mobile = true`) stoler ikke på ruter, der er ældre end 30 s.
- Roller: `Client`, `Repeater` og `Relay` (skov-tilstand, højst 4 hop).

## Backbone-DHT

- Kademlia-agtig, med XOR-afstand på 16-byte-adresser, K = 8 og 3 forespørgsler pr. runde.
- En post siger: "adresse X kan nås via gateway G på vært:port". Posten indeholder X's signerede announce, så ingen kan opfinde adresser.
- Gateways udgiver hver 30. minut, og posterne udløber efter 2 timer. Kender DHT'en flere gateways til samme adresse, vinder den med færrest hop.
- Når en host ser en ruteforespørgsel til en ukendt adresse, slår den adressen op. Det sker også, når en klient skriver til en ukendt adresse; så venter beskeden på svaret.
- Når adressen er fundet, forbinder hosten direkte til gatewayen, og den almindelige routing klarer resten.
- DHT-trafikken bruger samme TCP-port som backbone. Den kendes på de første to bytes (`D7 D7`).

## Test

```sh
cargo test --workspace                  # 66 tests
cargo build --release -p maarse-host
python tools/e2e_test.py target/release/maarse-host
python tools/dht_test.py target/release/maarse-host
node tools/safety_number_test.mjs        # sikkerhedsnummer i JS = Rust
cargo run --release -p maarse-sim -- --help
```

Ende-til-ende-testen starter tre noder (A, B og en postkasse P) og afprøver:
- beskeder begge veje,
- en kanalbesked,
- en besked i en privat kanal (tilfældig nøgle), som B stadig kender efter genstart,
- en fil på 20 KB, der bliver verificeret,
- en for stor fil, der bliver afvist,
- at B går offline, beskeden lægges i postkassen, og B henter den, når den kommer online igen,
- fjernadministration: en administrator får adgang, og en anden node bliver afvist,
- at en fremmed hjemmeside ikke kan forbinde.

DHT-testen starter tre backbone-hosts (B1, B2 og B3) uden pakkeforbindelse til hinanden, kun med B1 som DHT-startpunkt, samt A på B2 og C på B3. A skriver til C, som A aldrig har hørt, og beskeden kommer frem med kvittering. Svaret kommer også tilbage.

Webklienten er også testet i en headless Chrome: tekst, billede og kanal mellem to noder, status- og radiosiden, og mobillayout. Sikkerhedsnumre (ens på begge sider, markér som bekræftet) og private kanaler (opret, del invitation, deltag, send) er testet i en headless Edge.

## Simulator

| Scenarie | Tilstand | Leveret | Kvitteret | Median | Sendetid |
| --- | --- | --- | --- | --- | --- |
| 120 noder, 30×30 km (3 frø) | Må:rse | 97–98 % | 70–82 % | 2,6–4,2 s | 585–750 s |
| | flood, 7 hop | 92–100 % | 73–83 % | 5,0–6,5 s | 1.700–1.790 s |
| 200 noder, 60×60 km | Må:rse | 72 % | 53 % | 6,6 s | 1.021 s |
| | flood, 7 hop | 67 % | 47 % | 6,2 s | 2.919 s |
| Motorvej 100 km, bil i 110 km/t | Må:rse | 100 % | 100 % | 3,8 s | 346 s |
| | flood, 7 hop | 90 % | 60 % | 5,4 s | 1.573 s |

Må:rse leverer mindst lige så mange beskeder som flood med cirka en tredjedel af sendetiden, og håndterer bilen på motorvejen. **Svagheden** er store, tynde net: i 200-node-scenariet er mange noder kun dårligt dækket, og kvitteringerne tilbage går tabt oftere end ønsket. Det er næste ting at forbedre.

Radiomodellen er grov (log-distance, ingen terræn). Tallene er til at sammenligne varianter.

## Firmware: status og hvad mangler

- **Intet er afprøvet på hardware endnu.**
- **nRF52840:** kompileret for begge boards (ca. 270 KB flash, 95 KB RAM). Bluetooth (Nordic UART) er med.
  - WisMesh Tag: app ved 0x26000 (S140 v6).
  - XIAO: app ved 0x27000 (S140 v7).
  - Flash-placering: postkasse ved 0xEA000–0xEBFFF, indstillinger ved 0xEC000.
- **ESP32-S3:** Xtensa-værktøjskæden kunne ikke hentes i mit miljø.
  - Koden er type-tjekket og linket mod ESP32-C6 med samme esp-hal-version (`--features check-esp32c6`), så API-brugen er kontrolleret. Selve S3-bygget sker først hos dig.
  - Indstillinger og postkasse ligger i partitionen `maarse` ved 0x210000 (se `firmware/esp/partitions.csv`).
  - Heltec V4: standard-sendestyrken er 10 dBm, fordi PA'en lægger ca. 7–10 dB oveni. PA-styringen (GPIO7/2/46) følger Meshtastic for GC1109-udgaven. Har dit board KCT8103L (nyere V4), skal styringen måske justeres.
  - Bluetooth er ikke med på ESP32 endnu.
- **Mangler:** filer over LoRa på firmware, batterimåling og segmenterede stier over 12 hop.
- Ben-tildelingen er hentet fra Meshtastics board-filer.

## Licens

Koden er udgivet under enten MIT eller Apache-2.0, efter eget valg. Specifikationen er udgivet under CC-BY 4.0.
