# Post offices and carry

> **Type:** explanation · **Reader:** someone who wants to know what happens when the recipient is not reachable · **Status:** host post office `tested (automated)` on one computer (`tools/e2e_test.py`, `tools/post_office_test.py`); LoRa trunks and carry `simulated`; **not field-tested**. Exact rules: [protocol-spec.md, sections 5.10 and 8](../protocol-spec.md#510-store-carry-forward-and-the-delivered-notice).

## The problem

Nobody needs to be online at the same time as the person they write to. A phone
may be off, out of range, or asleep. A mesh that only delivers to nodes that are
reachable right now loses those messages.

## Two mechanisms

**Post offices (store and wait).** A host with a disk keeps mail for others.
When a direct message gets no end-to-end acknowledgement within 45 seconds, the
sender hands it to its own home post office. The post office keeps it until the
recipient shows up and asks for it. Hosts say that they keep mail in their hellos
and announces; every node picks the best post office among its neighbors as its
"home". Radio nodes keep no mail today: repeaters carry it but 8 KB of flash holds
only 30-40 messages for everyone in range.

**Carry (store, carry, forward).** When there is no route at all, a relay that
holds the message may carry it physically: a hiker's relay hands copies to
people it meets, and a copy that reaches the destination is delivered as an
ordinary message. This is "binary spray and wait": a limited number of copies
(4 for relays), a lifetime of 30 minutes, at most 8 carried messages.

The **delivered notice** closes the loop: when the destination gets a carried
copy it sends a short 18-byte notice, and carriers that hear it delete their
copies. When it reaches the sender, the app can show "delivered".

## Why this design

- Reuses the same acknowledgements and priorities as normal messages.
- The post office stores messages sealed without a session, because a ratchet
  message delivered days later may fall outside the skipped-key window (see
  [Identity and keys](identity-and-keys.md)). Since 0.9.13 the seal uses the
  recipient's weekly mail key, which the recipient erases after about three
  weeks, so older mail stays closed even if the long-term key leaks later. It is the static seal, without forward secrecy,
  when the sender has no recent key (not written to each other this week, an
  older version, or a radio that does not know the time).
- Messages stay end-to-end encrypted in every post office and every carrier.

## Limits you should know

| Limit | Value | Where |
| --- | --- | --- |
| Time before handing to a post office | 45 s | `DEPOSIT_AFTER_MS` |
| Carry lifetime | 30 min, 8 messages | `CARRY_TTL_MS`, `CARRY_MAX` |
| Mail quota on a host | 200 MB by default, configurable | `--mail-quota-mb` ([settings](../reference/settings.md)) |
| Radio nodes keeping mail | none (class 0) | [spec 8.1](../protocol-spec.md#81-post-offices-deposit-mailsync) |

## Not done (yet)

- Regional routing with custody transfer between anchors and a home mailbox for
  long LoRa-only distances: parked, see
  [decision 0006](../decisions/0006-long-range-routing-parked.md).
- Delivering MeshCore chats over the Må:rse message loop for LoRa-only phones:
  parked, see [decision 0007](../decisions/0007-meshcore-over-lora-parked.md).

User view of the same feature: [Post offices: messages that wait for you](../user/post-office.md).
