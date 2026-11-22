# Sensor catalogue

The list of sensors and boards that the sensor designer offers (web client and
Android app). It is data, not code: adding a sensor needs no new app and, for
an I2C chip, no new firmware.

```text
sensors/boards.json        the boards (pins, I2C pins, analogue pins, wake pins)
sensors/catalog/<id>.json  one sensor per file; the file name is the id
sensors/kinds.json         the kinds of measurement: key, number, name, unit and scale
sensors/templates/<id>.json  the starting points (title, one sentence, a first setup)
sensors/catalog.json       everything merged by `python tools/sensor_catalog.py build`
```

**How it gets to people.** The host embeds `sensors/catalog.json`, so it works
offline. When the sensor files change on `main`, `.github/workflows/sensors.yml`
tests them, signs the merged file with the release key and publishes it on the
`sensors` release. A host checks that release about once a day (Settings > Sensor
list, and `GET/POST /api/sensors/catalog/update`), accepts it only if the signature
matches the release key built into it and its version is newer, and keeps it in
its data folder. The clients ask the host, so they show it at once.

## An I2C chip

```json
{
 "id": "sht3x", "name": "SHT30 / SHT31 / SHT35", "group": "Temperature, humidity and pressure",
 "title": "Temperature and humidity",   // what a person asks for, in plain words (shown first)
 "bus": "i2c", "grove": "Grove SHT31, Grove SHT35", "note": "",
 "addrs": [68, 69],            // addresses the chip can have, the usual one first (decimal)
 "measure_ms": 25,             // how long one reading keeps the node awake
 "read": {                     // how to make the chip answer
   "init": [],                 // set-up commands, hex, each 1 to 4 bytes, sent every cycle
   "trigger": "2400",          // hex bytes that start a measurement
   "wait_ms": 20,              // wait after the trigger (at most 10000)
   "ptr": "",                  // register or command written before the read
   "read_len": 6,              // bytes to read (1 to 16)
   "crc": true                 // a Sensirion CRC-8 byte after every two bytes
 },
 "channels": [                 // what the chip measures
   {"name": "Temperature", "kind": "temperature",
    "field": {"bit_off": 0, "bits": 16, "signed": false, "little": false, "mul": 17500, "div": 65535, "add": -4500}}
   // "read": {...} on a channel replaces the shared read (a chip with one register per value)
 ],
 "tests": [                    // REQUIRED: numbers from the data sheet
   {"channel": 0, "answer": "6666938000a2", "value": 25.0}
 ]
}
```

**Write for ordinary people.** `title` is what the sensor does ("Brightness", "Air
quality (CO2)"), `grove` is the name on the box, `note` is one plain sentence (no
addresses, no milliamperes). Anything a person should not have to understand
(an adapter, "any pin", your own chip) gets `"advanced": true` and is shown only
under "More, for tinkerers". The designer hides addresses, calibration and pins
behind "Details".

`value = raw * mul / div + add`, in the unit of the kind with its scale (`0.01 °C`
for temperature: `2500` is 25.00 °C). The tests are run by `cargo test -p
maarse-host sensors::catalog`: every entry must be a valid description, and every
test must give its value. A pull request without tests from the data sheet is not
merged.

Chips that need a calibration table or a long start-up (the BME280, DHT,
ultrasonic) are not described this way: they have a `"driver"` instead of
`"read"`, and work only on a firmware that has that driver.

## A module on one pin

`"bus"` is `analog`, `digital_in` or `digital_out`. See the existing files
(`reed.json`, `soil_moisture.json`, `relay.json`). Only modules that work at 3.3 V.

## Starting points and kinds of measurement

Both live here too, so a new one needs no new app. A **starting point** is a
title, one plain sentence and the first version of a setup (sources and rules, in
the designer's JSON); `cargo test -p maarse-host sensors::catalog` makes a setup
of every one and compiles it. A **kind** gives a measurement its name, unit and
scale (`2500` with scale `100` is `25.00`). Kinds 1 to 11 are the ones the nodes
have always known; a new kind needs a number from **128 up**, which every firmware
accepts (anything between is refused on the node).
