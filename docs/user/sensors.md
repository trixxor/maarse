# Sensors: know what is going on at home

A **sensor** is a small Må:rse board with something attached: a temperature
sensor in the cellar, a door contact on the post box, a probe in a
greenhouse. It reads now and then, tells you the values, and warns you when
something is wrong, for example "Frost: below 10 °C". It runs for months on a
battery because it sleeps between readings.

## Before you start

- A Må:rse board: **Seeed XIAO nRF52840 with the Wio-SX1262 radio** or
  **Heltec WiFi LoRa 32 V4**.
- A sensor such as a **BME280** (temperature, humidity, pressure), four wires
  to the board. The designer shows which pins to use.
- A computer with Chrome or Edge and a USB cable, for the first setup.
- An account (Settings > Account and devices): a sensor belongs to a person.

## Set up a sensor

1. On the computer open **Nodes > Sensors > Set up a sensor**.
2. Pick a starting point, for example **Frost alarm**.
3. Change the sentences if you like. "When Temperature is below 10 °C for 2
   readings, send an alarm." The test on the right shows a simulated day and
   how much of the sensor's memory and radio time it uses.
4. Press **Send to the sensor**, choose **By USB**, and plug the board in.
   The sensor takes you as its owner and gets the setup.
5. Unplug the cable. The sensor now sleeps between readings.

Your values and alarms show up under **Nodes > Sensors** on the computer and
on the phone. Press the star to keep a sensor at the top.

## Change it later

Open the sensor and press **Edit setup**. On the computer or on the phone,
press **Send to the sensor over the radio**. The sensor fetches the change
after its next wake-up, so it can take up to one reading interval.

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

More technical details: [Sensor nodes](../sensors.md).
