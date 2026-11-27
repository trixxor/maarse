# Decision log

> **Type:** explanation · One short page per decision, including the ideas that were tried or parked and the numbers behind them. New decisions get the next number; old ones are never deleted, only superseded.

Format of every page: **Context**, **Options**, **Decision**, **Consequences**, **Evidence**, with a status:
`accepted` · `rejected (measured)` · `shelved` · `parked` (not started, may return) · `proposed` (waiting for a choice).

| No. | Decision | Status | Date |
| --- | --- | --- | --- |
| [0001](0001-documentation-structure.md) | Documentation follows Diataxis; generator and design not yet chosen | accepted / proposed | 2026-11-27 |
| [0002](0002-no-ota-over-lora.md) | No firmware updates over LoRa | shelved | 2026-11-14 |
| [0003](0003-one-radio-setting.md) | One radio setting for every node and link | accepted | 2026-11-16 |
| [0004](0004-load-control-rejected.md) | Send priorities yes; SF-scaled slot only with scaled timeouts; path request quota no | accepted / rejected (measured) | 2026-11-16 |
| [0005](0005-network-key-maybe.md) | Optional network key for closed nets | parked | 2026-11-15 |
| [0006](0006-long-range-routing-parked.md) | Regional routing with custody transfer and home mailbox | parked | 2026-11-15 |
| [0007](0007-meshcore-over-lora-parked.md) | MeshCore chats over the Må:rse message loop | parked | 2026-11-20 |
| [0008](0008-channel-sender-signing-later.md) | Signing the sender of channel messages | parked | 2026-11-23 |
| [0009](0009-native-android-app.md) | Native Android app with the host embedded | accepted | 2026-11-17 |
| [0010](0010-settings-in-web-and-file.md) | Every option lives in the web interface and `settings.txt` | accepted | 2026-11-21 |
| [0011](0011-airtime-round.md) | Airtime round: compact frame on, lean header and others not built | accepted / parked | 2026-11-26 |

Longer analyses behind several decisions are in `docs/maybe/` (for example
[airtime-optimisation.md](../maybe/airtime-optimisation.md),
[network-stack.md](../maybe/network-stack.md),
[crisis-carry.md](../maybe/crisis-carry.md)).
