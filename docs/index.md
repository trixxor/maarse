# Må:rse documentation

Må:rse is an encrypted mesh network for cheap LoRa radios that can reach out
into the world over TCP and the internet. This is the entry point to the
technical documentation. Pick the door that matches what you want to do.

> **Status of this documentation.** The structure below was introduced on
> 2026-11-27. Existing long pages ([GUIDE.md](GUIDE.md),
> [protocol-spec.md](protocol-spec.md), the [HTML manual](manual/index.html))
> are linked, not duplicated, and are being split into the four types over
> time. Where a page is still mixed, its row in the table says so.

## Four doors

| I want to... | Go to | Type |
| --- | --- | --- |
| **Learn by doing**: get a first node running | [Tutorial: your first node](tutorials/first-node.md) | Tutorial |
| **Get a task done**: flash a board, run a server, build and test | [How-to guides](guides/README.md) | How-to |
| **Look something up**: wire format, settings, companion commands, constants | [Reference](reference/README.md) | Reference |
| **Understand why**: routing, carry, post offices, keys, trade-offs | [Concepts](concepts/README.md) and [Architecture](architecture/overview.md) | Explanation |

```text
                 practical (doing)           theoretical (knowing)
   studying      TUTORIALS                   EXPLANATION
                 learning-oriented           understanding-oriented

   working       HOW-TO GUIDES               REFERENCE
                 goal-oriented               information-oriented
```

The four types follow the [Diataxis](https://diataxis.fr/) framework, as
described in the [Divio documentation system](https://docs.divio.com/documentation-system/).
Each page says which type it is.

## Quickstart by platform

| Platform | Start here |
| --- | --- |
| Windows PC (host + web client) | [Tutorial: your first node](tutorials/first-node.md) |
| Android phone | [User guide](user/README.md) |
| Heltec V4, XIAO ESP32-S3, XIAO nRF52840, WisMesh Tag | [How to flash a board](guides/flash-a-board.md) |
| Debian / Ubuntu server (backbone, post office) | [How to run a server](guides/run-a-server.md) |
| Browser only, no install | [Mesh Lab](https://trixxor.github.io/maarse/simulator/) and [Denmark simulator](https://trixxor.github.io/maarse/denmark/) |

## What exists today

| Area | Page | Type | Status |
| --- | --- | --- | --- |
| Overview and component map | [architecture/](architecture/overview.md) | Explanation | working copy |
| Packet formats, crypto, routing, companion protocol | [protocol-spec.md](protocol-spec.md) | Reference | follows the code of the working copy |
| Settings of host and server | [reference/settings.md](reference/settings.md) | Reference | generated from the code |
| Everything in one long page | [GUIDE.md](GUIDE.md) | Mixed (all four types) | released + working copy, being split |
| Plain-language pages for users | [user/](user/README.md) | How-to | working copy |
| Sensors | [sensors.md](sensors.md) | Reference + how-to | working copy |
| Range, loss and power measurement | [FIELD-TEST.md](FIELD-TEST.md) | How-to | procedure written; no field results yet |
| Threat model | [security/threat-model.md](security/threat-model.md) | Explanation | working copy |
| Security review | [security-audit-2026-11-20.txt](security-audit-2026-11-20.txt) | Report | internal, 2026-11-20 |
| Decision log (including rejected ideas) | [decisions/](decisions/README.md) | Explanation | maintained |
| What is verified, and how | [verification/](verification/README.md) | Reference | maintained |
| Comparison with Meshtastic, MeshCore, Reticulum | [../compare.md](../compare.md) | Explanation | simulator and documentation based |
| Interactive manual | [manual/index.html](manual/index.html) | Mixed | released + working copy |

## Status labels

Every claim in the documentation is one of these. If a page does not say, the
label is `working copy`.

| Label | Meaning |
| --- | --- |
| `released` | In a tagged release (latest tag: see the [releases](https://github.com/trixxor/maarse/releases)) |
| `working copy` | In the repository, not in a release |
| `simulated` | Result of `maarse-sim` or the Denmark simulator only |
| `tested (automated)` | Unit, end-to-end and parity tests, run on one computer |
| `bench-tested` | Run on real boards on a desk |
| `field-tested` | Run outdoors over real distances. **Nothing carries this label yet.** |
| `designed, not built` | Written down, no code |
| `shelved` | Built or designed, deliberately not pursued |
| `rejected (measured)` | Tried and dropped; the numbers are in the decision log |

## Conventions

- Everything new is written in English. Existing Danish pages stay until the
  translation job.
- The product name is **Må:rse** wherever a person reads it. `maarse` appears
  only in file names, paths, identifiers and URLs.
- Options are changed in the web client or in `settings.txt`. Command-line flags
  are never the recommended way.
- A change to behavior, defaults, deployment, security or protocols updates the
  affected pages in the same change ([AGENTS.md](../AGENTS.md)).
- The Android app and the PC web client are documented together.
- How to write and review pages: the `technical-docs` skill in
  `.claude/skills/technical-docs/SKILL.md`, and the
  [documentation check](verification/README.md#documentation-checks).
