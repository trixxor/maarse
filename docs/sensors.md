# Sensor nodes

A sensor node is a Må:rse node that reads things (temperature, humidity,
pressure, a pin, a voltage ...), sends them to its **owners**, raises alarms,
and can be changed from far away. The setup is made in a **designer** (PC web
client and Android app) out of small sentences such as "when Temperature is
below 10 °C for 2 readings, send an alarm". The designer turns it into a small
**recipe** that the node runs by itself, so alarms work even when your phone
or PC is off.

Status of this document: **built and tested in the working copy** (core,
host and both user interfaces, with unit tests and screenshots on a PC and on
Android). **Not yet verified on hardware:** the firmware compiles for the XIAO
nRF52840 and the Heltec V4, but sleep current, the BME280 on a real board,
and the deep-sleep wake on the ESP32 have not been measured or run. See
"What is not verified" at the end.

## What a person sees

- **Nodes > Sensors** (web client and Android): your sensors with their last
  values, alarms in red, quiet sensors in amber, favourites first.
- **Set up a sensor**: pick a template (frost alarm, door or post box, weather
  station, battery watch, empty), adjust the sentences, watch a simulated day,
  and send it to the node.
- **First setup is over USB from a computer** (Chrome or Edge): the node takes
  your account as its owner and gets the recipe. After that, changes can be
  sent over the radio from the PC or the phone; the node fetches them after
  its next wake-up.
- **Owners are people** (accounts), up to four, equal. Each gets the
  readings and alarms you choose. A recipe may also name one private channel
  (never the public one).

## Building blocks of a recipe

| Part | Choices |
|---|---|
| **Inputs** | a sensor from the catalogue (below), a sensor you describe yourself, battery voltage, chip temperature, a digital input pin, an analogue input (Heltec V4 only; elsewhere through an ADS1115), a digital output pin |
| **When** | every N minutes, a value compared with a limit (for N readings in a row, with a separate "back to normal" level so it does not flap), a value changes by more than X, a pin changes |
| **Then** | send readings, send an alarm (with your own text, at most 40 bytes), set an output on (it goes back when the rule clears), change the wake interval while the rule lasts |
| **Limits** | at most 8 inputs and outputs, 8 rules, 3 steps per rule, 64 bytes of alarm texts together, 256 bytes in all; at least 30 s between cycles |

There are no loops and no free-form code: a recipe cannot hang or damage a
node. The designer shows how much of the 256 bytes is used and how much radio
time a day the setup costs (typically 55 s, 0.06 %).

A pin is normally read once per cycle, so an opening that is shorter than the
cycle is missed. For doors, buttons and mailboxes switch on **Wake the sensor
when this pin changes** for the pin: the pin then wakes the node from sleep
the moment it changes, and a wake by the pin means that something happened.
The designer warns if a pin rule has neither a wake pin nor a short interval.

## Sensor catalogue, Grove modules and your own sensors

The designer's **Add a sensor or pin** list comes from the host
(`GET /api/sensors/catalog`), so the web client and the Android app always
offer the same boards and sensors. A sensor only shows if the chosen board
can run it.

**The catalogue is data, not code.** It lives in the repository's `sensors/`
folder (one JSON file per sensor, `boards.json`; see `sensors/README.md`), is
merged into `sensors/catalog.json` by `tools/sensor_catalog.py build`, and is
embedded in the host. It is not in the apps and not in the firmware:

- **Apps and web client** only draw what the host sends: the sensors, the boards, the
  kinds of measurement with their units, and the starting points. Nothing about a sensor is
  built into them.
- **Nodes** need no update for a new I2C chip: the recipe carries the
  description of how to read it (see below).
- **Hosts update it from GitHub.** When files in `sensors/` change on `main`,
  `.github/workflows/sensors.yml` runs the tests, signs the merged file with the
  release key (the one that signs the firmware and the Windows updates) and
  publishes it on the rolling `sensors` release (a pre-release). A host looks
  there about once a day (switch off or ask now under **Add a sensor or pin >
  Check for updates**, or `POST /api/sensors/catalog/update`). It accepts a list
  only if the signature matches the release key built into it (a build without
  the key never updates), the file matches the signed checksum, every description
  fits together, every test from a data sheet gives its number, and the version is
  newer than the one in use. The accepted files are kept in
  `<data>/sensor-catalog/` and checked again at every start. The Android app has
  no `curl`: it downloads the three files itself and posts them to the host,
  which does the same checks.
- **A catalogue entry that needs a firmware driver** (`"driver"` instead of a
  description, today only the BME280) works only when the firmware has it.
  Other chips never need new firmware.

**Plain words first.** The designer is for ordinary people: the list shows what a
sensor does ("Temperature and humidity", "Brightness") with the name on the box
below it, the chip name only as a hint. Technical choices (I2C address, analogue
adapter, calibration in millivolts, pins, "Build your own") sit behind **Details**
on a sensor and **More, for tinkerers** in the list. When a sensor needs extra
hardware, the designer says so in a sentence (a soil sensor on a board without
analogue input: "plug it into a Grove ADC module, and the module into the sensor
port"). The starting points (frost alarm, room climate, air quality, motion alarm,
mailbox, ...) are the simplest way in.

What is in it now:

- **I2C chips:** BME280/BMP280, SHT30/31/35, SHT40/41/45, AHT20/AHT21/DHT20,
  BH1750 (light), MCP9808, TMP117, MCP9600 (thermocouple), SCD41 (CO2; 5 s awake
  per reading), ADS1115 (four analogue inputs), ADXL345 (motion), MLX90614
  (infrared thermometer). Each entry must carry tests with numbers from the data
  sheet; CI refuses an entry whose test fails.
- **Grove modules on one pin:** PIR, button, touch, reed switch, tilt, vibration,
  flame, Hall, infrared obstacle, relay, LED, buzzer, and the analogue ones that
  work at 3.3 V (soil moisture, water, light, sound, rotary knob, UV). An analogue
  module can be shown as a level: a **calibration** turns millivolts into a
  percentage (`dry reads 2600 mV = 0 %`, `wet 1200 mV = 100 %`) before the rules
  and reports see the value. Calibrate every module: the numbers in the list are
  only a start.
- **Your own I2C sensor** ("Build your own"): the designer asks for the
  address, the set-up bytes, the start-measurement bytes, the wait, the
  register, how many bytes to read, whether a check byte follows every two
  bytes (Sensirion CRC-8), and which bits are the number and how to scale it.
  It is the same description a catalogue entry has. A description cannot hang or
  damage the node, but the designer cannot try the chip: check the numbers against
  the data sheet.
- **What a new catalogue cannot do:** a sensor that needs a firmware driver (DHT, ultrasonic), a new board,
  or a new *kind of screen* in the designer. A new kind of measurement is fine (id 128 or up).
- **Not supported yet:** DHT11/DHT22 and other single-wire sensors, ultrasonic
  distance, 1-Wire (DS18B20), serial sensors (GPS, particles), VL53L0X, and
  5 V modules such as the MQ gas sensors (the boards give 3.3 V).

## Wake by pin (a mailbox)

A **wake pin** wakes a sleeping node when its value becomes 1. An Edge rule
("when the pin reads 1") then fires at once, whatever the pin looked like at the
last cycle and even after hours of sleep or a restart. The wake cycle takes the
pin as 1 even if the flap has closed again by then. The timer still wakes the
node at its interval (for example once an hour to report the battery).

- **Wiring:** use a *normally open* reed switch that **closes** when the flap
  opens, between the pin and GND. The designer sets the pin to wake on a low
  level, held high by a pull-up. A switch that is closed while nothing happens
  would lose about 70 µA through the pull resistor.
- **nRF52840:** any free pin (D0, D6, D7); the chip wakes on the pin's PORT
  event in System ON.
- **ESP32-S3 (Heltec V4):** only the RTC pins GPIO 3, 4 and 6. The pin is armed
  before deep sleep and the node learns at boot which pin woke it.
- A pin that is still at its wake level when the node goes to sleep (a flap
  that is stuck open) is not armed, because it would wake the node at once,
  every time; the next timer wake looks again.
- A mailbox alarm never clears by itself. Press **Mark as read** on the sensor
  page after you have emptied the box.

## Pins and boards

Only pins that are free on the board are offered.

| Board | Free pins | I2C | Analogue |
|---|---|---|---|
| Seeed XIAO nRF52840 + Wio-SX1262 | D0 (P0.02), D6 (P1.11), D7 (P1.12) | D6 = SDA, D7 = SCL (takes both pins) | none yet (use an ADS1115) |
| Heltec WiFi LoRa 32 V4 | GPIO 3, 4, 6, 38, 39, 40, 41 | GPIO47 = SDA, GPIO48 = SCL (the OLED has its own bus) | GPIO4 (ADC1, 0 - 2.5 V) |
| Seeed XIAO ESP32S3 + Wio-SX1262 | D0 to D7 (GPIO 1 to 6, 43, 44) | D4 = SDA, D5 = SCL | D0 to D5 (not ready: the firmware cannot read sensors yet) |
| RAK WisMesh Tag | none | – | – |

The radio of the XIAO nRF52840 kit uses D1 to D5 and D8 to D10 (Meshtastic's
`seeed_xiao_nrf52840_kit` variant lists the same pins). That is why only three
pins are free there. The XIAO ESP32S3 kit puts its radio on D8 to D10 and the
connector under the board, which leaves D0 to D7 free.

**Grove Shield for XIAO** (3.3 V on all connectors): connectors 1 to 3 are
D0/D1, D1/D2, D2/D3, connectors 4 and 5 are I2C on D4/D5, connector 6 is UART
on D6/D7, connectors 7 and 8 are D8/D9 and D9/D10. On the nRF kit only
connector 6 can be used (an I2C Grove cable fits it: the wires sit in the same
places), because everything else is on the radio's pins. We have not checked
whether the shield and the Wio board can be stacked on one XIAO.

**A repeater with a sensor.** A repeater is powered all the time, so turn off
**Sleep between readings**: the node then forwards packets as usual and the
sensor reports at its interval. Not tested on a board.

On the Heltec V4 power a sensor from **Ve** (switched, off while asleep) or
3V3. The ESP32 reads the pin plan (I2C, analogue) at boot, so the node
restarts by itself when a new recipe needs a different plan.

## What each board supports

The catalogue has 32 entries: 12 I2C chips (about 18 different chips, because one
entry covers a family such as SHT30/31/35) and 20 Grove modules on one pin
(6 analogue, 10 digital in, 4 digital out). Both boards can run every entry. What
limits you is pins, not the list.

| | XIAO nRF52840 + Wio-SX1262 | Heltec V4 (ESP32-S3) |
|---|---|---|
| I2C chips (12 entries) | all, on D6/D7 | all, on GPIO47/48 |
| Digital Grove modules (10 in, 4 out) | all 14, but only **3 free pins** (D0, D6, D7); the I2C bus takes D6 and D7, so **one** pin (D0) is left | all 14, 7 free pins |
| Analogue Grove modules (6) | no analogue pin: only through an **ADS1115** adapter | 1 direct pin (GPIO4, up to 2.5 V), more through an ADS1115 |
| Wake by pin | any free pin | GPIO 3, 4 and 6 only |
| Sensors at once | at most 8 in a recipe; with I2C only one digital pin | at most 8 in a recipe |
| XIAO ESP32S3 + Wio-SX1262 | listed but greyed out: the firmware cannot read sensors yet (8 free pins and 6 analogue inputs once it can) | – |

In practice: on the nRF kit you can have temperature, CO2 and light together (all
on one I2C cable) but only one PIR or door contact beside them. Analogue modules
(soil moisture, light, sound) need an ADS1115 on that board, and the designer
says so in a sentence. A direct analogue pin on D0 is planned.

**Supported:** BME280/BMP280, SHT30/31/35, SHT40/41/45, AHT20/21/DHT20, BH1750,
MCP9808, TMP117, MCP9600, SCD41, ADS1115, ADXL345, MLX90614; PIR, button, touch,
reed, tilt, vibration, flame, Hall, infrared obstacle, relay, LED, buzzer, and
analogue soil moisture, water, light, sound, rotary knob and UV.

**Not supported on any board yet:** DHT11/DHT22 and other single-wire sensors,
ultrasonic distance, 1-Wire (DS18B20), serial sensors (GPS, particles), VL53L0X,
and 5 V modules such as the MQ gas sensors (the boards give 3.3 V).

**Not verified:** no chip has been tried on a real sensor and no recipe has run
on the physical boards. The maths of every chip passes the example numbers from
its data sheet, but compare the first reading with a thermometer (or similar)
before relying on it.

## Sleep

A recipe can let the node sleep between cycles (default on).

- **nRF52840:** the node keeps its RAM and wakes by its own timer. The
  SX1262 goes to sleep, Bluetooth, USB and the 32 MHz crystal stay off. On
  battery with a sleeping recipe the board starts without USB and Bluetooth;
  plug in USB and press reset to talk to it.
- **ESP32-S3:** the chip deep sleeps and the wake restarts the firmware. What
  it must remember (which alarms are active, outputs, unsent readings) is kept
  in flash, written only when it changes. A timer wake runs one cycle without
  Bluetooth, Wi-Fi or the display. After a power-up or reset the node stays
  awake for 10 minutes (and while a client is connected), so there is time to
  set it up. The RTC timer keeps the clock through the sleep.
- Between cycles the node does not forward packets. A sensor is a leaf, not a
  repeater.

Wake time is set by the interval (for example 60 minutes for a wake every
hour). Commands for a sleeping node wait at a post office; the node asks for
its mail about once an hour.

## Battery life: a mailbox on one 18650

**An estimate, not a measurement.** The sleep current is the biggest unknown (see
"What is not verified"), so three cases are shown. Measure with an ammeter before
promising anything.

The case: a sensor on the XIAO nRF52840 + Wio-SX1262 in sleep mode, one 18650 of
2500 mAh. The flap opens once a day (a wake by the reed switch sends "post in the
mailbox"), and a timer wake every 12 hours sends temperature and battery level.
That is **3 wake-ups a day**. Each wake sends two messages: the reading or alarm
(about 100 bytes, 0.59 s on air at SF9/125 kHz) and the hourly mail check
(about 0.4 s), then stays awake at least 8 s to hear the acknowledgement.

| | Optimistic | Realistic | Pessimistic |
|---|---|---|---|
| Sleep current (whole board) | 10 µA | 30 µA | 100 µA |
| Awake time per wake-up | 8 s at 7 mA | 12 s at 8.5 mA | 40 s at 8.5 mA (no ack) |
| Transmit power, current | 14 dBm, 60 mA | 22 dBm, 118 mA | 22 dBm, 118 mA |
| Resends | none | 20 % | 50 % |
| Energy per wake-up | 32 µAh | 67 µAh | 143 µAh |
| Messages, 3 a day | 0.10 mAh | 0.20 mAh | 0.43 mAh |
| Sleep, 24 h | 0.24 mAh | 0.72 mAh | 2.4 mAh |
| **Node per day** | 0.34 mAh | 0.92 mAh | 2.8 mAh |
| Usable part of the cell | 90 % | 85 % | 75 % (cold) |
| Self-discharge of the cell | 1 %/month | 2 %/month | 3 %/month |
| **Life, node only** | about 18 years | about 6 years | about 1.8 years |
| **Life with self-discharge** | **about 5 years** | **about 2 years** | **about 1 year** |

What this says:

- **The messages hardly matter.** Three wake-ups a day cost 0.1 to 0.4 mAh. Sleep
  current and the cell's own self-discharge (1.7 mAh a day for 2500 mAh at 2 %
  a month) use far more. A good low-self-discharge cell matters more than the
  radio settings.
- **Realistically two years**, with a range from one year to five.
- **Cheap changes that help:** transmit at 14 dBm instead of 22 (roughly halves
  the radio energy, if the range allows it), ask for mail once a day instead of
  at every wake (`MAIL_EVERY` is one hour), and shorten the 8 s wait.
- **The battery divider** on the XIAO is on all the time (the firmware must keep
  it on): about 2 to 3 µA of the sleep current. The battery task wakes every 60 s
  to measure; that is under 1 µA on average.
- **Heltec V4 (ESP32-S3) is different: the table above does not apply to it.** See below.
- A normally-open reed switch that closes when the flap opens costs nothing while
  the flap is shut. A switch that is closed all the time loses about 70 µA.
- Cold lowers the capacity of the cell: a mailbox in a Danish winter loses
  perhaps 10 to 20 % (the pessimistic column).

### The same mailbox on the Heltec V4 (ESP32-S3)

The ESP32 deep sleeps and **restarts the firmware at every wake** (about 2 s at
roughly 40 mA), stays awake about 10 s at about 30 mA (CPU and radio receiving),
and transmits at much higher current: the SX1262 plus the board's power amplifier
(about 220 mA at 22 dBm, from the board profile in `firmware/esp/src/main.rs`).
One wake-up costs about **180 µAh**, against about 67 µAh on the nRF kit
(realistic case), so the messages cost 0.54 mAh a day instead of 0.20.

The real unknown is the **sleep current of the whole board**: one source says
under 20 µA, another measured about 12 mA in deep standby (the OLED, the power
amplifier or the USB chip may stay powered). Same cell (2500 mAh, 85 % usable,
2 % a month self-discharge, 3 wake-ups a day):

| Sleep current of the board | Node per day | Life, node only | Life with self-discharge |
|---|---|---|---|
| 20 µA (the optimistic claim) | 1.0 mAh | about 5.7 years | **about 2 years** |
| 100 µA | 2.9 mAh | about 2 years | about 1.3 years |
| 1 mA | 25 mAh | about 3 months | about 3 months |
| 12 mA (the pessimistic measurement) | 290 mAh | about 1 week | about 1 week |

So the Heltec V4 can reach about the same two years as the nRF kit **only if its
real sleep current is near 20 µA**. Until someone measures it, treat it as
mains- or solar-powered, or expect days to months on a cell. The XIAO nRF52840
kit is the board to use for a battery-only mailbox.

## How it is built

| Part | Where |
|---|---|
| Recipe format, engine, messages, BME280 maths | `maarse-core/src/sensor.rs` (tests, including the Bosch data sheet example) |
| Node runtime (cycles, owners, acks, sleep) | `firmware/common/src/node/sensor.rs` |
| Flash sector (recipe, owners, state) | `firmware/common/src/storage.rs` (nRF `0xEE000`, ESP32 partition + `0x5000`) |
| Board code | `firmware/nrf/src/sensor.rs`, `firmware/esp/src/sensor.rs`, `firmware/common/src/bme280.rs`, `firmware/common/src/i2c_sensor.rs` (the one reader for every described I2C sensor) |
| I2C description, bit extraction, CRC | `maarse-core/src/sensor/catalog.rs` |
| The catalogue (data) | `sensors/` (`catalog/*.json`, `boards.json`, `kinds.json`, `templates/*.json`), merged by `tools/sensor_catalog.py` |
| Catalogue in the host, tests, board checks | `maarse-host/src/sensors/catalog.rs` |
| Signed updates from GitHub | `maarse-host/src/sensors/catalog_update.rs`, `.github/workflows/sensors.yml` |
| Host: history, designer help | `maarse-host/src/sensors.rs`, `maarse-host/src/node/sensors.rs` |
| Web client | `maarse-client/index.html` ("sensors" section) |
| Android | `android/.../Sensors.kt`, `ui/SensorsUi.kt` |

### Recipe, in bytes

```text
version (1) | flags (1) | interval s (u16) | report_to (1) | alarm_to (1)
| n sources (1) | n rules (1) | text length (1) | [channel address (16)]
| sources | rules | text
source = channel (1) | kind (1) | input (1) | a (1) | b (1)
tail   = count (1) | (tag (1) | length (1) | data)*   only when flag 0x04 is set
         tag 1 = how to make an I2C chip answer (address, set-up, trigger, wait, register, bytes, check bytes)
         tag 3 = how to take a number out of the answer (bits, sign, byte order, mul, div, add; 15 bytes)
         tag 2 = value map (source, in lo/hi, out lo/hi as i16)
         the sources of one chip share a tag 1; an I2C source has input 8, a = number of its tag 1, b = number of its tag 3
rule   = id (1) | condition (1) | condition data | limit (1, steps of 5 min)
         | n actions (1) | actions
```

`report_to` and `alarm_to` are masks: bits 0-3 are the node's owners, bit 4 is
the recipe's private channel. Kinds: 1 temperature (0.01 °C), 2 humidity
(0.01 %), 3 pressure (Pa), 4 voltage (mV), 5 pin, 6 counter, 7 level (0.01 %),
8 distance (mm), 9 light (lx), 10 CO2 (ppm), 11 acceleration (mg); 128 and up are free.
Inputs: 1 battery, 2 chip temperature, 3 pin in, 4 analogue, 5 BME280, 6 counter, 7 pin out,
8 I2C chip (the description is in the tail: at most 4 ways of reading and 8 ways of taking a number per recipe).

### Messages (content type `0x0E`)

```text
READINGS (1): seq (1) | n (1) | n x ( age s (u16) | m (1) | m x ( channel | kind | value (zigzag varint) ) )
ALARM    (2): rule (1) | flags (1, bit 0 = raised) | age s (u16) | channel | kind | value | text
```

Readings carry their age, not a time, so a node without a clock can send
them, and a reading that was not acknowledged is sent again in the next
message. Each owner gets one message to the device of theirs that was heard
most recently. The ack is the ordinary end-to-end one; mail kept by a post
office is acknowledged by a delivery receipt (see the protocol specification,
section 8.1). A private channel gets a line of text instead, because the
members read it as chat.

### Owners and commands

A node keeps each owner's master public identity and the latest signed device
list (see `docs/maybe/multi-device.md`). A device in the list is an owner's
device: it may send commands, and the counter against replays is the person's,
shared by all their devices. A newer list sent to the node replaces the old
one, which is also how a lost phone is removed. Owners may add and remove
other owners.

Commands are `Command::Sensor` (companion `0x39`) with an operation byte:
get/write recipe (in pieces, checked with a CRC), read now, status, add or
remove an owner. The answers are `Event::Sensor` (`0xbd`). A command for
another node goes to it as a control command (so the sender must be an owner).
The host adds its own operations (history, simulation, naming); see
`maarse-core/src/sensor.rs` (`op` and `error`).

### The host's HTTP API (used by both user interfaces)

```text
GET  /api/sensors                      list with last values and alarms
GET  /api/sensors/catalog              boards and sensors the designer offers
GET  /api/sensors/catalog/update       which list is in use, when it was checked
POST /api/sensors/catalog/update       {"check":true}, {"auto":bool} or a signed list {"catalog","sums","sig"}
GET  /api/sensors/series?node=&chan=&from=&to=&max=
GET  /api/sensors/alarms?node=
GET  /api/sensors/setup?node=   PUT    the designer's document
POST /api/sensors/meta                 name, favourite
POST /api/sensors/forget
POST /api/sensors/compile              setup -> recipe, size, checks
POST /api/sensors/decompile            recipe -> setup
POST /api/sensors/simulate             setup + scenario -> a simulated day
```

The designer works on the setup JSON; the host builds and checks the recipe
and runs the same engine the nodes run, so the test in the designer shows what
the node will do. A host keeps at most 50 sensors and drops a sensor that
sends more than 400 readings an hour.

Two settings in the setup JSON are not part of the recipe:

- `rules[].on`: `false` keeps a rule in the designer but leaves it out of the
  recipe (the sensor never gets it, and it does not count in the limits). A
  missing value means on. A change reaches the sensor when the setup is sent.
- `quiet`: `{"on": true, "min": 120}`, the host's "node went quiet" rule.
  The host lists the sensor as `quiet` (and shows it in amber and under
  Alarms) when it has heard nothing for `min` minutes (5 to 10080). It runs on
  the host, so it needs the host to be running and the setup to be saved on the
  host (sending the setup does that). Without the setting a sensor is quiet
  after two hours, as before. The designer warns when `min` is shorter than
  two reading intervals. There is no push notification yet: the state shows in
  the lists.

A setup can be shared as one line of text, `maarse-sensor:` followed by the
setup JSON in base64url (the channel is left out). A setup opened from such a
link or from a file never keeps its recipients or channel: it starts with you
as the only receiver, so a setup from someone else cannot send your readings
elsewhere. The designer reads the setup of another of your sensors the same
way (the copy of the setup the host saved).

## What is not verified

- **The catalogue update has not run against GitHub.** The workflow and the signature checks are tested, but the first publication on the `sensors` release (needs the repository secret `MAARSE_SIGN_KEY_B64` and variable `MAARSE_RELEASE_PUBKEY`) and a host fetching it have not happened. A development build has no release key and never updates.
- **The catalogue chips have not been tried on real chips.** Their maths passes data sheet example values, but no SHT3x, AHT20, SCD41, ADS1115 and so on has been connected. Treat the first reading as a check against a thermometer.
- **No hardware test yet.** The firmware builds for the XIAO nRF52840 and the
  Heltec V4. The BME280 driver, the pin tables, the SX1262 sleep and the
  nRF52 low-power start have not been run on a board. The only reference for
  the BME280 maths is the data sheet example (passes).
- **Sleep current is not measured.** Documents conflict for the Heltec V4: the
  product page says under 20 µA, Heltec's own test measured about 12 mA in
  deep standby. Measure with an ammeter before promising battery life. The
  XIAO with the Wio-SX1262 is reported at a few µA by users; the SPI pins
  must be idle and the RF switch pin low, which the firmware does, but this is
  unmeasured too.
- **ESP32 outputs do not hold their level in deep sleep.** Do not use an
  output that must stay on (a heater relay) with a sleeping Heltec. The
  designer warns about it.
- **ESP32 deep sleep forgets some state:** a "change by" baseline and the time
  of the last "every" report. Such rules report at every wake. Alarms, their
  "back to normal" state, outputs and unsent readings are kept.
- **No counter input.** A wake pin gives one event per wake, not a count. The
  nRF52 has no analogue input in this version.
- **Pin wake is built and compiles for both boards, but not run on hardware.**
  Check the wiring and the wake with a real reed switch before you rely on it.
- **Claim by Bluetooth or from Android is not built.** The first setup needs
  a computer with USB; Android can change a sensor it already owns.
- **The nj01 service** (up to 10 sensors per owner as a public service) is not
  built. Every host stores and graphs the sensors that send to it; forwarding
  to a service and a per-owner quota need owner information in the messages.
- **A device list that revokes a lost phone reaches a sleeping node only after
  its next wake-up** and only if the host that manages it sent it; there is no
  push to a node that has never been set up from this host.
