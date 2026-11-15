# Field test: range, loss and power

Every range and battery comparison so far comes from the simulator (`maarse-sim`, log-distance path loss with exponent 3, see [GUIDE.md](GUIDE.md#simulator)). This procedure gets real numbers from two nodes: how far a direct link reaches at SF7–SF12 in town and in open country, how many packets are lost along the way, and how much current a node draws as beacon, receiver or idle.

The firmware has a **range test mode** for this. One node is the **beacon**: it stays at a fixed spot and sends a numbered packet every few seconds. The other is the **receiver**: it comes with you and reports every beacon it hears to the web client, along with RSSI, SNR and SF. The web client adds the phone's GPS position and the distance to the beacon. Loss is counted from gaps in the sequence numbers.

- [What you need](#what-you-need)
- [Before you leave (at the desk)](#before-you-leave-at-the-desk)
- [Setting up the beacon](#setting-up-the-beacon)
- [The drive](#the-drive)
- [What to log](#what-to-log)
- [Power measurement](#power-measurement)
- [Reading the results](#reading-the-results)
- [Results template](#results-template)
- [Troubleshooting](#troubleshooting)
- [Technical details](#technical-details)

## What you need

- **Two nodes with this firmware.** For example the XIAO nRF52840 + Wio-SX1262 and the Heltec V4. Both must be flashed from the same build. Commands are in [GUIDE.md](GUIDE.md#firmware-status-and-what-is-missing). In short:
  - XIAO: `cargo build --release --features board-xiao-nrf52840` in `firmware/nrf`, then `cargo run --release -p maarse-uf2 -- firmware/nrf/target/thumbv7em-none-eabihf/release/maarse-fw-nrf dist/maarse-xiao-nrf52840.uf2` and `python scripts/flash-nrf.py COM7 dist/maarse-xiao-nrf52840.uf2`. Press reset once afterwards so USB comes back.
  - Heltec V4: `cargo +esp build --release --features board-heltec-v4` in `firmware/esp`, then `espflash flash --port COM6 --chip esp32s3 --partition-table firmware/esp/partitions.csv --erase-parts otadata <elf>`. The port must be free: a maarse-host using the board as its modem (`--serial COM6`) has to be stopped first.
- **A phone for the receiver.** Use the Må:rse Android app (APK from `python tools/build_apk.py`, which now asks for location permission) or Chrome on Android with the client opened over **https** or from `localhost`. Location and Bluetooth only work in a secure context. A plain `http://192.168.x.x:8080` address from maarse-host does not qualify. A laptop works too, connected over USB, but it has no GPS: you then type in the distance per point yourself.
- **Power for the beacon.** A battery or power bank for at least the length of the drive. A USB power bank may switch off at low current: the XIAO draws only a few mA, so use one without auto-off, or a LiPo on the battery pads.
- **Optional:** a USB power meter (inline USB-A/C "USB doctor") for the power part, tape or a mast to put the beacon up high, and a notebook.

## Before you leave (at the desk)

1. **Same radio settings on both.** Open each node in the web client (USB or Bluetooth) → **Radio**. Frequency, bandwidth, coding rate and preamble must match. The SF only matters for runs without SF cycling. The defaults are 869.525 MHz, 125 kHz, CR 4/5, preamble 16 and 10 % duty cycle (the 869.4–869.65 MHz sub-band allows 10 % and 500 mW ERP).
2. **Transmit power.** Write down the TX power of the beacon. On the XIAO, 22 dBm is the power at the antenna connector. On the Heltec V4, the setting is the power *into* its PA: the default is 10 dBm, which comes out at about 17–20 dBm. For the cleanest reference, make the XIAO the beacon at 22 dBm. Swap roles in a second run if you want to compare the boards.
3. **Bench check (5 minutes).** Put the nodes a few metres apart and connect both to the PC:
   ```sh
   python tools/range_test.py --beacon COM7 --receiver COM6 --minutes 5 --interval 5
   python tools/range_test.py --beacon COM7 --receiver COM6 --minutes 5 --interval 3 --cycle 7,8,9,10,11,12 --per-step 2
   ```
   Every beacon should be received (0 lost), RSSI should be strong (−30 to −70 dBm) and SNR around +10 dB. With cycling, the receiver reports `synced=1` after the first beacon and switches SF with the beacon. If it does not, see [Troubleshooting](#troubleshooting).
4. **Phone check.** Open the **Range test** page on the phone, tap **Phone GPS on** and allow location. The status line should show your position with an accuracy of a few metres (go outdoors).

## Setting up the beacon

1. Choose a fixed spot and note it: height above ground, indoors or outdoors, what the antenna sees. Height matters more than anything else. A beacon at 1 m behind a house wall gives a very different result than one on a balcony or mast. Keep the antenna vertical.
2. Connect to the beacon node (phone over Bluetooth or a laptop over USB) and open **Range test**:
   - **Interval**: 10 s is a good start. The node raises it per SF if the duty cycle requires it (see the table below).
   - **Cycle through SF**: tick it and choose the SFs (all six for a complete picture). **Beacons per SF**: 3–5.
   - **Beacon position**: stand next to the beacon, tap **Use this phone's position** and tick **The beacon sends this position in every packet**. The receiver then gets the distance without typing anything.
   - **Keep after restart**: tick it if the beacon is set up at home and then carried to its spot. It also keeps the beacon going after a brown-out.
   - Tap **Start beacon**. The status shows the next sequence number and SF. "Not transmitted" should stay at 0.
3. Disconnect the phone from the beacon (Bluetooth) and leave it running.

**Interval floor per SF** (36-byte beacon with 6 SFs and position, preamble 16, 125 kHz, CR 4/5). The node computes this itself. The minimum interval keeps beacons within the duty cycle with 25 % to spare:

| SF | Airtime | Min. interval at 10 % | Min. interval at 1 % |
| --- | --- | --- | --- |
| 7 | 85 ms | 2 s | 11 s |
| 8 | 160 ms | 3 s | 21 s |
| 9 | 300 ms | 4 s | 38 s |
| 10 | 559 ms | 7 s | 70 s |
| 11 | 1118 ms | 14 s | 140 s |
| 12 | 2236 ms | 28 s | 255 s (max) |

With interval 10 s, 5 beacons per SF and all six SFs, one cycle is (10 + 10 + 10 + 10 + 14 + 28 s) × 5 + 6 × 1.5 s guard ≈ 7 minutes. With 3 beacons per SF it is about 4 minutes.

## The drive

Connect the phone to the **receiver** node over Bluetooth, open **Range test**, tick the same SFs as on the beacon and tap **Start receiver**. The receiver learns the rest of the schedule from the beacons. The ticked SFs only tell it where to wait for the first one. GPS turns on automatically. Keep the page in the foreground and the screen on. Browsers slow down or stop background pages, so the GPS and the log can stall if you switch app. Plug the phone into the car charger.

**Where the receiver sits matters.** Inside a car the body absorbs roughly 5–10 dB, about a third to half of the range. For results that compare with the simulator, put the receiver (or its antenna) outside: on the roof with a magnet mount, or held out of the window at stops. Note what you did.

Use two kinds of run:

1. **Stop-and-measure (for the numbers).** Drive outwards from the beacon and stop at points with increasing distance. At each point, stay for **one full SF cycle** (4–7 minutes, see above), so every SF gets its beacons. Write down the point (see [What to log](#what-to-log)). Carry on until nothing is heard during a whole cycle, then go back to the last point where SF12 was still heard and try one or two points in between.
   - **Town:** points about every 250–500 m along a street that leads away from the beacon, until loss is above 90 % at SF12.
   - **Country:** points about every 1–2 km on open roads, also hilltops and dips.
2. **Moving coverage log (for the map).** Stop SF cycling (uncheck it on both, restart both modes) and use a fixed SF (e.g. SF9 or SF12) with a short interval (SF9: 5 s). Drive normally. Every received beacon is logged with its position, and the lost ones show as red marks in the chart. At 50 km/h with a 5 s interval there is one sample per 70 m.

When you are done, tap the three **Export** buttons on the receiver page (received beacons, beacons sent, power). In the Android app they open the share menu (save to Files/Drive, or mail them to yourself). Back home, connect to the beacon, tap **Export beacons sent** and **Export power**, then **Stop**.

The CSVs are kept only in the page while it is open. Export before you close it or reload. The receiver node also keeps per-SF totals until it is restarted or the test is started again, and the page shows them under **Results per SF**.

## What to log

The CSV export has all packet data. Add these notes per stop (paper or a phone note), keyed by time:

- time, place name, distance (the page shows it for the last beacon) and GPS accuracy
- environment: dense town / suburb / open field / forest / water, and whether there is line of sight to the beacon (yes / partly / no)
- receiver antenna: inside the car / outside / height
- weather (rain and wet leaves cost a few dB)
- anything odd: the beacon's battery (shown in the status), the phone's GPS jumping, a truck parked in front

Note once per run:

- beacon spot, height, antenna, TX power and board
- receiver board and antenna
- radio settings (frequency, BW, CR, preamble, duty cycle), interval, beacons per SF, extra bytes

## Power measurement

The boards cannot measure their own current. The node therefore counts time and **TX on-time** in each mode (off, beacon, receiver) and multiplies them with typical currents for the board. The result is an **estimate** and is labelled as such everywhere:

| Board | Base (MCU, Bluetooth, display) | Receive | Transmit |
| --- | --- | --- | --- |
| XIAO nRF52840 + Wio-SX1262 | 3.0 mA | 5.5 mA | SX1262 curve: 118 mA at 22 dBm, about 60 mA at 14 dBm |
| Heltec V4 | 50 mA | 9.5 mA | SX1262 curve at the PA input power + 100 mA for the PA |

These come from datasheets and typical values, not from measurements on these boards. Make them real:

1. **Measure the three currents** with a USB power meter between the power source and the node (no phone connected over USB, Bluetooth may be connected):
   - *Base + receive:* range test off or receiver mode, wait 2 minutes, read the average current. This is base + receive.
   - *Base alone* cannot be separated without a lab supply. Put the measured value in "Base" and 0 in "Receive", or subtract the SX1262's 5.3 mA.
   - *Transmit:* beacon mode at SF12 with interval = the floor (28 s). Read the peak current during a beacon. It shows for about 2 s on most meters. Transmit = peak − base − receive.
2. Enter the values on the **Range test** page under **Power use (estimate)**. The table is recomputed from the node's own time and TX on-time counters, so the numbers are good for any run.
3. **Long run on battery (the real test).** Charge the battery fully, note its capacity and start the beacon (or receiver) without USB power. Leave the page connected (or connect every now and then). The node sends a power report every minute with the battery voltage. **Export power** gives the voltage curve, and the time until the node stops is the real battery life. Compare it with the estimate: *capacity ÷ average current*. The page shows that too when you enter the capacity.

Battery voltage while on USB power says nothing about drain, and the page says so. For mAh per hour in each mode, run each mode for at least an hour. The page's table shows *average mA*, which is the same as mAh per hour, for off, beacon and receiver separately.

## Reading the results

**Per SF on the receiver** (page section *Results per SF*, or the end of `tools/range_test.py`):

- **Loss** = lost ÷ (received + lost). Beacons the beacon itself could not send (duty cycle, busy channel) are counted on the beacon and subtracted. They are not radio loss.
- **Reliable range**: the farthest stop with loss ≤ 10 % at that SF. **Maximum range**: the farthest stop where at least one beacon was heard. The page shows "Farthest heard" per SF.

**Link margin.** The SX1262 can decode down to these SNR values (125 kHz). The receiver's sensitivity is about −174 + 10·log10(125000) + 6 (noise figure) + SNR limit:

| SF | SNR limit | Sensitivity |
| --- | --- | --- |
| 7 | −7.5 dB | −124.5 dBm |
| 8 | −10 dB | −127 dBm |
| 9 | −12.5 dB | −129.5 dBm |
| 10 | −15 dB | −132 dBm |
| 11 | −17.5 dB | −134.5 dBm |
| 12 | −20 dB | −137 dBm |

The margin is *SNR − SNR limit*. A link with 0–3 dB margin is on the edge. Below the noise floor (SNR < 0) the packet RSSI is mostly noise. The signal strength is then about **RSSI + SNR**.

**Comparing with the simulator.** For each stop, compute the path loss:

> path loss = TX power at the antenna (dBm) − signal strength (dBm), where signal strength = RSSI, or RSSI + SNR when SNR < 0

Plot path loss against log10(distance in metres) and fit a straight line: path loss = A + 10·n·log10(d). The simulator assumes **A = 31.2 dB and n = 3** (`maarse-sim/src/main.rs`, `Radio::rx_dbm`), with a noise floor of −117 dBm. Open country typically gives n ≈ 2.5–3, suburbs 3–3.5 and dense town 3.5–4.5. Put the fitted A and n for town and country into the results table. They are what the simulator needs to be corrected with. A quick check of the simulator's own prediction: with 22 dBm and n = 3 it expects about 7 km at SF7 and 18 km at SF12 with no obstacles.

**In the chart**, RSSI and SNR are plotted per SF (one colour each) over time or over distance, and red bars along the bottom mark lost beacons (taller = more lost in a row). Switch to *Over distance* once the beacon position is known.

## Results template

Copy this into a note and fill one table per environment.

**Run:** date ____ · beacon: board ____ at ____ (lat, lon) · height ____ m · TX ____ dBm · antenna ____ · receiver: board ____ · antenna inside/outside car · radio ____ MHz / ____ kHz / CR 4/__ / preamble __ · interval ____ s · beacons per SF __ · weather ____

**Stops (one row per stop and SF):**

| Time | Place | Distance (m) | Environment | Line of sight | SF | Received | Lost | Loss % | RSSI avg | SNR avg | SNR min | Margin (dB) | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| | | | | | 7 | | | | | | | | |
| | | | | | 12 | | | | | | | | |

**Summary per SF:**

| SF | Town: reliable range (≤ 10 % loss) | Town: max range | Country: reliable range | Country: max range | Simulator prediction |
| --- | --- | --- | --- | --- | --- |
| 7 | | | | | ≈ 7 km |
| 8 | | | | | |
| 9 | | | | | |
| 10 | | | | | |
| 11 | | | | | |
| 12 | | | | | ≈ 18 km |

**Path loss fit:**

| Environment | A (dB) | n | Points used | Simulator |
| --- | --- | --- | --- | --- |
| Town | | | | A = 31.2, n = 3 |
| Country | | | | A = 31.2, n = 3 |

**Power:**

| Board | Mode | Hours | TX on-time % | Measured current (meter) | Estimated current (page) | Battery voltage start → end | Battery life (real / estimated) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| | Off (idle) | | | | | | |
| | Receiver | | | | | | |
| | Beacon SF__ / every __ s | | | | | | |

## Troubleshooting

- **The receiver hears nothing.** Check that frequency, bandwidth, CR and preamble are the same on both (**Radio** page). With SF cycling, an unsynced receiver waits on the *first* SF of the cycle (the lowest ticked SF) and only catches the beacon when the beacon comes back to that SF. That can take a whole cycle. Without cycling, both must be on the same SF in the radio settings.
- **"Not transmitted" climbs on the beacon.** The duty cycle budget is used up (other traffic, or a 1 % band) or the channel is busy. Raise the interval.
- **The receiver loses sync after a while.** It returns to the first SF after three cycles without a beacon (out of range). It syncs again by itself when it hears the beacon.
- **No GPS position.** The page must be in a secure context (Android app, https, or localhost), location must be allowed for the app or site, and the page must stay in the foreground. Indoors, the position may never come.
- **The page says to connect directly.** The range test runs on the radio node. Through maarse-host there is no radio to control, so connect to the node over USB or Bluetooth.
- **The Heltec does not respond after opening the port.** Open it with DTR high and RTS low (`tools/range_test.py` does). The default pyserial open resets the ESP32-S3.
- **Normal mesh traffic during the test.** A receiver that switches SF does not hear the mesh on other SFs, and both nodes keep sending their announces (every 15 minutes). Stop the range test to go back to normal: the receiver then returns to the SF in its radio settings.

## Technical details

- **Commands** `0x2a SetRangeTest` (15 bytes: mode, flags, interval, SF mask, beacons per SF, extra bytes, latitude, longitude) and `0x2b GetRangeTest`. **Events** `0xa8 RangeState`, `0xa9 RangeRx`, `0xaa RangeTx` and `0xab Power`. The formats are in `maarse-core/src/companion.rs`.
- **The beacon packet** starts with `0xD7 'R' 'T'`. The version bits of the first byte are `11`, so every Må:rse node rejects it and no repeater forwards it: only the direct link is measured. The format and the schedule are in `maarse-core/src/rangetest.rs`.
- **Schedule.** Each SF gets a step: a 1.5 s guard, then *beacons per SF* slots of that SF's interval. Beacon number *j* (counted from the start) has sequence number *j*, so the SF of a lost beacon is known. The receiver estimates the cycle start from every beacon it hears. Listen-before-talk only ever delays a beacon, so it keeps the earliest estimate and follows later ones slowly (clock drift). It switches its own receive SF at the step boundaries. The beacon does not change its own receive SF: each beacon carries its SF to the radio as part of the transmit job.
- **Duty cycle.** Beacons go through the same duty cycle limiter and listen-before-talk as all other traffic. The receiver's SF changes keep the airtime budget (it used to be reset on every radio setting change).
- **Persistence.** With *Keep after restart* the setting is stored in flash (settings page at offset 0x400, `RNG1`) and resumed at boot. Without it, the test stops at the next restart. Stopping the test clears the stored setting.
- **Power accounting.** Per mode: time, TX on-time (the radio's airtime counter) and charge = base × time + receive × (time − TX time) + transmit × TX time. Board profiles are in `firmware/nrf/src/main.rs` and `firmware/esp/src/main.rs` (`power::set_profile`). The transmit curve is in `rangetest::sx1262_tx_ma_x10`.
