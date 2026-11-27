# Identity and keys

> **Type:** explanation · **Status:** `working copy`. **The cryptography has not been reviewed by an external cryptographer**, and the forward-secrecy ratchet is a draft in its own source. Exact formats: [protocol-spec.md, sections 2 and 6](../protocol-spec.md#6-cryptography). Source: `maarse-crypto/src/*`.

## An identity is two key pairs

An identity is an Ed25519 key pair (signatures) and an independent X25519 key
pair (key agreement). The **address** is a 16-byte hash of the public keys. It is
the same over LoRa, TCP and the internet, so a contact survives a change of link.
Nobody assigns addresses: a node creates its own the first time it starts.

## How a direct message is protected

1. **Static seal.** Without a session, a message is sealed to the recipient's
   public key in the style of Noise `X`: an ephemeral key, a sender field masked so
   that only the recipient learns who sent it, and ChaCha20-Poly1305 for the
   content. The sender address is never sent in clear text. Overhead: 64 bytes; the
   largest text is 148 bytes.
2. **Forward-secrecy ratchet (draft).** Once two contacts both support it, messages
   use a ratchet so that a later key leak does not expose earlier messages. A
   compact frame variant saves airtime (`CAP_COMPACT`).
3. **Safety number.** Two people can compare a 60-digit number derived from both
   public keys to detect a man in the middle.

## Channels

A channel has a symmetric key: the public channel and `#hashtag` channels derive
theirs from the name (so anyone can read them), private channels use 32 random
bytes shared by invitation. **The claimed sender inside a channel message is not
authenticated**: anyone who holds the key can write as anyone. Direct messages do
not have this weakness. Sender signing for channels is an idea, not built
([decision 0008](../decisions/0008-channel-sender-signing-later.md)).

## Announces and spam

Nodes announce their public keys, signed. An announce needs a small proof of work
(a *stamp*, about 0.3 s on an nRF52840), so one device can make only a few fake
identities per second.

## Recovery and linked devices

A 24-word recovery phrase (BIP39 word list) derives the master identity. Several
devices can be linked to one person through a signed device list; a linked device
counts as the person on that person's other devices.

## What this does not give you

See the [threat model](../security/threat-model.md): metadata (who talks to
whom, when, how often, from where) is visible to anyone who hears the radio
traffic, and there is no anonymity.
