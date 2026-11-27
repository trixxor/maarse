# Concepts

> **Type:** index of explanation pages · Why Må:rse works the way it does. For exact formats and constants use the [protocol specification](../protocol-spec.md); for recipes use the [how-to guides](../guides/README.md).

| Topic | Page | Status |
| --- | --- | --- |
| Finding a route, per-hop acknowledgement, repair, roles, priorities | [Routing](routing.md) | `simulated`, `bench-tested` on two boards; not field-tested |
| Messages that wait for you, and messages that get carried | [Post offices and carry](post-offices-and-carry.md) | host post office `tested (automated)`; carry `simulated` |
| Addresses, keys, sealed messages, ratchet, channels | [Identity and keys](identity-and-keys.md) | `working copy`; **not externally reviewed** |
| Why every node uses the same radio settings | [Decision 0003](../decisions/0003-one-radio-setting.md) | decision |
| What the network does and does not protect | [Threat model](../security/threat-model.md) | `working copy` |
| Comparison with Meshtastic, MeshCore, Reticulum | [compare.md](../../compare.md) | documentation and simulator based |

Still only in [GUIDE.md](../GUIDE.md) and waiting for their own concept page:
regions, the backbone DHT, battery-driven routing, pages (`maarse://`), alerts,
linked devices, sensors.
