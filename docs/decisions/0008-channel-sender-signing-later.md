# 0008: Signing the sender of channel messages

- **Status:** parked (idea noted, not started)
- **Date:** 2026-11-23

## Context

The sender inside a channel message is **claimed, not authenticated**: anyone who
holds the channel key can write as anyone ([spec 6.4](../protocol-spec.md#64-channels)).
For the public and hashtag channels the key is derived from the name, so this is
anyone. Linked-device handling on the host shows a message as "from me" on the
person's other devices based on that claim.

## Idea

Add a signature by the sender's identity inside the channel plaintext (as signed
bulletins already do for `#bulletin`, [spec 6.6](../protocol-spec.md#66-signed-bulletins)),
verified by the receiving host.

## Trade-offs

- **For:** removes the impersonation hole in channels.
- **Against:** a 64-byte signature costs airtime on a 212-byte frame; it needs a
  format version and a rollout to all nodes.

## Decision

Not built. Documented as a known weakness in the
[threat model](../security/threat-model.md).
