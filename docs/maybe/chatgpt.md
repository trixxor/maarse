Strategisk analyse af MaArSe vs. MeshCore, Meshtastic og Reticulum Det
her er faktisk et rigtig interessant projekt, især hvis målet er at
udvikle MaArSe til et landsdækkende, decentraliseret
kommunikationsnetværk, der kan fungere under større strømsvigt,
internetnedbrud, oversvømmelser eller andre katastrofer i Danmark.

Min overordnede anbefaling er:

MaArSe bør ikke nødvendigvis forsøge at blive en dansk udgave af
Meshtastic eller MeshCore. Det bør i stedet udvikles som en hybrid
kommunikationsplatform, der kan benytte flere forskellige
transportmedier.

Det vil sige: - LoRa til langtrækkende kommunikation med lavt
strømforbrug. - TCP/IP til hurtig kommunikation, hvor der er
netværksforbindelse. - Radioforbindelser mellem geografiske områder,
hvor internet ikke er tilgængeligt. - Store, permanente backbone-noder
med batteri og solceller. - Mobile noder til beredskab, frivillige,
køretøjer og personer. - Store og små beskeder med forskellige
prioriteter. - Mulighed for at fungere helt uden central server. Jeg vil
sammenligne systemerne ud fra både teknisk arkitektur og det praktiske
perspektiv: Hvad virker bedst, når man faktisk skal kunne kommunikere
under en katastrofe? \## 1. Hvad jeg kan se i MaArSe Jeg har gennemgået
repository'et og README'en, herunder den beskrevne arkitektur,
funktioner, hardwareunderstøttelse og sammenligningen med de øvrige
systemer.

Det første, jeg vil fremhæve, er, at MaArSe allerede har nogle ret
interessante arkitektoniske valg.

#### Interessant arkitektur

MaArSe kombinerer tre ting, som normalt er adskilt: \## 1. Et LoRa-mesh
med dynamisk rutefinding. \## 2. Et TCP-backbone med distribueret
DHT-opslag. \## 3. Store-and-forward med postkasser, så modtageren ikke
behøver at være online. Dertil kommer blandt andet: - Hop-for-hop
kvittering og retransmission. - End-to-end-kryptering med X25519 og
ChaCha20-Poly1305. - Ed25519-identiteter. - Forward secrecy. - Regionale
kanaler. - Mobil videresendelse (carry). - Voice, Codec2 og Opus. -
Filoverførsel og små hjemmesider. - Home Assistant/MQTT-integration. -
Gateway til MeshCore. - Windows-host og Android-klient. Det er en
bredere ambition end blot at lave en LoRa-messenger.

#### Den vigtigste forskel

MaArSe forsøger at kombinere: - MeshCore-lignende effektiv rutning. -
Meshtastic-lignende mulighed for at finde alternative veje. - Reticulums
idé om et transportuafhængigt netværk. - En selvstændig
mailbox-/store-and-forward-funktion. - En internet-backbone, der kun
sender trafik til det relevante geografiske område. Det er en rigtig
fornuftig retning.

Men der er en vigtig forskel på at have funktionerne implementeret og at
have dokumenteret, at de fungerer stabilt i et virkeligt netværk.
README'en angiver selv v0.5 som prototype, med to radioenheder på et
bord, PC-host, webklient og Android-app, men endnu uden feltmålinger
eller ekstern kryptografisk revision.

## 2. Direkte sammenligning

Arkitektonisk vurdering -- ikke en benchmarkmåling. Vurderingen er
baseret på projekternes dokumenterede design. \| Funktion \| MaArSe \|
MeshCore \| Meshtastic \| Reticulum \| \| --- \| --- \| --- \| --- \|
--- \| \| Primært formål \| Hybridnetværk \| LoRa-mesh \| LoRa-messenger
\| Generelt netværk \| \| Routing \| Path routing \| Learned paths \|
Flooding + routing \| Hop-by-hop \| \| Dynamisk rerouting \| Ja,
designet til det \| Begrænset \| Ja \| Ja \| \| TCP-backbone \| Ja \|
Ikke indbygget \| MQTT \| Ja \| \| Store-and-forward \| Ja \| Room
server \| Store & Forward \| Propagation \| \| End-to-end-kryptering \|
Ja \| Ja \| Ja \| Ja \| \| Mobil relay \| Carry/forest mode \| Begrænset
\| Ja \| Transportnoder \| \| Filoverførsel \| Ja \| Begrænset \|
Begrænset \| Ja \| \| Voice \| Ja \| Ikke centralt \| Moduler/begrænset
\| Via applikationer \| \| Ingen central server \| Ja \| Ja \| Ja \| Ja
\| \| Flere transportmedier \| Ja \| Begrænset \| Begrænset \| Ja \| \|
Modent feltmiljø \| Nej \| Ja \| Ja \| Ja \|

En vigtig nuance: Meshtastic har også udviklet direkte routing for
beskeder, og Reticulum har flere transportmuligheder end blot LoRa. De
er derfor ikke helt så simple, som en traditionel sammenligning kan give
indtryk af.

### A. MaArSe

#### Største potentiale

Fordele: - Kan kombinere langsom LoRa med hurtig TCP. - DHT giver
mulighed for geografisk distribueret kommunikation. - Hop-kvitteringer
begrænser behovet for at genudsende hele ruten. - Mailbox gør asynkron
kommunikation mulig. - Carry giver mulighed for at udnytte mobile
mennesker og køretøjer. - Regionale kanaler kan holde lokal trafik
lokal. - Voice, filer og data giver flere anvendelsesmuligheder end
almindelig chat. Ulemper og risici: - Ny kodebase betyder endnu ingen
større produktionshistorik. - Mange funktioner giver stor
kompleksitet. - Path routing kan blive problematisk, hvis noder flytter
sig hurtigt. - DHT og TCP-backbone skal kunne håndtere
netværkspartitioner. - Ingen ekstern kryptografisk revision. - Begrænset
hardware- og feltvalidering. - Risiko for at protokollen bliver for
kompleks til små batteridrevne enheder. Min vurdering: MaArSe har den
mest interessante samlede arkitektur til det formål, du beskriver, men
har også den største dokumentations- og valideringsopgave. \### B.
MeshCore \#### Stærk infrastrukturmodel

MeshCore har en meget interessant filosofi: Brugerne skal ikke
nødvendigvis være repeatere.

Der er typisk: - Companion-enheder til brugerne. - Repeatere til
videresendelse. - Room servers til beskeder og opslag. Fordele: - God
kontrol over, hvem der bruger radiokapacitet. - Velegnet til permanente
repeatere på tage og master. - Mindre unødvendig trafik fra almindelige
brugere. - Relativt enkel infrastrukturmodel. - Gode muligheder for at
bygge et regionalt net. Ulemper: - Afhængig af at infrastrukturen er
placeret fornuftigt. - En mobil bruger kan miste sin lærte rute. - Ikke
samme indbyggede TCP-netværksarkitektur som MaArSe. - Mindre fleksibel
til dynamiske, blandede transportnetværk. MeshCore er efter min mening
den bedste inspiration til MaArSe's infrastrukturroller. \### C.
Meshtastic \#### Bedst til hurtig udbredelse

Meshtastic har en stor fordel: Det er nemt at komme i gang.

Fordele: - Stor brugerbase og hardwareunderstøttelse. - Mange
eksisterende enheder. - Relativt let at installere. - Brugernoder kan
videresende trafik. - Godt til mobile grupper, hvor der ikke er
etableret infrastruktur. - MQTT giver mulighed for internetbaseret
sammenkobling. Ulemper: - Flooding kan skabe meget radiotrafik. -
Batteriforbrug kan stige med mængden af videresendt trafik. - Begrænset
kontrol over store netværk med mange samtidige beskeder. - MQTT-baseret
sammenkobling er ikke det samme som et intelligent, decentraliseret
backbone. Meshtastic er især interessant som en slags nødnet, hvor man
ikke på forhånd ved, hvor infrastrukturen findes. \### D. Reticulum
\#### Bedste netværksfilosofi

Reticulum er nok den mest interessante konkurrent rent arkitektonisk.

Det er ikke bare en messenger. Det er en komplet netværksstack.

Fordele: - Flere transportmedier. - LoRa, TCP, Wi-Fi, serial og andre
interfaces. - Kryptering som grundprincip. - Ingen traditionel central
server. - Mulighed for at bygge mange forskellige applikationer
ovenpå. - God understøttelse af asynkron kommunikation. Ulemper: - Mere
kompleks at implementere og administrere. - Kræver typisk en host,
selvom microcontroller-implementeringer findes. - Mindre intuitiv for
almindelige brugere. - Annonceringer og netværksopdagelse kan være dyre
over langsom LoRa. Reticulum er den, jeg ville studere mest grundigt,
når det gælder MaArSe's fremtidige netværksarkitektur.

## 3. Hvor MaArSe faktisk har en fordel

Jeg synes, der er fem områder, hvor I potentielt kan gøre noget bedre
end de eksisterende løsninger. \### 3.1 Hop-for-hop kvittering Det her
er en af de mest interessante funktioner i MaArSe.

Forestil dig en besked, der skal gennem fem repeatere: Afsender Start

A ACK

B ACK

C ACK

D ACK

E ACK

Modtager Leveret

#### Illustration af en rute med lokal kvittering på hvert hop.

Hvis forbindelse C--D fejler, behøver afsenderen ikke nødvendigvis at
sende hele beskeden igen.

Det er særligt interessant ved: - Lange forbindelser. - Svage
radiolinks. - Mobile repeatere. - Batteridrevne repeatere. - Netværk med
varierende signalstyrke. Men jeg ville undersøge en vigtig optimering:

Brug forskellige kvitteringsniveauer: \## 1. Hop-ACK: Næste repeater har
modtaget pakken. \## 2. Route-ACK: Pakken er kommet frem til næste
vigtige rutepunkt. \## 3. Delivery-ACK: Slutmodtageren har modtaget
beskeden. \## 4. Application-ACK: Slutapplikationen har behandlet
beskeden. Det er ikke nødvendigt at kvittere med store datapakker for
hvert hop. En meget lille ACK-pakke med et reference-ID kan være
tilstrækkelig.

Jeg ville også implementere en begrænset retransmission med backoff og
jitter, så repeatere ikke begynder at sende samtidig, når forbindelsen
bliver dårlig. \### 3.2 TCP-backbone med DHT Her har MaArSe en meget
væsentlig fordel.

Forestil dig, at Danmark opdeles i 20 geografiske områder. Distribueret
backbone

DHT · TCP · Gateway discovery

Nord GatewayLoRa-mesh

Midt GatewayLoRa-mesh

Syd GatewayLoRa-mesh

Øst GatewayLoRa-mesh

Forenklet eksempel med fire geografiske netværksområder.

Hvis en person i Aalborg vil sende en besked til en person i Odense:
\## 1. Aalborg-radioen afleverer beskeden til sin lokale gateway. \## 2.
Gatewayen finder modtagerens område via DHT. \## 3. Beskeden sendes over
TCP. \## 4. Odense-gatewayen afleverer beskeden til sit lokale LoRa-net.
\## 5. Hvis modtageren er offline, opbevares beskeden. Det er præcis den
type arkitektur, jeg ville bygge videre på.

Min anbefaling: Gør TCP-backbone til en optimering og ikke en
forudsætning.

Det betyder: - Hvis TCP virker, benyttes det. - Hvis TCP forsvinder,
skal LoRa-nettet fortsætte. - Hvis en gateway går ned, skal en anden
gateway kunne overtage. - Hvis Danmark bliver opdelt i isolerede
netværk, skal hvert område fortsætte selvstændigt. - Når forbindelsen
kommer tilbage, skal områderne kunne synkronisere igen. Det sidste punkt
er meget vigtigt. Det er ikke nok at kunne sende en besked gennem
nettet. Man skal også kunne håndtere, at to isolerede netværk har været
aktive hver for sig i flere timer eller dage. \### 3.3 Mailbox og
store-and-forward Her ville jeg faktisk gå endnu længere.

MaArSe har allerede mailbox på host og i repeaterens flash.

Jeg ville udvikle tre niveauer: \| Niveau \| Placering \| Formål \| \|
--- \| --- \| --- \| \| L1 \| Handheld \| Kortvarig kø \| \| L2 \|
Repeater \| Regional lagring \| \| L3 \| Gateway/server \|
Langtidslagring og synkronisering \|

Eksempel:

En besked sendes fra Aalborg til København, men forbindelsen mellem
landsdelene er afbrudt.

Beskeden gemmes i L3 i Aalborg. Når TCP-forbindelsen kommer tilbage,
sendes den videre.

Hvis en repeater mister strøm, må den ikke være den eneste kopi af en
kritisk besked.

Jeg ville tilføje: - Prioritetsklasser. - Udløbstid (TTL). - Kvittering
for lagring. - Kvittering for endelig levering. - Duplikatbeskyttelse. -
Begrænsning af lagerplads per bruger. - Genoptagelse af delvist
overførte filer. Og vigtigst: En besked, der allerede er leveret, må
ikke cirkulere rundt i netværket igen. \### 3.4 Carry -- mobil
videresendelse Det her er efter min mening en af MaArSe's største
muligheder.

Forestil dig, at en redningsbil kører fra Aalborg til Hjørring.

Bilen har en MaArSe-radio, der automatisk kan tage imod beskeder fra et
område og transportere dem videre til et andet område.

Det kan også være: - En hjemmeværnsenhed. - En ambulance. - En kommunal
servicebil. - En frivillig på cykel. - En person med en mobil
LoRa-enhed. Jeg ville gøre carry til en selvstændig funktion i
protokollen med følgende egenskaber: \## 1. En node kan opbevare en
besked uden at have en fast rute. \## 2. Den kan se, hvilke områder
eller noder den potentielt kan bringe beskeden tættere på. \## 3. Den
afleverer automatisk beskeden, når den møder en relevant node. \## 4.
Den kan have flere kopier af kritiske beskeder. \## 5. Den sletter ikke
sin kopi, før den har modtaget en gyldig kvittering. Jeg ville også
tilføje en mobility score, der tager højde for: - Hastighed. -
Bevægelsesretning. - Forventet kontakt med andre noder. -
Batteriniveau. - Historiske møder med andre noder. Det kan gøre
netværket meget mere effektivt ved store geografiske afstande.

## 4. De største problemer, jeg ville løse

Her kommer de områder, hvor jeg mener, MaArSe skal forbedres, før det
kan blive et seriøst katastrofenet. \### 4.1 Routing skal kunne håndtere
både faste og mobile netværk MaArSe's nuværende model med path request,
SNR-baseret valg af rute og lokale kvitteringer er god.

Men en fast rute er ikke nødvendigvis den bedste rute fem minutter
senere.

Jeg ville indføre tre routingtyper: Static infrastructure routing

Faste repeatere på master, bygninger og høje punkter. Ruter kan caches
længe og optimeres efter linkkvalitet.

Dynamic routing

Automatisk opdagelse af nye naboer, linkfejl og alternative ruter. Ruter
får en udløbstid og genberegnes ved behov.

Delay-tolerant routing

Gem beskeder til senere levering. Brug carry, når der ikke eksisterer en
sammenhængende forbindelse.

Jeg ville undgå, at én routingalgoritme skal løse alle tre opgaver. \###
4.2 Netværket skal have en egentlig trafikstyring Det er måske det
vigtigste punkt efter sikkerhed.

Et katastrofenet skal kunne håndtere: - 10 beskeder i minuttet. - 1.000
beskeder i minuttet. - 10.000 enheder. - Et stort antal
statusmeldinger. - Mange brugere, der forsøger at sende samtidig. Det er
ikke realistisk at sende al trafik med samme prioritet.

Jeg foreslår denne prioritering: \| Prioritet \| Trafik \| Eksempel \|
\| --- \| --- \| --- \| \| P0 \| Nødtrafik \| Livstruende situation \|
\| P1 \| Beredskab \| Evakuering, koordinering \| \| P2 \| Drift \|
Infrastruktur, status, forsyning \| \| P3 \| Almindelige beskeder \|
Chat og beskeder \| \| P4 \| Ikke-kritisk \| Billeder, filer,
opdateringer \|

P0 og P1 skal kunne fortrænge P3 og P4 fra sendekøen.

Men der skal også være fairness og begrænsninger, så én bruger ikke kan
blokere hele nettet ved at sende falske nødmeldinger. \### 4.3 En
sikkerhedsrevision er nødvendig I har allerede valgt gode kryptografiske
byggesten.

Men jeg ville ikke betragte systemet som sikkert nok til beredskabsbrug,
før en uafhængig ekspert har gennemgået: - Identitetsoprettelse og
nøglehåndtering. - Ratchet og forward secrecy. - Replay protection. -
Nonce-håndtering. - Signering af firmware. - DHT-manipulation. - Falske
repeatere. - Denial-of-service. - Kompromitterede gateways. - Nøgle- og
identitetsgendannelse. Der er også en vigtig arkitektonisk detalje:
Kryptering af indhold beskytter ikke automatisk routinginformation,
lokationsdata eller trafikmønstre.

MaArSe skjuler afsenderens adresse, men modtageradresse og rute kan
stadig observeres ifølge jeres egen beskrivelse.

Jeg ville derfor arbejde videre med metadata-minimering, men uden at
gøre fejlsøgning og routing unødigt kompliceret. \## 5. Sådan ville jeg
gøre MaArSe hurtigere Her er der efter min mening nogle muligheder, der
kan gøre en meget stor forskel.

Først skal man skelne mellem to ting: - Hurtigere levering: Hvor hurtigt
en besked kommer frem. - Højere kapacitet: Hvor mange beskeder og hvor
meget data netværket kan håndtere. LoRa er fantastisk til rækkevidde og
lavt strømforbrug, men det er ikke et hurtigt medie. Derfor skal MaArSe
kunne vælge transport efter opgavens karakter. \### 5.1 Flere
transportmedier Jeg ville udvikle MaArSe omkring en
transportabstraktion. MaArSe Core Identitet · Kryptering · Routing ·
Prioritering · Mailbox LoRa Lang rækkevidde Små beskeder, status og
nødmeldinger

Wi-Fi Høj kapacitet Lokale forbindelser og mobil hotspots

TCP/IP Backbone Fiber, Ethernet og IP-forbindelser

Andre radioer Ekstra rækkevidde Packet radio og andre understøttede
links

Samme MaArSe-adresse og kryptering Transporten må ikke ændre brugerens
identitet

Det er vigtigt, at transporterne ikke bare bliver forskellige gateways.
De skal være egentlige interfaces i samme netværksstack.

Eksempelvis: trait Transport { fn send(&mut self, packet: &\[u8\]) -\>
Result\<(), Error\>; fn receive(&mut self) -\> Option`<Packet>`{=html};
fn mtu(&self) -\> usize; fn capacity(&self) -\> LinkCapacity; fn
latency(&self) -\> LatencyEstimate; fn is_available(&self) -\> bool; }

Det er kun en illustrativ API, men princippet er vigtigt.

MaArSe Core skal ikke vide, om en pakke sendes via LoRa, Wi-Fi eller
TCP. \### 5.2 Hvilke forbindelser ville jeg prioritere? \| Transport \|
Hastighedspotentiale \| Strømforbrug \| Anvendelse \| \| --- \| --- \|
--- \| --- \| \| LoRa SF9 \| Lavt \| Lavt \| Beskeder \| \| LoRa SF7 \|
Højere \| Lavt/moderat \| Lokale beskeder \| \| Wi-Fi 2,4 GHz \| Højt \|
Moderat \| Mobil forbindelse \| \| Wi-Fi 5 GHz \| Meget højt \|
Moderat/højt \| Backbone \| \| Ethernet/fiber \| Meget højt \| Lavt pr.
overført data \| Fast backbone \| \| TCP over eksisterende internet \|
Meget højt \| Afhænger af forbindelsen \| Landsdækkende net \|

Det er ikke konkrete gennemløbsmålinger af MaArSe, men transporternes
relative egenskaber.

Min anbefaling er, at I beholder LoRa som grundlaget, men begynder at
udvikle en Wi-Fi-baseret backbone som et selvstændigt lag.

Det vil være langt mere effektivt end at forsøge at presse store
datamængder gennem LoRa. \### 5.3 Adaptiv LoRa-hastighed Jeg ville også
gøre radiokonfigurationen mere intelligent.

I dag er det typisk en bestemt kombination af: - Spreading Factor. -
Bandwidth. - Coding Rate. - Transmit power. Men der er ingen grund til,
at alle links skal bruge samme indstillinger, hvis hardware og
radioforhold tillader andet.

Eksempel: \| Linkkvalitet \| Foreslået strategi \| \| --- \| --- \| \|
Meget god \| SF7 / højere datahastighed \| \| God \| SF8 \| \| Middel \|
SF9 \| \| Dårlig \| SF10--SF11 \| \| Meget dårlig \| SF12 eller
store-and-forward \|

Det kræver dog, at begge sider kan kommunikere med den valgte profil.

Jeg ville implementere en Link Adaptation Manager, som løbende måler: -
RSSI. - SNR. - Packet Error Rate. - Retransmissioner. - Airtime. -
Round-trip time. Den skal ændre radioparametre forsigtigt og have
hysterese, så den ikke skifter frem og tilbage hele tiden.

Et vigtigt designvalg: Hav en fast, velkendt rendezvous-profil, så noder
altid har mulighed for at finde hinanden igen, hvis en hurtigere profil
fejler. \### 5.4 Fragmentering og pakkestørrelse En anden mulighed er at
optimere pakkerne.

Jeg ville bruge: - Små kontrolpakker. - Komprimerede headers. -
Fragmentering af større beskeder. - Selektiv retransmission af manglende
fragmenter. - Dedikerede sekvensnumre. - Mulighed for at fortsætte en
filoverførsel efter forbindelsesbrud. Det er især relevant, fordi jeres
guide allerede beskriver filoverførsel med 8 KB over LoRa og op til 1 MB
over internet.

En vigtig detalje: Prioritetsstyringen skal også kunne afbryde en stor
filoverførsel mellem fragmenter, så en nødmelding ikke skal vente på, at
en hel fil er sendt.

## 6. Hvordan skal MaArSe fungere uden TCP-backbone?

Det her er den vigtigste del, hvis I vil bruge det som katastrofenet.

Jeg ville designe netværket med tre driftsformer. Mode 1 -- Fuldt
netværk Normal drift

-   TCP-backbone aktiv.
-   LoRa som adgangsnet.
-   Wi-Fi-backbone mellem geografiske områder.
-   DHT og distribuerede postkasser.
-   Automatisk valg af hurtigste og mest pålidelige rute.

Mode 2 -- Isolerede regioner Backbone afbrudt

-   Hver region arbejder selvstændigt.
-   Lokale repeatere fortsætter.
-   Lokale mailboxes fortsætter.
-   DHT-opslag erstattes af lokal routing og cache.
-   Beskeder til andre regioner sættes i kø.

Mode 3 -- Total isolering Nøddrift

-   Kun direkte radio og lokale repeatere.
-   Forest mode aktiveres.
-   Carry bruges mellem isolerede områder.
-   Kun kritiske beskeder prioriteres.
-   Noder bruger lokale og kendte identiteter.

#### En vigtig forbedring: Gem routingviden lokalt

I jeres tekniske guide står der, at DHT kun eksisterer over TCP og ikke
over LoRa.

Det er helt fornuftigt, men jeg ville sikre, at en node stadig har et
lokalt katalog over: - Kendte repeatere. - Kendte regionale gateways. -
Tidligere fungerende ruter. - Regionernes tilgængelighed. - Kendte
mobile relaynoder. På den måde er DHT ikke nødvendig for at drive en
region. \#### Genforbindelse efter en katastrofe Her ville jeg indføre
en egentlig Partition Recovery Protocol.

Når to netværk har været adskilt: \## 1. De udveksler regionale
statusoplysninger. \## 2. De udveksler hvilke gateways og områder, de
kan nå. \## 3. De sammenligner mailbox-indhold og besked-ID'er. \## 4.
De synkroniserer manglende beskeder. \## 5. De løser konflikter og
fjerner dubletter. \## 6. De genopbygger routingtabellerne. Det er
vigtigt, at det ikke bare bliver en stor flood, når internettet kommer
tilbage.

Jeg ville anvende Merkle trees eller tilsvarende kompakte
synkroniseringsstrukturer til at finde forskelle mellem postkasser og
kataloger.

## 7. Hvordan jeg ville bygge et landsdækkende MaArSe i Danmark

Jeg ville ikke begynde med at forsøge at dække hele Danmark med
LoRa-repeatere.

Det bliver dyrt, vanskeligt at vedligeholde og unødvendigt, hvis der
allerede findes andre forbindelsesmuligheder.

I stedet ville jeg bygge et hierarkisk net.

#### Foreslået netværksstruktur

  Niveau   Infrastruktur            Funktion
  -------- ------------------------ --------------------------------------
  L0       Telefoner og handhelds   Brugere
  L1       Mobile repeatere         Køretøjer og beredskab
  L2       Lokale repeatere         Byer og landsbyer
  L3       Regionale gateways       Kommuner og regioner
  L4       Backbone                 Forbindelse mellem regionerne
  L5       Nationale tjenester      Kort, kataloger, status og tjenester

#### Geografisk opdeling

Jeg ville bruge kommuner eller naturlige dækningsområder som lokale
netværkszoner og ikke nødvendigvis regionernes administrative grænser.

Eksempelvis: - Nordjylland. - Østjylland. - Vestjylland. -
Midtjylland. - Sydjylland. - Fyn. - Sjælland. - Hovedstadsområdet. -
Bornholm. Det skal være muligt at kommunikere inden for hvert område,
selvom forbindelserne til resten af landet forsvinder.

Nordjylland4,0•Association / Organization

MidtjyllandRegional government office

Vestjylland3,0•Tourist information centerLukket

Sydjylland4,1•Labor union

Fyn3,7•Tourism

Sjælland2,2•Tenant ownershipLukket

HovedstadsområdetRegional government office

Bornholm4,2•Marketing agencyLukket

Dette er en konceptuel opdeling, ikke en dækningsplan eller placering af
konkrete repeatere. \#### Infrastrukturens fysiske opbygning Jeg ville
have tre typer faste installationer:

#### Klasse A -- Mikrorepeater

-   Solcelle.
-   Batteri.
-   LoRa.
-   Meget lavt strømforbrug.
-   Ingen internetforbindelse nødvendig.

#### Klasse B -- Regional node

-   LoRa.
-   Wi-Fi 5 GHz.
-   Raspberry Pi eller tilsvarende.
-   Lokal mailbox.
-   Mulighed for TCP.

#### Klasse C -- Backbone node

-   Flere TCP-forbindelser.
-   DHT.
-   Persistent mailbox.
-   Backup-strøm.
-   Flere netværksinterfaces.

#### Klasse D -- Mobil node

-   LoRa.
-   Wi-Fi.
-   GPS efter behov.
-   Batteri.
-   Carry.
-   Automatisk roaming.

Jeg ville gøre hardwareprofilerne standardiserede, så en kommune, en
frivillig eller en privatperson kan bygge en node uden at skulle forstå
hele protokollen.

## 8. Mobilitet: Her kan MaArSe blive rigtig interessant

Jeg ville faktisk prioritere mobilitet højere, end man normalt gør i
LoRa-projekter.

MaArSe har allerede en interessant simulator med et scenarie, hvor en
bil kører 210 km på E45 med repeatere for hver 15 km.

Men jeg vil understrege, at resultaterne er simulatorresultater og ikke
målinger fra virkelige radiolinks. Jeres egen dokumentation beskriver
netop, at der mangler feltmålinger.

### 8.1 Mobile enheder skal have deres egen routingprofil

En mobil enhed bør ikke behandles som en fast repeater.

Jeg ville indføre: \| Parameter \| Fast repeater \| Mobil node \| \| ---
\| --- \| --- \| \| Rutelevetid \| Lang \| Kort \| \| Position \| Stabil
\| Dynamisk \| \| Relay \| Altid efter konfiguration \|
Situationsbestemt \| \| Batteriforbrug \| Prioriteret lavt \| Mere
fleksibelt \| \| Carry \| Ikke nødvendigt \| Centralt \| \| Naboskab \|
Relativt stabilt \| Konstant ændrende \|

Mobilnoder skal kunne registrere: - At de har mistet en repeater. - At
de har mødt en ny repeater. - At deres geografiske område har ændret
sig. - At de kan levere en besked til en anden region. - At de er på vej
mod en kendt gateway. \### 8.2 Roaming mellem repeatere Jeg ville lave
en funktion, der minder om Wi-Fi-roaming.

Eksempel:

En bil bevæger sig gennem tre dækningsområder. Mobil node Roaming

Repeater A Tidligere

Repeater B Aktiv forbindelse

Repeater C Næste

#### Eksempel på overdragelse mellem repeatere.

Jeg ville ikke vente på, at en besked fejler, før systemet undersøger
nye forbindelser.

Det kan gøres ved at: - Lytte efter naborepeatere. - Gemme SNR og RSSI
over tid. - Have en liste over alternative repeatere. - Skifte til ny
rute ved tilstrækkelig forbedring. - Beholde den gamle rute kortvarigt
som fallback. Det skal dog være kontrolleret, så mobilnoder ikke hele
tiden ændrer rute ved små signaludsving. \### 8.3 To radioer i stedet
for én Her er et mere radikalt forslag.

En mobil beredskabsnode kunne have to radioer: - Radio 1: LoRa til det
almindelige MaArSe-net. - Radio 2: En separat hurtigere forbindelse,
eksempelvis Wi-Fi eller en anden egnet radio. Det giver mulighed for
at: - Modtage LoRa-beskeder konstant. - Kommunikere hurtigt med en
nærliggende node. - Overføre større filer. - Bruge den hurtige
forbindelse som midlertidig backbone. - Beholde LoRa-forbindelsen,
selvom Wi-Fi-forbindelsen forsvinder. Det er især relevant i køretøjer
og mobile kommandostationer.

## 9. Hvad jeg konkret ville ændre i MaArSe-koden

Her er min prioriterede liste. Jeg ville ikke forsøge at implementere
det hele på én gang. P0 -- Før større udbredelse \| Prioritet \|
Funktion \| Hvorfor \| \| --- \| --- \| --- \| \| 1 \| Ekstern
kryptografisk revision \| Sikkerhed og tillid \| \| 2 \| Feltmålinger af
LoRa \| Dokumenteret rækkevidde \| \| 3 \| Robust mailbox-replikering \|
Ingen tab ved gatewayfejl \| \| 4 \| Partition recovery \| Sammenkobling
efter nedbrud \| \| 5 \| Trafikprioritering \| Nødmeldinger skal frem \|
\| 6 \| Routingtest med fejl \| Dokumenteret stabilitet \|

P1 -- Netværksforbedringer \| Prioritet \| Funktion \| Hvorfor \| \| ---
\| --- \| --- \| \| 7 \| Link adaptation \| Hurtigere radioforbindelser
\| \| 8 \| Wi-Fi backbone \| Højere kapacitet \| \| 9 \| Mobility
manager \| Bedre mobile forbindelser \| \| 10 \| Carry i rigtig firmware
\| Transport mellem isolerede områder \| \| 11 \| Regional routing cache
\| Drift uden TCP \| \| 12 \| Automatisk failover \| Ingen manuel
omkonfiguration \|

P2 -- Udvidelser \| Prioritet \| Funktion \| Hvorfor \| \| --- \| --- \|
--- \| \| 13 \| Telefon som selvstændig node \| Mere mobilitet \| \| 14
\| iOS-klient \| Flere brugere \| \| 15 \| Reticulum-gateway \|
Interoperabilitet \| \| 16 \| MeshCore-forbedringer \| Eksisterende
netværk \| \| 17 \| Kort og dækningsmålinger \| Infrastrukturplanlægning
\| \| 18 \| Flere transportinterfaces \| Fleksibilitet \|

Der er nogle særlige detaljer i den nuværende implementering, jeg ville
være opmærksom på:

## 1. Mailbox er endnu ikke fuldt distribueret.

I den tekniske guide beskrives fil-mailboxen som hostbaseret, hvor
afsenderens host skal være online, når modtageren vender tilbage.
Firmware-enhederne har desuden begrænsede buffere, og filoverførsler
ligger ikke i deres flash-mailbox. Det er et af de første steder, jeg
ville forbedre arkitekturen.

## 2. Carry skal testes i rigtig firmware.

Der er en vigtig forskel mellem carry i simulatoren og carry i de
fysiske noder. Jeres sammenligningsdokument nævner selv aktivering af
carry for relay-rollen som en manglende opgave. Jeg ville få
dokumentation og implementering til at stemme overens.

## 3. Wi-Fi-backbone skal kunne fungere som mere end en tunnel.

Jeres ESP32-S3 kan allerede forbinde til en host over Wi-Fi. Det er et
godt udgangspunkt. Men jeg ville sikre, at en node kan fortsætte
selvstændigt, når hosten forsvinder, og at forbindelsen ikke skaber
unødvendig dobbelttrafik.

## 10. Hvad jeg ville gøre anderledes ved TCP-backbone

Jeg synes, jeres nuværende DHT-koncept er godt, men jeg ville
videreudvikle det. \#### Fra simpel DHT til distribueret
netværksdirectory Jeg ville skelne mellem tre ting: \## 1. Identitet:
Hvem er brugeren? \## 2. Reachability: Hvor kan brugeren nås? \## 3.
Storage: Hvem har ansvaret for brugerens offlinebeskeder? Det bør ikke
være samme mekanisme.

Eksempel: \#### Eksempel på distribueret opslag Identity

Public key hash → MaArSe-adresse

Reachability directory

Adresse → Gateway A / Gateway B / lokal LoRa

Mailbox directory

Adresse → Flere ansvarlige storage-noder

Jeg ville desuden tilføje: - Flere DHT-peers, som ikke afhænger af
hinanden. - Replikering af vigtige opslag. - TTL på alle
annonceringer. - Signerede reachability-oplysninger. - Kontrol af
forældede gateways. - Automatisk failover. - Begrænsning af
DHT-trafik. - Mulighed for at have lokale DHT-caches. \#### Brug TCP,
QUIC og UDP hvor det giver mening Jeg ville ikke låse backbone til én
transportprotokol. \| Protokol \| Brug \| \| --- \| --- \| \| TCP \|
Pålidelig forbindelse og eksisterende backbone \| \| QUIC \| Hurtig
genoprettelse, multiplexing og krypteret transport \| \| UDP \| Små
datagrammer, discovery og visse realtidsfunktioner \|

QUIC kunne især være interessant mellem mobile gateways, hvor
forbindelser ændres, og IP-adresser kan skifte.

Det er vigtigt, at I stadig har jeres egen ende-til-ende-kryptering.
QUIC-kryptering må ikke blive en erstatning for MaArSe-identiteter og
applikationssikkerhed.

## 11. Katastrofeberedskab: Hvad skal der til for at kunne stole på MaArSe?

Jeg ville faktisk udvikle en særskilt MaArSe Emergency Profile.

Det skal være en konfiguration, som kan aktiveres på både repeatere,
gateways og mobile enheder.

Den skal blandt andet indeholde: \| Funktion \| Emergency Profile \| \|
--- \| --- \| \| Nødmeldinger \| Højeste prioritet \| \| Almindelig chat
\| Lavere prioritet \| \| Billeder \| Begrænset \| \| Voice \|
Prioriteret efter behov \| \| Store filer \| Kø eller pause \| \|
Position \| Valgfri og adgangsstyret \| \| Repeaterstatus \| Regelmæssig
\| \| Batteristatus \| Regelmæssig \| \| Routing \| Automatisk failover
\| \| Internet \| Ikke påkrævet \|

#### En særlig funktion: Emergency Beacon

Alle noder kunne udsende en lille statuspakke med: - Node-ID. - Rolle. -
Batteriniveau. - Aktuel forbindelse. - Sidste kontakt. - Eventuel
fejlstatus. Men jeg ville ikke sende position for alle noder konstant.
Det kan både belaste radiokanalen og skabe privatlivsproblemer.

Position bør være opt-in eller baseret på en særskilt beredskabsrolle.
\#### En anden vigtig funktion: Emergency Bulletin Jeg ville lave et
system, hvor autoriserede myndigheder eller beredskabsorganisationer kan
udsende signerede meddelelser.

Eksempelvis: - Evakuering af et område. - Lukning af broer. - Forurenet
drikkevand. - Manglende elforsyning. - Opsamlingssteder. -
Hjælpebehov. - Vejr- og oversvømmelsesvarsler. Meddelelserne skal være
signerede og kunne verificeres offline.

Det skal være muligt at distribuere dem via LoRa, TCP og lokale
mailboxes.

En offentlig nødmelding må ikke afhænge af, at en bestemt central server
er online. \#### Radio og lovgivning i Danmark Jeres README angiver
869,525 MHz og 10 % duty cycle. Det er et vigtigt udgangspunkt, men før
en dansk udbredelse skal I have verificeret de gældende frekvensvilkår,
maksimal ERP, antenneforhold, duty cycle og øvrige begrænsninger for den
konkrete radiokonfiguration.

Der er også forskel på at bygge et hobby-netværk og at drive et
kommunikationssystem, som myndigheder skal kunne stole på.

Jeg ville derfor have en beredskabsfaglig og radioteknisk gennemgang,
før det markedsføres som en kritisk kommunikationsløsning.

MaArSe skal være et supplerende kommunikationsnet, ikke en erstatning
for myndighedernes autoriserede beredskabskommunikation.

## 12. Min samlede vurdering

Subjektiv vurdering af projekternes egnethed til de forskellige opgaver
-- ikke målte karakterer. \| Område \| MaArSe \| MeshCore \| Meshtastic
\| Reticulum \| \| --- \| --- \| --- \| --- \| --- \| \| Fast
infrastruktur \| 9 \| 9 \| 7 \| 8 \| \| Mobile brugere \| 8\* \| 6 \| 9
\| 7 \| \| TCP-integration \| 9 \| 4 \| 6 \| 10 \| \|
Offline-kommunikation \| 9\* \| 8 \| 8 \| 9 \| \|
Skalerbarhedspotentiale \| 9\* \| 8 \| 6 \| 9 \| \|
Hardwarefleksibilitet \| 7 \| 9 \| 10 \| 8 \| \| Modenhed \| 3 \| 8 \|
10 \| 8 \| \| Katastrofepotentiale \| 9\* \| 8 \| 8 \| 9 \|

-   MaArSe's karakterer er potentialevurderinger, ikke dokumenteret
    produktionsperformance. Den reelle modenhed er væsentligt lavere end
    hos de etablerede systemer.

#### Hvem skal MaArSe lære af?

MeshCore

Lær af repeaterroller, effektiv airtime-udnyttelse og
infrastrukturadministration.

Meshtastic

Lær af mobilitet, enkel opsætning, hardwareunderstøttelse og
brugeroplevelse.

Reticulum

Lær af transportuafhængighed, identiteter, kryptografi og
netværkstjenester.

MaArSe

Byg videre på kombinationen, men gør stabilitet, dokumentation og
feltmålinger til førsteprioritet.

## 13. Min konkrete udviklingsplan for MaArSe

Hvis det var mit projekt, ville jeg prioritere sådan her: \#### Fase 1
-- Gør det pålideligt \#### Første milepæl - \[ \] Ekstern
sikkerhedsrevision. - \[ \] Test af alle hardwareprofiler. - \[ \]
Måling af rækkevidde og strømforbrug. - \[ \] 72 timers kontinuerlig
drift. - \[ \] Test af 10--50 repeatere. - \[ \] Test af tabte pakker og
gentagne ACK. - \[ \] Automatisk genstart og recovery.

#### Fase 2 -- Gør det robust

-   [ ] Distribueret mailbox.
-   [ ] Regional routing cache.
-   [ ] Emergency Profile.
-   [ ] Prioriterede sendekøer.
-   [ ] Automatisk failover.
-   [ ] Partition Recovery.

#### Fase 3 -- Gør det hurtigt

-   [ ] Transportabstraktion.
-   [ ] Wi-Fi backbone.
-   [ ] QUIC-understøttelse.
-   [ ] Adaptiv LoRa.
-   [ ] Dual-radio mobile enheder.
-   [ ] Intelligent roaming.

#### Fase 4 -- Gør det landsdækkende

-   [ ] Regionalt netværksdirectory.
-   [ ] Kort over noder og faktisk dækning.
-   [ ] Standardiserede repeaterprofiler.
-   [ ] Kommunale og frivillige gateways.
-   [ ] Mobile beredskabsenheder.
-   [ ] Samarbejde med beredskabsorganisationer.

## 14. Den vigtigste arkitektoniske beslutning

Jeg ville gå efter følgende: MaArSe = Hybrid Delay-Tolerant Mesh Network
Ét netværk -- mange transportformer. - LoRa er det robuste
kommunikationslag. - Wi-Fi er det hurtige lokale kommunikationslag. -
TCP/QUIC er det geografiske backbone. - Carry er forbindelsen mellem
isolerede områder. - Mailbox er garantien for asynkron levering. - DHT
er den distribuerede adressebog. - MaArSe Core styrer identitet,
sikkerhed, routing og prioritering.

Det vigtigste princip:

Ingen enkelt repeater, gateway, internetforbindelse eller server må være
nødvendig for, at en lokal region kan fungere.

Det er den arkitektur, jeg mener kan gøre MaArSe interessant langt ud
over hobbybrug. \## 15. Tre ting, jeg gerne vil undersøge nærmere i
selve projektet Jeg har gennemgået jeres offentlige README, tekniske
guide og sammenligningsdokument. Jeg har ikke foretaget en fuld
linje-for-linje-kodeaudit af samtlige Rust-crates og firmwaremoduler.
Der er derfor tre tekniske områder, jeg ville gå dybere med, inden jeg
anbefaler konkrete ændringer i routeren: \## 1. Routingmotoren: Om den
nuværende SNR-baserede ruteudvælgelse kan udvides til en samlet
cost-funktion med airtime, pakketab, latenstid, batteri og mobilitet.
\## 2. TCP/DHT: Om DHT og gateway-routing kan blive fuldt distribueret
og modstandsdygtig over for længere netværkspartitioner. \## 3.
Firmware: Om ESP32-S3 og nRF52840 kan understøtte mere avanceret
routing, carry og dual-transport uden at RAM- og strømforbruget bliver
for højt. Min anbefaling er at begynde med routingmotoren. Den er
fundamentet for næsten alt det andet.

Jeg ville meget gerne tage MaArSe et skridt videre og udarbejde en
konkret teknisk arkitektur for version 1.0 med routingalgoritmer,
pakkeformater, transportinterfaces, failover, mailbox-replikering og en
testplan. Det ville være et egentligt udviklingsgrundlag, som kan
omsættes direkte til opgaver i jeres Rust-projekt.

Vil du have, at jeg også løbende holder øje med udviklingen i MeshCore,
Meshtastic og Reticulum og fremhæver nye funktioner, som MaArSe bør lære
af?
