# Sensors: know what is going on at home

A **sensor** is a small Må:rse board with something attached: a temperature
sensor in the cellar, a door contact on the post box, a probe in a
greenhouse. It reads now and then, tells you the values, and warns you when
something is wrong, for example "Frost: below 10 °C". It runs for months on a
battery because it sleeps between readings.

## Before you start

- A Må:rse board: **Seeed XIAO nRF52840 with the Wio-SX1262 radio** or
  **Heltec WiFi LoRa 32 V4**.
- A sensor. **Add a sensor or pin** lists what works on your board: chips such
  as the BME280, SHT31, AHT20, SCD41 (CO2) and BH1750 (light), and many Seeed
  **Grove** modules (motion, door contact, soil moisture, relay and more). Four
  wires to the board; the designer shows which pins to use. A chip that is not in
  the list can be added with **Build your own**, from its data sheet.
- A computer with Chrome or Edge and a USB cable, for the first setup.
- An account (Settings > Account and devices): a sensor belongs to a person.

## What works on which board

Both boards can run the same sensors; the difference is how many you can plug in.

- **Seeed XIAO nRF52840 with Wio-SX1262:** three free pins. Sensors that use the
  sensor port (temperature, humidity, CO2, light, movement) can all share one
  cable, but then only one extra switch or motion sensor fits. Soil, light and
  sound sensors with an analogue output need a small adapter module (a Grove
  ADC); the designer tells you.
- **Heltec WiFi LoRa 32 V4:** seven free pins and one direct analogue input, so
  more switches and one analogue sensor can be plugged straight in.

Not working yet on either board: DHT11/DHT22, ultrasonic distance, waterproof
DS18B20, GPS and particle sensors, and 5 V gas sensors. No sensor has been
tried on real hardware yet: check the first reading against a thermometer.

## Set up a sensor

1. On the computer open **Nodes > Sensors > Set up a sensor**.
2. Pick a starting point, for example **Frost alarm**.
3. Change the sentences if you like. "When Temperature is below 10 °C for 2
   readings, send an alarm." The test on the right shows a simulated day and
   how much of the sensor's memory and radio time it uses.
4. Press **Send to the sensor**, choose **By USB**, and plug the board in.
   The window lists **What changes** (new, changed, removed and unchanged
   rules) compared with the setup saved on this computer. **Save a copy
   first** writes the setup to a file; **Open file** and **Save file** at the
   top of the designer do the same at any time. The sensor takes you as its
   owner and gets the setup.
5. Unplug the cable. The sensor now sleeps between readings.

Your values and alarms show up under **Nodes > Sensors** on the computer and
on the phone. Press the star to keep a sensor at the top.

## Change it later

Open the sensor and press **Edit setup**. On the computer or on the phone,
press **Send to** and the sensor's name. The phone shows how many packets
have been sent. The sensor fetches the change after its next wake-up, so it
can take up to one reading interval.

## Good to know

- **Who gets the messages:** you choose, per sensor, who gets readings and
  who gets alarms. Up to four people can look after one sensor. Never the
  public channel.
- **A door or a mailbox flap that opens for a short moment** is missed if the
  sensor only looks now and then. Use the **Mailbox** starting point, or switch
  on *Wake the sensor when this pin changes*: the switch then wakes the sensor
  the moment it happens. Use a reed switch that **closes** when the flap opens,
  between the pin and GND.
- **No reading for hours:** a sensor that has gone quiet is shown in amber,
  because a flat battery is the most common reason.
- **A pin or output that must stay on** (a heater relay) should not be used
  with a sleeping Heltec V4: outputs switch off while it sleeps.
- **Plugged into USB the sensor stays awake**, so you can talk to it. After
  a power-up or reset it stays awake for ten minutes.

## A mailbox

Pick **Mailbox**. When post arrives and the flap moves the magnet, the switch wakes
the sleeping sensor, which tells you "Post in the mailbox". The alarm stays
until you press **Mark as read** on the sensor page.

**How long does the battery last?** With one 18650 cell (2500 mAh), the board sleeping,
post arriving once a day and a temperature and battery report twice a day, expect
about **two years**, in a good case five, in a bad case one. Most of the energy
goes to sleeping and to the cell slowly losing charge by itself, not to sending.
That is for the XIAO nRF52840 kit. The Heltec V4 uses more power while it sleeps and
sending, and nobody knows how much yet: for a mailbox on a battery use the XIAO kit.
This is an estimate: nobody has measured it on the real board yet.

More technical details: [Sensor nodes](../sensors.md).
