# Arkitektonisk vurdering og optimeringsforslag til Må:rse

Dette dokument opsummerer en kritisk gennemgang af den strategiske analyse af **Må:rse** (vs. MeshCore, Meshtastic og Reticulum) samt konkrete anbefalinger til, hvordan arkitekturen kan optimeres og forenkles for at opnå maksimal robusthed under et dansk katastrofe- eller beredskabsscenarie.

---

## 1. Hvor jeg er helt enig med analysen

Analysen rammer plet på de helt fundamentale arkitektoniske udfordringer for et dansk nød- og beredskabsnet:

* **Hybrid-tilgangen er den eneste rigtige vej i Danmark:** Danmark har et relativt fladt terræn, men er geografisk opdelt af fjorde, bælter og øer. At dække hele landet rent med LoRa-repeatere kræver enorm infrastruktur og skaber hurtigt flaskehalse i båndbredden. Kombinationen af lokalt LoRa-mesh, hurtige lokale links (Wi-Fi/5 GHz) og et IP-backbone giver langt mere mening.
* **Adskillelse af noderoller (MeshCore-modellen):** Hvis almindelige håndholdte brugerenheder fungerer som aktive repeatere (som i standard-Meshtastic), drukner radiokanalen i unødvendige retransmitteringer, og brugernes batterier drænes. Faste infrastruktur-noder (Klasse A/B/C) kombineret med dedikerede mobile relay-noder (Klasse D) er afgørende.
* **Prioritering (QoS) og 10 % duty cycle:** På `869,525 MHz` (hvor 10 % duty cycle tillader 360 sekunders sendetid i timen) er sendetid den absolut mest knappe ressource. Uden benhård trafikprioritering (`P0`–`P4`) og mulighed for at afbryde filoverførsler midt i fragmenter, vil en enkelt bruger, der sender et billede eller en fil, kunne blokere en kritisk nødmelding.
* **Delay-Tolerant Networking (Carry / Store-and-forward):** Under et større strøm- eller internetnedbrud vil netværket uundgåeligt blive splittet i isolerede "øer". At bruge køretøjer (beredskab, hjemmeværn, kommunale biler) som fysiske databærere mellem isolerede mesh-øer er en af de mest robuste løsninger i praksis.

---

## 2. Hvad jeg ville gøre anderledes – eller skærpe yderligere

For at undgå, at Må:rse bliver for tungt, fejlbehæftet eller komplekst til små batteridrevne enheder, anbefales følgende **fem arkitektoniske justeringer**:

### A. Pas på "Feature Creep" (Skær ind til benet i v1.0)
Analysen fremhæver, at Må:rse rummer alt fra Codec2/Opus-voice og små hjemmesider til Home Assistant/MQTT, DHT, forward secrecy og filoverførsel. **Det er projektets største risiko.**

* **Anbefaling:** Hold voice (Codec2/Opus) og "små hjemmesider" helt ude af LoRa-laget og kernen i første omgang. Voice over LoRa kræver så stabilt et link og så høj datahastighed (`SF7`/`SF8`), at det i et katastrofescenarie med svage signaler primært skaber frustration og støj på båndet.
* **Fokusér 100 % på:** Kort tekst, strukturerede statuspakker (telemetri, position, hjælpebehov), kvitteringer og små komprimerede datafiler over højere båndbredde (Wi-Fi/TCP).

### B. Drop klassisk "Path Routing" til fordel for Gradient / Directed Flooding
Analysen foreslår tre forskellige routing-algoritmer (*Static*, *Dynamic* og *Delay-Tolerant*) samt Wi-Fi-lignende roaming mellem repeatere. At vedligeholde tre separate routing-mekanismer på en microcontroller (f.eks. ESP32-S3) er ekstremt fejlbehæftet.

* **Anbefaling:** Brug **Cost/Gradient-baseret routing** (inspireret af Reticulum eller RPL) kombineret med **Hop-by-Hop ACK**.
* **Princip:** I stedet for at låse en besked til en fast sti (`A -> B -> C -> D`), bør hver node blot vide, hvilke naboer der har den laveste "pris" (et vægtet tal baseret på hop-antal, SNR og batteristatus) hen mod destinationen eller nærmeste gateway. Hvis `C` forsvinder, vælger `B` automatisk den næstbedste nabo på næste hop – uden at skulle lave en ny *Path Request* helt fra afsenderen.

### C. Gør DHT-laget ekstremt simpelt (og undgå Kademlia-kompleksitet)
En fuld distribueret hash-tabel (DHT) på tværs af ustabile forbindelser lider ofte under *churn* (noder, der falder ud og ind, skaber massiv synkroniseringstrafik).

* **Anbefaling:** I et dansk netværk med f.eks. 20–50 regionale gateways er en kompleks P2P-DHT overflødig. Brug i stedet et **Gossip-baseret katalog (CRDT / State-based sync)** mellem gateways.
* **Fordel:** 50 gateways kan synkronisere en samlet tabel over, hvilke brugere der sidder under hvilken gateway, på få kilobyte hvert minut – og modellen overlever netværksopdelinger (*split-brain*) langt bedre end en klassisk DHT.

### D. Krypto-pragmatisme på LoRa-laget
Analysen nævner X25519, ChaCha20-Poly1305, Ed25519 og **Forward Secrecy** (Double Ratchet).

* **Udfordring:** Forward secrecy (ratcheting) kræver synkroniseret tilstand mellem afsender og modtager. På et tabstungt LoRa-netværk med *carry* og *store-and-forward*, hvor pakker ankommer i vilkårlig rækkefølge eller med timers forsinkelse, er ratchets berygtede for at komme ud af sync, så modtageren ikke kan dekryptere beskeden.
* **Anbefaling:** Brug statisk X25519 key-exchange kombineret med en tilfældig engangsnøgle (*ephemeral key*) pr. besked (ECIES / *Sealed Box*-princippet). Det giver stadig stærk kryptering og autenticitet uden risiko for, at en tabt pakke ødelægger kryptotilstanden for fremtidige beskeder.

### E. Brug radioen som et "Dumt Radiomodem" (RNode-modellen)
I stedet for at bygge al routing-, mailbox- og protokoldynamik direkte ind i firmwaren på alle ESP32/nRF52-boards, bør arkitekturen opdeles som i **Reticulum (RNode)**:

* **Microcontroller (L1/L2):** Fungerer som en ren, lynhurtig radiotransceiver med kø-styring, CSMA/CA (*listen-before-talk*) og Hop-ACK.
* **Host / Companion (L3–L7):** Læg al intelligens (routing, mailbox, kryptering, prioritering, Wi-Fi/TCP-bro) i en **Companion-app (Android/iOS)** eller på en **Raspberry Pi / Linux-host** (for faste noder).
* **Fordel:** Det gør det markant nemmere at opdatere og fejlfinde protokollen uden at skulle flashe fysisk firmware på alle radioer i landet.

---

## 3. Opsummering af anbefalede ændringer

| Område | Nuværende / Foreslået i analyse | Anbefalet optimering |
| :--- | :--- | :--- |
| **Funktionsomfang** | Voice, små websider, MQTT, chat, filer | Skær ind til tekst, telemetri, kvitteringer og små datafiler |
| **Routing** | 3 separate algoritmer + faste stier | Én samlet Cost/Gradient-routing med Hop-by-Hop ACK |
| **Backbone-opslag** | Distribueret Hash-Tabel (DHT) | Simpel Gossip / CRDT-synkronisering mellem gateways |
| **Kryptering** | Double Ratchet (Forward Secrecy) | ECIES / Ephemeral key pr. besked (stateless dekryptering) |
| **Firmware-arkitektur** | Fuld protokolstack på MCU | RNode-princip: Dumt L1/L2-modem + smart Host/App |