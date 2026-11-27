# 0005: Optional network key for closed nets

- **Status:** parked (idea, not started)
- **Date:** 2026-11-15

## Context

Emergency services, clubs or companies may want their own net on the same
frequency without foreign nodes routing through it or seeing its metadata.
Reticulum solves this with IFAC.

## Idea

An optional network key. Every packet gets a short code (4-6 bytes HMAC with the
key) and a lightly encrypted header; packets without a valid code are dropped
before routing. Modes: open (today), closed (only keyed packets), both (a
repeater serving a closed net and the public one). The key is shared as a QR code,
per radio in firmware and per link on the host.

## Trade-offs

- **For:** access control and less metadata; foreign floods stop at the first
  repeater.
- **Against:** 2-4 % more airtime, key management (a leaked key means change
  everywhere), and it splits the network, which cuts against
  [decision 0003](0003-one-radio-setting.md).
- **Not** a replacement for end-to-end encryption: messages are already encrypted;
  this is about access and metadata.

## Decision

Not built. Proposed in the [comparison](../maarse-comparison.html) as
"Network key (optional, like IFAC)".

## Consequences

Closed nets are not possible today; privacy relies on end-to-end encryption
only ([threat model](../security/threat-model.md)).
