LoRa-mesh · status november 2026

# Meshtastic, MeshCore, Reticulum og Må:rse

Fire måder at sende beskeder over LoRa uden mobilnet. Siden sammenligner dem side om side, giver fordele og ulemper og slutter med, hvad Må:rse mangler, og hvad det bør have.

## Kort fortalt

GPL-3.0 · C++

### Meshtastic

Det største og nemmeste. Alle noder gentager alle pakker (styret flood), højst 7 hop. Stabil version er 2.7; 2.8 med signerede pakker er i alpha.

MIT · C++

### MeshCore

Kun repeatere videresender. Første besked floodes, derefter går den ad en lært sti. Op til 64 hop, regioner og fjernstyring af repeatere.

Egen licens · Python

### Reticulum

En hel netværksstak, der kører over alt fra LoRa til internet. Stærkest på kryptografi og global rækkevidde, men kræver en computer eller telefon.

MIT/Apache-2.0 · Rust

### Må:rse

Vores prototype (v0.5). Reticulums identitet og globale rækkevidde, MeshCores repeater-routing, kørt direkte på mikrocontrolleren. Kører på hardware med gateway til MeshCore, Android-app og fjernadgang over VPN; rækkevidde og strøm er ikke målt.

## Side om side

Må:rse er bedømt på hvad der er bygget og testet i v0.4, ikke på hvad der står i specifikationen.

javirkerdelvistmed begrænsningernejfindes ikke

|  | Meshtastic | MeshCore | Reticulum | Må:rse |
| --- | --- | --- | --- | --- |
| Routing |  |  |  |  |
| Princip | Styret flood: alle gentager, med forsinkelse efter signalstyrke | Flood første gang, derefter kildestyret sti via repeatere | Announces spredes; hver transportnode husker næste hop | Flood med ruteforespørgsel, derefter kildestyret sti; bedste sti vælges ud fra SNR |
| Max hop | 7 (standard 3) | 64 | 128 | 64 i alt; over 12 med anker-repeatere |
| Hvem videresender | Alle (roller som ROUTER, ROUTER_LATE, CLIENT) | Kun repeatere (klienter kan slå "repeat" til) | Noder med transport slået til | Repeatere; skov-tilstand lader klienter hjælpe i op til 4 hop |
| Kvittering | jafor direkte beskeder | jaende til ende | jabevis for levering | japr. hop og ende til ende, med rutereparation |
| Duty cycle (EU) | ja | jatoken bucket fra v1.14.1 | japr. interface | jatoken bucket + lyt før send (CAD) |
| Bevægelige noder | jaflood er ligeglad med topologi | delviststier skal læres igen | delviststier opdateres via announces | jamobil-flag og ruteopdatering; 100 % i motorvejs-simulering |
| Rækkevidde ud over LoRa |  |  |  |  |
| Internet / TCP | delvistMQTT-bro via server | nejkun via tredjeparts-broer | jaTCP, UDP, I2P, seriel m.m. i samme net | jaTCP-backbone med DHT, MQTT-bro og gateway til MeshCore (afprøvet på hardware) |
| Finde nogen i et andet land | Via fælles MQTT-server | Nej | Announces over offentlige transportnoder | Opslag i DHT, derefter direkte forbindelse til modtagerens gateway |
| Sikkerhed |  |  |  |  |
| Identitet | Nodenummer + nøgle (PKC siden 2.5) | Ed25519-nøgle; kort præfiks som adresse | Nøglepar; adresse = 16-byte hash | Nøglepar; adresse = 16-byte hash; ingen afsender i klartekst |
| Direkte beskeder | jaX25519 + AES | jakrypteret, kompakt | jaX25519 + AES-256, forward secrecy på links | jaX25519 + ChaCha20-Poly1305, ny nøgle pr. besked; forward secrecy (udkast) |
| Signerede pakker | delvistkommer i 2.8 (alpha) | delvistadverts er signeret | jaannounces og beviser | delvistannounces og firmware er signeret |
| Beskyttelse mod spam | delvistgrænser for trafik | delvistloft over flood-hop | ja"stamps" (arbejdsbevis) fra 1.6 | jaarbejdsbevis på announces og ruteforespørgsler |
| Funktioner |  |  |  |  |
| Offentlige kanaler | ja | jaogså #hashtag og regioner | delvistgrupper via apps | jaoffentlig, #hashtag, private kanaler og regioner |
| Beskeder til offline | delvistStore & Forward-modul | jaroom servers | jaLXMF-propagation nodes | japostkasse på host og i flash på noder; telefonen kan bruge hjemme-hosten over VPN |
| Filer og billeder | nej | nej | jaresources over links | jaop til 8 KB over LoRa (også på firmware), 1 MB over internettet |
| Sider / tjenester | nej | delvistroom servers, sensorer | jaNomadNet-sider | ja`maarse://`-sider fra maarse-host, op til 8 KB komprimeret over LoRa |
| Tale | nej | nej | jaopkald i Sideband | jatalebeskeder (Codec2) og opkald (Opus over TCP) |
| Fjernadministration | jaadmin-nøgler | jalogin på repeatere | delvistrnstatus, fjern-styring | jastatus og radio på andre noder, kun for administratorer |
| Hardware og modenhed |  |  |  |  |
| Kører på mikrocontroller | jaESP32, nRF52, RP2040 m.fl. | jaESP32, nRF52, RP2040 m.fl. | nejRNode er kun modem; Rust-versioner er unge | janRF52840 og ESP32-S3 (4 boards) |
| Apps | Android, iOS, web, CLI | Officiel app (lukket kode) + web, tredjeparts | Sideband, NomadNet, MeshChat | Web-klient (PWA) og Android-app (Bluetooth til noden eller maarse-host over VPN med adgangsnøgle) |
| Modenhed | Stor brugerbase, mange år | Hurtigt voksende, hyppige udgivelser | 1.0 i 2025, nu 1.6 | Prototype; 187 tests, browsertest, fuzzing, simulator og testlaboratorium; signerede udgivelser; afprøvet på 2 boards |
| Licens | GPL-3.0 | MIT | MIT med tilføjede klausuler siden 2025; AGPL-fork findes | MIT eller Apache-2.0; spec under CC-BY |

## Fordele og ulemper

### Meshtastic

#### Fordele

- Klart flest brugere og noder i luften.
- Mange apps og boards, nem opsætning.
- Flood virker uden planlægning, også når alle bevæger sig.
- Store & Forward, telemetri, GPS og MQTT er indbygget.

#### Ulemper

- Flood bruger meget sendetid og skalerer dårligt i store net.
- Højst 7 hop.
- Kanaler bygger på delt nøgle; pakker signeres først fra 2.8.
- Internet kun via MQTT-servere, ikke som en del af nettet.

### MeshCore

#### Fordele

- Effektiv routing: kun repeatere sender, og faste stier sparer luft.
- Op til 64 hop; regioner holder kanaler lokale.
- Gode værktøjer til repeatere: fjernstyring, naboer, støjgulv.
- MIT-licens og aktiv udvikling.

#### Ulemper

- Ingen indbygget forbindelse ud over LoRa.
- Korte adresse-præfikser kan kollidere (derfor nu stier med 1–3 byte).
- Afhænger af, at nogen har sat repeatere op.
- Den officielle app er ikke open source.

### Reticulum

#### Fordele

- Stærkeste kryptografi og identitetsmodel af de fire.
- Samme net over LoRa, TCP, I2P og andet: rækker globalt.
- Flest tjenester: beskeder, filer, sider, tale.
- Spam-beskyttelse med arbejdsbevis fra 1.6.

#### Ulemper

- Kræver Python på en computer eller telefon; kører ikke selv på en LoRa-chip.
- Announces og stiopslag fylder meget på langsom LoRa.
- Licensen fik ekstra klausuler i 2025, og stifteren trådte tilbage samme år, så fremtiden er uklar.
- Mere teknisk at sætte op.

### Må:rse

#### Fordele

- Kører helt på mikrocontrolleren, uden heap, i Rust.
- Repeater-routing med bedste sti, kvittering pr. hop og ruteopdatering. I simulering bruges ca. en tredjedel af floodets sendetid.
- Global rækkevidde: TCP-backbone og DHT, uden central server, gateway til MeshCore og IP-tunnel.
- Ingen afsender i klartekst, 16-byte adresser og moderne kryptering.
- Postkasse, filer, private kanaler, signerede opdateringer over luften og gateway til MeshCore allerede i prototypen.

#### Ulemper

- Kun en første test på hardware; rækkevidde, strøm og mange funktioner er ikke prøvet i luften.
- Ingen brugere endnu; Android-appen er ny og kun prøvet i emulator og over wifi.
- Kryptografien er ikke gennemgået af andre.
- Nogle planlagte dele mangler (kort, gateway til Reticulum), og telefonen kan endnu ikke være sin egen node.

## Hvad Må:rse mangler, og hvad det bør have

"Mangler" er ting, vi har besluttet, men ikke bygget. "Bør tilføjes" er nye forslag, mest lånt fra de tre andre.

### Mangler (besluttet, ikke bygget)

Sorteret efter vigtighed.

HØJ**Mere test på hardware**Første test er bestået (Heltec V4.3 og XIAO nRF52840 + Wio-SX1262). Mangler: rækkevidde, strømforbrug, Bluetooth på ESP32-S3, filer og opdatering over Bluetooth i praksis (opdatering over LoRa er skrinlagt: for langsom), samt WisMesh Tag og XIAO ESP32-S3.

HØJ**Gateway til Reticulum**Gatewayen til MeshCore er lavet og afprøvet på hardware. Reticulum mangler.

MIDDEL**Node- og dækningskort**Kort over noder og målt dækning, som MeshMapper.


### Bør tilføjes (åbne forslag)

HØJ**Ekstern sikkerhedsgennemgang**Fuzzing er gjort uden fund; kryptoen, især forward secrecy (`maarse-crypto/src/ratchet.rs`), skal ses efter af andre før v1.0.

MIDDEL**Telefonen som egen node**Android-appen findes (Bluetooth og maarse-host over VPN). Næste trin: maarse-host i appen, så telefonen har egen identitet på TCP-backbone; derefter Bluetooth-radioen som modem. iOS mangler.

LAV**Sensorer**Batteri, oppetid, energistatus og chip-temperatur (nRF52840) sendes; MQTT-broen har Home Assistant discovery. Eksterne sensorer (vejr) og temperatur på ESP32-S3 mangler.

### Lavet siden første version

- **Hardware:** Heltec V4.3 og XIAO nRF52840 sender beskeder med kvittering begge veje; PA-chip på Heltec V4 findes automatisk.
- **Spam-beskyttelse:** arbejdsbevis på announces (12 bit) og ruteforespørgsler (8 bit).
- **Regioner for kanaler:** `navn@region`, regionsliste pr. node, filter på LoRa og backbone.
- **Bekræft nøgler:** sikkerhedsnummer (60 cifre) og QR-kode.
- **Private kanaler:** tilfældig nøgle, invitation som link og QR-kode.
- **Fuzzing** af alle dekodere og `forbid(unsafe_code)`.
- **Forward secrecy (udkast):** ratchet pr. kontakt, 5 bytes ekstra pr. besked.
- **Færre tabte kvitteringer** i store, tynde net.
- **Bluetooth på ESP32-S3** (Nordic UART Service).
- **Batterimåling og energistyret routing** på firmware.
- **Filer og billeder på firmware** (op til 8 KB) og filpostkasse på host.
- **Diagnose:** trace route med SNR, naboliste og støjgulv.
- **Position privat som standard**, afrundet til 10 km eller 1 km.
- **MQTT-bro** med Home Assistant discovery.
- **Gateway til MeshCore** i maarse-host, afprøvet på hardware (Heltec V4.3 med `companion_radio_usb`).
- **Visningsnavne** i announces, også vist i MeshCore-broen som `[Må Dennis]`.
- **`maarse://`-sider:** komprimeret markup fra `<data>/sider/`, browser med cache i webklienten. Ikke HTML, så fremmede sider ikke kan køre kode.
- **Signerede firmwareopdateringer** over USB, Bluetooth og nettet, med anti-rollback.
- **Webklient som PWA**, historik på flere skærme, gendannelsesfrase (24 ord) og krypteret sikkerhedskopi.
- **maarse-lab:** testlaboratorium i browseren med live simulator.
- **Talebeskeder** (Codec2, 1200 bit/s) og **opkald** (Opus, kun over TCP).
- **Anker-repeatere:** stier over 12 hop.
- **IP-tunnel** (IPv6 over TCP, Wintun/TUN).
- **Signerede udgivelser:** firmware, host og APK med udgivelsesnøgle og SHA256SUMS.sig.
- **Nyt design af webklienten** og **Android-app** (Bluetooth til noden).
- **Fjernadgang med adgangsnøgle:** appen og browsere kan forbinde til maarse-host over VPN eller hjemmenet; afprøvet fra en Android-telefon.
- **Chip-temperatur** i telemetrien (nRF52840).
- **Rolle** (klient, repeater, relay) gemmes i flash.

## Hvor står Må:rse?

På papiret har Må:rse den bedste kombination: Reticulums identitet og globale rækkevidde, MeshCores effektive routing, og alt kører på chippen. Men de tre andre har det, der tæller mest i praksis: brugere, apps og år med drift.

Siden første version er firmwaren kommet i luften, gatewayen til MeshCore virker, og telefonen kan bruge nettet via Bluetooth eller via hjemme-hosten over VPN. Næste skridt er at måle rækkevidde og strøm, afprøve opdateringer, filer og Bluetooth på rigtige boards, lade telefonen blive sin egen node og få kryptoen set efter af andre.

Kilder (hentet november 2026):

- [Meshtastic-bloggen](https://meshtastic.org/blog/) (2.7 og 2.8-alpha)
- [Meshtastic 2.7 preview](https://meshtastic.org/blog/meshtastic-2-7-preview/) (nøgle-bekræftelse)
- [MeshCore v1.16.0](https://blog.meshcore.io/2026/06/06/release-1-16-0)
- [MeshCore v1.14.1](https://meshcoreeuropeeu.pages.dev/en/blog/2026-04-04-meshcore-v1-14-1-adds-duty-cycle-control-and-device-support) (duty cycle, stier)
- [Reticulum RNS 1.6.0](https://newreleases.io/project/github/markqvist/Reticulum/release/1.6.0)
- [FOSDEM 2026: Reticulum, what's next](https://fosdem.org/2026/events/attachments/9NCWUR-reticulum_community_meetup_implementations_migration_and_future/slides/262533/reticulum_dimz1j8.pdf) (licens og forks)

Må:rse-tal kommer fra projektets egne tests og simulator (v0.5).