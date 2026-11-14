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

Vores prototype (v0.4). Reticulums identitet og globale rækkevidde, MeshCores repeater-routing, kørt direkte på mikrocontrolleren. Ikke afprøvet på hardware endnu.

## Side om side

Må:rse er bedømt på hvad der er bygget og testet i v0.4, ikke på hvad der står i specifikationen.

javirkerdelvistmed begrænsningernejfindes ikke

|  | Meshtastic | MeshCore | Reticulum | Må:rse |
| --- | --- | --- | --- | --- |
| Routing |  |  |  |  |
| Princip | Styret flood: alle gentager, med forsinkelse efter signalstyrke | Flood første gang, derefter kildestyret sti via repeatere | Announces spredes; hver transportnode husker næste hop | Flood med ruteforespørgsel, derefter kildestyret sti; bedste sti vælges ud fra SNR |
| Max hop | 7 (standard 3) | 64 | 128 | 64 i alt, 12 i en sti |
| Hvem videresender | Alle (roller som ROUTER, ROUTER_LATE, CLIENT) | Kun repeatere (klienter kan slå "repeat" til) | Noder med transport slået til | Repeatere; skov-tilstand lader klienter hjælpe i op til 4 hop |
| Kvittering | jafor direkte beskeder | jaende til ende | jabevis for levering | japr. hop og ende til ende, med rutereparation |
| Duty cycle (EU) | ja | jatoken bucket fra v1.14.1 | japr. interface | jatoken bucket + lyt før send (CAD) |
| Bevægelige noder | jaflood er ligeglad med topologi | delviststier skal læres igen | delviststier opdateres via announces | jamobil-flag og ruteopdatering; 100 % i motorvejs-simulering |
| Rækkevidde ud over LoRa |  |  |  |  |
| Internet / TCP | delvistMQTT-bro via server | nejkun via tredjeparts-broer | jaTCP, UDP, I2P, seriel m.m. i samme net | jaTCP-backbone med DHT til at finde gateways |
| Finde nogen i et andet land | Via fælles MQTT-server | Nej | Announces over offentlige transportnoder | Opslag i DHT, derefter direkte forbindelse til modtagerens gateway |
| Sikkerhed |  |  |  |  |
| Identitet | Nodenummer + nøgle (PKC siden 2.5) | Ed25519-nøgle; kort præfiks som adresse | Nøglepar; adresse = 16-byte hash | Nøglepar; adresse = 16-byte hash; ingen afsender i klartekst |
| Direkte beskeder | jaX25519 + AES | jakrypteret, kompakt | jaX25519 + AES-256, forward secrecy på links | jaX25519 + ChaCha20-Poly1305, ny nøgle pr. besked |
| Signerede pakker | delvistkommer i 2.8 (alpha) | delvistadverts er signeret | jaannounces og beviser | delvistannounces er signeret |
| Beskyttelse mod spam | delvistgrænser for trafik | delvistloft over flood-hop | ja"stamps" (arbejdsbevis) fra 1.6 | nejkun duty cycle og flood-dæmpning |
| Funktioner |  |  |  |  |
| Offentlige kanaler | ja | jaogså #hashtag og regioner | delvistgrupper via apps | jaoffentlig og #hashtag |
| Beskeder til offline | delvistStore & Forward-modul | jaroom servers | jaLXMF-propagation nodes | japostkasse på host og i flash på noder |
| Filer og billeder | nej | nej | jaresources over links | delvistvia host (8 KB over LoRa), ikke på firmware endnu |
| Sider / tjenester | nej | delvistroom servers, sensorer | jaNomadNet-sider | nejHTML-sider er kun specificeret |
| Tale | nej | nej | jaopkald i Sideband | nejkun specificeret |
| Fjernadministration | jaadmin-nøgler | jalogin på repeatere | delvistrnstatus, fjern-styring | jastatus og radio på andre noder, kun for administratorer |
| Hardware og modenhed |  |  |  |  |
| Kører på mikrocontroller | jaESP32, nRF52, RP2040 m.fl. | jaESP32, nRF52, RP2040 m.fl. | nejRNode er kun modem; Rust-versioner er unge | janRF52840 og ESP32-S3 (4 boards) |
| Apps | Android, iOS, web, CLI | Officiel app (lukket kode) + web, tredjeparts | Sideband, NomadNet, MeshChat | Én web-klient (USB, Bluetooth, WebSocket) |
| Modenhed | Stor brugerbase, mange år | Hurtigt voksende, hyppige udgivelser | 1.0 i 2025, nu 1.6 | Prototype; 66 tests og simulator, ingen hardwaretest |
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
- Global rækkevidde: TCP-backbone og DHT, uden central server.
- Ingen afsender i klartekst, 16-byte adresser og moderne kryptering.
- Postkasse, filer og fjernadministration allerede i prototypen.

#### Ulemper

- Ikke afprøvet på rigtig hardware.
- Ingen brugere, kun én web-klient.
- Kryptografien er ikke gennemgået af andre.
- Store, tynde net er stadig svage: kvitteringer går tabt.
- Mange planlagte dele er kun beskrevet (sider, tale, IP-tunnel).

## Hvad Må:rse mangler, og hvad det bør have

"Mangler" er ting, vi har besluttet, men ikke bygget. "Bør tilføjes" er nye forslag, mest lånt fra de tre andre.

### Mangler (besluttet, ikke bygget)

Sorteret efter vigtighed.

HØJ**Test på hardware**Flash de fire boards, mål rækkevidde, strømforbrug og om Bluetooth og ESP32-buildet virker.

HØJ**Gateways til MeshCore og Reticulum**Et af de oprindelige mål. Uden dem starter Må:rse fra nul brugere.

HØJ**Batterimåling og energistyret routing på firmware**Routeren kan allerede tage hensyn til energi, men firmwaren måler ikke batteriet. Vigtigt for sol-repeatere.

MIDDEL**Filer og billeder på firmware**Virker i dag kun via host.

MIDDEL**Private kanaler**Kanal med egen nøgle, delt via QR-kode eller invitation.

MIDDEL**Bluetooth på ESP32-S3**Så Heltec V4 og XIAO S3 kan bruges fra telefonen.

MIDDEL**HTML-sider (`maarse://`)**Komprimerede, cachede sider på op til ca. 8 KB, som NomadNet.

MIDDEL**Node- og dækningskort**Kort over noder og målt dækning, som MeshMapper.

LAV**Talebeskeder (Codec2) og opkald (Opus)**Talebeskeder over LoRa, live-opkald kun over hurtige links.

LAV**IP-tunnel**TUN-interface med IPv6 udledt af adressen, så SSH og mosh kan køre.

LAV**Stier over 12 hop og anker-repeatere**Segmenterede stier og faste ankre langs fx motorveje.

### Bør tilføjes (nye forslag)

Hvert forslag nævner, hvor idéen kommer fra.

HØJ**Spam-beskyttelse**Lille arbejdsbevis på announces og ruteforespørgsler, så ingen kan oversvømme nettet billigt.fra Reticulum 1.6

HØJ**Forward secrecy for samtaler**I dag kan en stjålen nøgle åbne gamle beskeder. En kort nøgleudveksling (Double Ratchet-lignende) løser det.fra Reticulum-links

HØJ**Bekræft nøgler**Sikkerhedsnummer eller QR-kode, så man ved, at en kontakt er den rigtige.fra Meshtastic 2.7

HØJ**Ekstern sikkerhedsgennemgang og fuzzing**Fuzz pakke- og rammedekodere og få kryptoen gennemgået før v1.0.

MIDDEL**Regioner for kanaler**Kanalbeskeder bliver i et område i stedet for at gå over hele landet.fra MeshCore

MIDDEL**Opdatering af firmware over luften**Så sol-repeatere i marken kan opdateres uden at blive hentet ind.fra MeshCore

MIDDEL**Diagnose-værktøjer**Sporing af ruter, naboliste med SNR og måling af støjgulv på repeatere.fra MeshCore

MIDDEL**Position privat som standard**Del kun omtrentlig position, og kun når man selv vælger det.fra Meshtastic 2.8

MIDDEL**Telefon-apps**Android og iOS ovenpå web-klienten (fx som PWA), så almindelige brugere kan være med.

LAV**Telemetri og sensorer**Batteri, temperatur og vejr fra noder, plus bro til MQTT og Home Assistant.fra Meshtastic

LAV**Signerede udgivelser**Signér firmware og host-builds, så brugere kan tjekke, at de er ægte.fra Reticulum

## Hvor står Må:rse?

På papiret har Må:rse den bedste kombination: Reticulums identitet og globale rækkevidde, MeshCores effektive routing, og alt kører på chippen. Men de tre andre har det, der tæller mest i praksis: brugere, apps og år med drift.

Næste skridt er derfor ikke flere funktioner. Det er at få firmwaren til at virke på rigtig hardware, få en gateway til MeshCore, så man kan tale med eksisterende net, og få kryptoen set efter af andre.

Kilder (hentet november 2026):

- [Meshtastic-bloggen](https://meshtastic.org/blog/) (2.7 og 2.8-alpha)
- [Meshtastic 2.7 preview](https://meshtastic.org/blog/meshtastic-2-7-preview/) (nøgle-bekræftelse)
- [MeshCore v1.16.0](https://blog.meshcore.io/2026/06/06/release-1-16-0)
- [MeshCore v1.14.1](https://meshcoreeuropeeu.pages.dev/en/blog/2026-04-04-meshcore-v1-14-1-adds-duty-cycle-control-and-device-support) (duty cycle, stier)
- [Reticulum RNS 1.6.0](https://newreleases.io/project/github/markqvist/Reticulum/release/1.6.0)
- [FOSDEM 2026: Reticulum, what's next](https://fosdem.org/2026/events/attachments/9NCWUR-reticulum_community_meetup_implementations_migration_and_future/slides/262533/reticulum_dimz1j8.pdf) (licens og forks)

Må:rse-tal kommer fra projektets egne tests og simulator (v0.4).