# Reference

> **Type:** index of reference pages · Exact, complete, consistent. For the why, see [Concepts](../concepts/README.md).

| Subject | Page | Source of truth |
| --- | --- | --- |
| Packet format, announces, routing rules, cryptography, mailboxes, DHT, companion protocol, versioning | [protocol-spec.md](../protocol-spec.md) | the code (each constant names its file) |
| Every host and server option (`settings.txt`) | [settings.md](settings.md) (generated) | `maarse-host --help` |
| Companion commands, events, error codes and stats keys | [spec section 9](../protocol-spec.md#9-companion-protocol) | `maarse-core/src/companion.rs` |
| Sensor catalog and kinds | [sensors.md](../sensors.md) and `sensors/catalog.json` | `sensors/` |
| Translations | [i18n.md](../i18n.md) | `i18n/` |
| Component list and dependencies | [component-map.md](../architecture/component-map.md) | the `Cargo.toml` files |
| Test and simulator scenarios | [GUIDE.md, Simulator](../GUIDE.md#simulator) | `maarse-sim`, `tools/sim-experiments` |

Rules for reference pages: only describe, mirror the structure of the code, state
defaults, ranges, versions and platforms, and name the file the facts come from.
Generated pages are never edited by hand.
