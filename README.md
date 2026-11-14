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
| Billeder og filer (krypteret, med kvittering pr. bid) | ja, op til 1 MB | ikke endnu | ikke endnu |
| Postkasse til modtagere, der er offline | ja (disk, 7 dage) | ja (8 KB flash) | ja (8 KB flash) |
| Fjernadministration (status og radio på en anden node) | ja | ja | ja |
| Status-side (radio, routing, postkasse, trafik, DHT) | ja | ja, med RSSI/SNR, duty cycle, CAD og fejl | ja, samme |
| Radioindstillinger (MHz, båndbredde, SF, CR, dBm, duty %) | – | ja, gemmes i flash | ja, gemmes i flash |
| Forbindelse | TCP-backbone + DHT, WebSocket, seriel LoRa-modem | USB og Bluetooth | USB og Bluetooth (ja, ikke afprøvet på hardware) |
| Display | – | – | Heltec V4: 3 sider, skift med PRG-knappen |

Webklienten (`maarse-client/index.html`) virker over WebSocket (maarse-host), USB/seriel og Bluetooth (Nordic UART). Den skalerer billeder automatisk ned, så de passer under grænsen: 8 KB over LoRa og 1 MB over maarse-host.

## Webklienten som app (PWA)

Webklienten kan installeres som en app, så du kan bruge en radio via Bluetooth eller USB direkte fra telefonen eller computeren, uden en PC med maarse-host. Den virker også uden net, når den først er åbnet én gang.

- **Android (Chrome):** åbn klienten over https (fx fra GitHub Pages eller en anden statisk vært) eller fra maarse-host. Tryk på menuen ⋮ → **Installér app** (eller "Føj til startskærm"), eller tryk på **Installér app** i klienten. Forbind derefter med **Bluetooth** (nRF-noder) eller **USB** (kræver USB-OTG-kabel).
- **Windows, Linux og macOS (Chrome eller Edge):** klik på installér-ikonet i adresselinjen eller på **Installér app** i klienten.
- **iPhone/iPad:** Safari har hverken Web Bluetooth eller Web Serial. Der virker kun forbindelsen via WebSocket til en maarse-host (fx på en PC eller Raspberry Pi på samme net).
- **Notifikationer:** tryk på **Slå notifikationer til** i sidepanelet. Så viser klienten en notifikation, når der kommer en direkte besked eller en kanalbesked, mens den ligger i baggrunden. Antallet af ulæste står i fanens titel.
- **Opdatering:** når der findes en ny version, viser klienten "Ny version – genindlæs".

Offline-cache, installation og notifikationer kræver, at siden hentes over http(s) (localhost tæller som sikker). Åbnet direkte som fil (`file://`) virker klienten som før, bare uden de ting.

**Selv hoste klienten:** læg filerne i `maarse-client/` (`index.html`, `manifest.webmanifest`, `sw.js`, `icon.svg`, `icon-192.png`, `icon-512.png`) på en vilkårlig statisk https-vært, fx GitHub Pages. Hæv `VERSION` i `sw.js`, når filerne ændres, så brugerne får besked om den nye version (maarse-host gør det selv ud fra indholdet). Ikonerne laves med `python tools/make_icons.py`.

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
- Indtil videre er broen kun testet mod rammer, der er udledt af MeshCores kildekode, ikke mod rigtig hardware.

## Test

```sh
cargo test --workspace                  # 73 tests
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
  - Flash-placering: postkasse ved 0xEA000–0xEBFFF, indstillinger ved 0xEC000.
- **ESP32-S3:** bygget for begge boards med Espressifs Xtensa-værktøjskæde (ca. 550 KB app på Heltec V4, heraf ca. 220 KB til Bluetooth; app-partitionen er 2 MB). `--features check-esp32c6` type-tjekker stadig koden mod ESP32-C6.
  - Indstillinger og postkasse ligger i partitionen `maarse` ved 0x210000 (se `firmware/esp/partitions.csv`).
  - Heltec V4: standard-sendestyrken er 10 dBm, fordi PA'en lægger ca. 7–10 dB oveni. PA-styringen (GPIO7/2/46) følger Meshtastic for GC1109-udgaven. Har dit board KCT8103L (nyere V4), skal styringen måske justeres.
  - Bluetooth (Nordic UART) er med: esp-radios BLE-controller med TrouBLE som host, samme GATT-tjeneste, rammer og navn (`Maarse-xxxx`) som på nRF. Controlleren bruger en heap på 72 KB (esp-alloc). Ikke afprøvet på hardware, især ikke samspillet med LoRa og USB.
- **Mangler:** filer over LoRa på firmware, batterimåling og segmenterede stier over 12 hop.
- Ben-tildelingen er hentet fra Meshtastics board-filer.

## Licens

Koden er udgivet under enten MIT eller Apache-2.0, efter eget valg. Specifikationen er udgivet under CC-BY 4.0.
