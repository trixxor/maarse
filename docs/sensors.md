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
| **Inputs** | BME280/BMP280 (temperature, humidity, pressure) on I2C, battery voltage, chip temperature, a digital input pin, an analogue input (ESP32 only), a digital output pin |
| **When** | every N minutes, a value compared with a limit (for N readings in a row, with a separate "back to normal" level so it does not flap), a value changes by more than X, a pin changes |
| **Then** | send readings, send an alarm (with your own text, at most 40 bytes), set an output on (it goes back when the rule clears), change the wake interval while the rule lasts |
| **Limits** | at most 8 inputs and outputs, 8 rules, 3 steps per rule, 64 bytes of alarm texts together, 256 bytes in all; at least 30 s between cycles |

There are no loops and no free-form code: a recipe cannot hang or damage a
node. The designer shows how much of the 256 bytes is used and how much radio
time a day the setup costs (typically 55 s, 0.06 %).

A pin is read once per cycle. An opening that is shorter than the cycle is
missed; the designer warns about it. Use a shorter interval for doors.

## Pins and boards

Only pins that are free on the board are offered.

| Board | Free pins | I2C | Analogue |
|---|---|---|---|
| Seeed XIAO nRF52840 + Wio-SX1262 | D0 (P0.02), D6 (P1.11), D7 (P1.12) | D6 = SDA, D7 = SCL (takes both pins) | none |
| Heltec WiFi LoRa 32 V4 | GPIO 3, 4, 6, 38, 39, 40, 41 | GPIO47 = SDA, GPIO48 = SCL (the OLED has its own bus) | GPIO4 (ADC1, 0 - 2.5 V) |
| RAK WisMesh Tag | none | – | – |

On the Heltec V4 power a sensor from **Ve** (switched, off while asleep) or
3V3. The ESP32 reads the pin plan (I2C, analogue) at boot, so the node
restarts by itself when a new recipe needs a different plan.

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

## How it is built

| Part | Where |
|---|---|
| Recipe format, engine, messages, BME280 maths | `maarse-core/src/sensor.rs` (tests, including the Bosch data sheet example) |
| Node runtime (cycles, owners, acks, sleep) | `firmware/common/src/node/sensor.rs` |
| Flash sector (recipe, owners, state) | `firmware/common/src/storage.rs` (nRF `0xEE000`, ESP32 partition + `0x5000`) |
| Board code | `firmware/nrf/src/sensor.rs`, `firmware/esp/src/sensor.rs`, `firmware/common/src/bme280.rs` |
| Host: history, designer help | `maarse-host/src/sensors.rs`, `maarse-host/src/node/sensors.rs` |
| Web client | `maarse-client/index.html` ("sensors" section) |
| Android | `android/.../Sensors.kt`, `ui/SensorsUi.kt` |

### Recipe, in bytes

```text
version (1) | flags (1) | interval s (u16) | report_to (1) | alarm_to (1)
| n sources (1) | n rules (1) | text length (1) | [channel address (16)]
| sources | rules | text
source = channel (1) | kind (1) | input (1) | a (1) | b (1)
rule   = id (1) | condition (1) | condition data | limit (1, steps of 5 min)
         | n actions (1) | actions
```

`report_to` and `alarm_to` are masks: bits 0-3 are the node's owners, bit 4 is
the recipe's private channel. Kinds: 1 temperature (0.01 °C), 2 humidity
(0.01 %), 3 pressure (Pa), 4 voltage (mV), 5 pin, 6 counter, 7 level (0.01 %),
8 distance (mm); 128 and up are free.

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

## What is not verified

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
- **No counter input, no interrupt wake.** A pin is read per cycle. The nRF52
  has no analogue input in this version.
- **Claim by Bluetooth or from Android is not built.** The first setup needs
  a computer with USB; Android can change a sensor it already owns.
- **The nj01 service** (up to 10 sensors per owner as a public service) is not
  built. Every host stores and graphs the sensors that send to it; forwarding
  to a service and a per-owner quota need owner information in the messages.
- **A device list that revokes a lost phone reaches a sleeping node only after
  its next wake-up** and only if the host that manages it sent it; there is no
  push to a node that has never been set up from this host.
