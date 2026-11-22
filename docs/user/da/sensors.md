# Sensorer: vid, hvad der sker derhjemme

En **sensor** er et lille Må:rse-printkort med noget sat på: en temperaturføler
i kælderen, en dørkontakt på postkassen, en føler i et drivhus. Den måler en
gang imellem, fortæller dig værdierne og advarer dig, når noget er galt, fx
"Frost: under 10 °C". Den kører i måneder på et batteri, fordi den sover
mellem målingerne.

## Før du begynder

- Et Må:rse-printkort: **Seeed XIAO nRF52840 med Wio-SX1262-radioen** eller
  **Heltec WiFi LoRa 32 V4**.
- En sensor. **Tilføj sensor eller pin** viser, hvad der virker på dit
  printkort: chips som BME280, SHT31, AHT20, SCD41 (CO2) og BH1750 (lys) og
  mange Seeed **Grove**-moduler (bevægelse, dørkontakt, jordfugtighed, relæ og
  mere). Fire ledninger til printkortet; designeren viser, hvilke pins du skal
  bruge. En chip, der ikke står på listen, kan tilføjes med **Byg din egen**
  ud fra dens datablad.
- En computer med Chrome eller Edge og et USB-kabel til den første opsætning.
- En konto (Indstillinger > Konto og enheder): en sensor tilhører en person.

## Hvad der virker på hvilket printkort

Begge printkort kan køre de samme sensorer; forskellen er, hvor mange du kan
sætte i.

- **Seeed XIAO nRF52840 med Wio-SX1262:** tre frie pins. Sensorer, der bruger
  sensorporten (temperatur, luftfugtighed, CO2, lys, bevægelse), kan alle dele
  ét kabel, men så er der kun plads til én ekstra kontakt eller
  bevægelsessensor. Jord-, lys- og lydsensorer med analog udgang kræver et lille
  adaptermodul (et Grove ADC); designeren fortæller dig det.
- **Heltec WiFi LoRa 32 V4:** syv frie pins og én direkte analog indgang, så
  flere kontakter og én analog sensor kan sættes direkte i.

Virker endnu ikke på nogen af dem: DHT11/DHT22, ultralydsafstand, vandtæt
DS18B20, GPS og partikelsensorer og 5 V-gassensorer. Ingen sensor er prøvet på
rigtigt udstyr endnu: sammenlign den første måling med et termometer.

## Sæt en sensor op

1. Åbn **Noder > Sensorer > Sæt en sensor op** på computeren.
2. Vælg et udgangspunkt, fx **Frostalarm**.
3. Ret sætningerne, hvis du vil. "Når Temperatur er under 10 °C i 2 målinger,
   send en alarm." Testen til højre viser en simuleret dag og hvor meget af
   sensorens hukommelse og radiotid den bruger.
4. Tryk på **Send til sensoren**, vælg **Via USB**, og sæt printkortet i.
   Sensoren tager dig som ejer og får opsætningen.
5. Tag kablet ud. Sensoren sover nu mellem målingerne.

Dine værdier og alarmer vises under **Noder > Sensorer** på computeren og på
telefonen. Tryk på stjernen for at have en sensor øverst.

## Ret den senere

Åbn sensoren og tryk på **Rediger opsætning**. På computeren eller telefonen
trykker du på **Send til sensoren over radioen**. Sensoren henter ændringen
efter sin næste opvågning, så der kan gå op til ét måleinterval.

## Godt at vide

- **Hvem får beskederne:** du vælger pr. sensor, hvem der får målinger, og hvem
  der får alarmer. Op til fire personer kan passe én sensor. Aldrig den
  offentlige kanal.
- **En dør eller en postkasseklap, der åbnes et kort øjeblik,** overses, hvis
  sensoren kun kigger en gang imellem. Brug udgangspunktet **Postkasse**, eller
  slå *Væk sensoren, når denne pin skifter* til: kontakten vækker så sensoren i
  det øjeblik, det sker. Brug en reed-kontakt, der **lukker**, når klappen
  åbnes, mellem pinnen og GND.
- **Ingen måling i timer:** en sensor, der er blevet tavs, vises i gult, fordi
  et fladt batteri er den mest almindelige grund.
- **En pin eller udgang, der skal blive tændt** (et varmerelæ), bør ikke bruges
  med en sovende Heltec V4: udgange slukker, mens den sover.
- **Sat i USB bliver sensoren vågen,** så du kan tale med den. Efter en
  opstart eller nulstilling er den vågen i ti minutter.

## En postkasse

Vælg **Postkasse**. Når der kommer post, og klappen flytter magneten, vækker
kontakten den sovende sensor, som fortæller dig "Post i postkassen". Alarmen
bliver, til du trykker på **Markér som læst** på sensorens side.

**Hvor længe holder batteriet?** Med én 18650-celle (2500 mAh), printkortet i
søvn, post én gang om dagen og en temperatur- og batterirapport to gange om
dagen kan du regne med ca. **to år**, i et godt tilfælde fem, i et dårligt ét.
Det meste af energien går til søvn og til, at cellen selv langsomt mister
ladning, ikke til at sende. Det er et skøn: ingen har målt det på det rigtige
printkort endnu.

Flere tekniske detaljer: [Sensornoder](../../sensors.md).
