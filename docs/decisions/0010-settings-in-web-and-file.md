# 0010: Every option lives in the web interface and `settings.txt`

- **Status:** accepted
- **Date:** 2026-11-21

## Context

Command-line flags are easy to forget after a reboot and awkward for ordinary
operators.

## Decision

All options of `maarse-host` and `maarse-server` are set in the web interface
(Settings) and stored in `<data>/settings.txt`, one option per line, in the same
spelling as the flag. Flags remain for the very first start and for scripts, but
documentation describes the web interface and `settings.txt` first. A new feature
gets its setting there first.

## Consequences

- The [settings reference](../reference/settings.md) is generated from the host's
  `--help` text and describes `settings.txt`.
- At start-up `settings.txt` is read first and the command line afterwards, so the
  command line wins; the dashboard shows such options in their fields.
- Saving restarts the host (same program, same privileges).

## Evidence

`maarse-host/src/settings.rs`; [GUIDE.md, Dedicated server](../GUIDE.md#dedicated-server-maarse-server).
