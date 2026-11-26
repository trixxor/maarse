# Remote repeater information and safe radio changes (proposal)

Status: approved and built 2026-11-26 (working copy, not flashed, not bench-tested). Original wording of the proposal follows; the decisions are in section 6.

## 1. Problem

A repeater in the field is reached over LoRa (every byte costs airtime, and the radio is deaf while it
transmits) or over TCP (if it has Wi-Fi, bandwidth is not a concern). Administrators want to see how it
is doing and change its radio settings without driving there.

What exists today (`firmware/common/src/node.rs`, `Command::Remote`):

- Remote control needs no login. Each command is encrypted by the sender, who must be on the repeater's
  administrator list, and carries a counter that must be newer than the last one accepted (and within
  10 minutes of the repeater's clock). This is a good fit for LoRa and is kept.
- Only these commands are allowed through `Remote`: `GetStats`, `GetRadio`, `SetRadio`, `SetRole`, OTA,
  `Sensor`, and the spectrum recording commands. Everything else is refused.
- `GetStats` always returns everything (about 100 keys, in chunks of 25 over remote control). There is
  no way to ask for a few keys.
- `GetNeighbors` (0x21, neighbour list with SNR and RSSI) exists but is local only.
- `SetRadio` over remote control applies at once **and saves to flash**, with no confirmation and no
  way back. A wrong frequency or spreading factor leaves the repeater unreachable, and the mistake
  survives a reboot. Only someone with a USB cable at the repeater can fix it.

## 2. Three levels of information

Always one signed request, one reply of a known size. Nothing streams. Clients choose the level by the
link: short over LoRa by default, more over TCP.

| Level | Content | Size | When |
| --- | --- | --- | --- |
| Short status | A chosen list of stats keys | under 60 bytes of values, one reply | Overview of many repeaters; the default over LoRa |
| Full status | All keys, as today | a few hundred bytes, 3-4 chunks | Looking closely at one repeater; the default over TCP |
| Heavy items | Spectrum recording (MSR2, about 200 bytes), neighbour list, history | up to a few KB, on request only | Fault finding; always an explicit action |

### 2.1 Short status: `GetStats` with a key list

`GetStats` gets an optional trailing list of stat keys (one byte each, at most 24). An old node ignores
the trailing bytes and answers as today, so the request is safe to send to any node. A node that
understands the list answers with one `Stats` frame holding only those keys that it has.

Suggested default set for the "short status" button, which a client may change:

| Key | Meaning |
| --- | --- |
| 1 | uptime s |
| 10, 13, 14 | rx packets, forwarded, originated |
| 11, 12 | rx invalid, rx duplicates |
| 32, 33 | radio tx ok, tx err |
| 40, 41 | airtime ms, duty used permille |
| 44 | tx queue |
| 34, 35 | last RSSI, last SNR |
| 80 | noise floor dBm |
| 49, 85 | battery mV, battery % |
| 86 | temperature (0.1 degC) |
| 90 | crashes |

That is 18 keys, so a reply of well under 100 bytes. Whoever only wants send/receive counts and a
temperature asks for keys 10, 32, 33 and 86.

Keys 116/117 (own position) are never returned through remote control, as today.

### 2.2 Neighbour list on demand

Allow `GetNeighbors` through `Remote` for administrators. The reply is the existing `Neighbors` frames
(15 bytes per neighbour). Over LoRa a client asks only after the user presses a button, and the list is
limited to the 8 neighbours heard most recently unless the request asks for more.

### 2.3 Not proposed

- No periodic push of status from the repeater over LoRa. It would use the shared channel for everyone.
  Over TCP a push (for example every 10 minutes to the host) is harmless and can be added later as a
  separate decision.
- No temperature or battery history stored on the repeater for now (flash and RAM are tight, see the
  ESP32 stack note). A host can build a history by polling short status.
- No password login and no session state in the repeater. The administrator list is stronger and
  stateless.

## 3. Safe radio change

### 3.1 Behaviour

1. A remote `SetRadio` no longer saves. The repeater replies with the new settings (still on the old
   ones, as today), switches, and starts a trial timer (default 5 minutes, at most 30).
2. During the trial the radio settings are **not** written to flash. The previous settings stay in flash
   and in RAM.
3. A new command, `ConfirmRadio` (administrator, remote or local), sent after the repeater answers on
   the new settings, stops the timer and saves the settings to flash.
4. If the timer runs out without a confirmation, the repeater switches back to the previous settings
   and clears the trial. A reboot during the trial also ends up on the previous settings, because flash
   was never changed.
5. A second `SetRadio` during a trial restarts the trial from the settings still stored in flash, not
   from the unconfirmed ones, so a chain of mistakes cannot become permanent.

A lost confirmation over LoRa makes the repeater roll back, which is the safe direction. The client
retries the confirmation a few times and says plainly if the repeater did not answer.

Local `SetRadio` (USB, Bluetooth) keeps today's behaviour: applied and saved at once, because the
person is at the device.

### 3.2 Protocol changes

- `SetRadio` through `Remote` gains an optional trailing byte: trial minutes (0 or absent: use the
  default 5). Absent keeps the new safe behaviour. There is no "save at once" flag over remote control.
- New command `ConfirmRadio`, allowed through `Remote`, no content. Reply: `Radio` with the settings
  that are now stored, so the client can check that they are the intended ones.
- New stats key: seconds left in a radio trial (0 = none). It is included in the short status, so a
  repeater that is in a trial shows it.
- `RemoteReply` for `SetRadio` stays as it is.

Command codes are assigned when building, from the free codes at that time (0x15-0x17 and 0x35-0x37 are
reserved today; check `protocol-spec.md` section 9.2 first).

### 3.3 Interaction with the same-radio-settings rule

All nodes and trunks are meant to share one frequency, SF and settings. A remote change of one repeater
therefore splits it from its neighbours until the others follow. The trial timer protects against a
wrong value, not against a deliberate change that others do not follow. The client warns when the new
settings differ from the neighbours' (it knows them from the radio config of the connected node).

## 4. What the clients show

Both the web client and the Android app, in the same piece of work:

- A repeater page with **Short status** and **Full status** buttons, the choice defaulting by link
  (LoRa: short; TCP or host: full), and a **Neighbours** and a **Spectrum recording** action.
- Radio settings with a trial: after **Apply**, a countdown and a **Confirm** button. The client also
  tries to confirm by itself when the repeater answers on the new settings, and says what happened.
- All text in plain words; counters and keys behind **Details**.

MeshCore repeaters stay read-only in the app, as agreed, so none of this applies to them.

## 5. Testing

- Firmware unit tests: key list parsing (old nodes ignore it), trial timer, rollback, confirm, reboot
  during a trial, second `SetRadio` during a trial.
- Simulator: a repeater that is given a wrong frequency and never confirmed comes back.
- Bench (two radios, needs the user at the bench): change SF remotely, confirm; change it and do not
  confirm; check the repeater is reachable again after the timer.
- Golden/codec tests are not affected (no change to MSR2).

## 6. Decisions

Approved by the user on 2026-11-26: the three levels, the key list in `GetStats`, `GetNeighbors` through
`Remote`, and the trial-and-confirm `SetRadio` (sections 2 and 3).

Deferred, to be looked at later together with a TCP-connected host:

- A repeater on TCP pushing its short status to the host periodically, and how often.

Not decided, defaults used until the user says otherwise:

- Trial length: 5 minutes default, 30 maximum.
- A "view only" administrator role is not part of this work.
- The short-status key list in 2.1 is a default that a client may change.
