# Android app redesign (proposal)

Status: proposal, 2026-11-17. Not built yet. Mockup with seven phone screens
(a private design canvas; ask the owner for access):
<https://claude.ai/artifact/UK4CvEWwTEohn94nKmYZDn>

## Why

The native app (`android/`) works, but it is laid out like a tool: four tabs,
and the connection mode is a list of radio buttons at the top of Settings.
Users asked "where do I switch mode?". The redesign takes the shape people
know from messengers.

## Inspiration

| App | What we take |
|---|---|
| Signal, WhatsApp | Chats as the home screen; avatars with initials, preview, time, unread count; a button for a new chat |
| Signal, Telegram | Settings behind your own picture (top left), not a tab |
| Threema, Briar | A short first-start guide; linking devices by QR |
| Signal | Delivery shown as small icons, words on tap; contact page with safety number and "verified" |
| Meshtastic, MeshCore | The network in plain words first, numbers second |

## The screens

1. **First start**: "How will you use Må:rse?" — on this phone
   (recommended), with my radio only, or with my home computer. Step 2:
   create an account or link this phone to one by QR.
2. **Chats (home)**: a connection chip at the top ("Phone node · online ·
   LoRa radio connected · Change"), verified alerts as a card, the chat list,
   a new-chat button. Three tabs: Chats, Contacts, Network.
3. **Switching mode**: tapping the chip opens a bottom sheet with the three
   ways to connect and the switch "Use my radio as LoRa modem". Advanced
   modes (server, VPN host) are under You › Advanced.
4. **A conversation**: bubbles with icons for sent, delivered and waiting in
   a post office; the header says how the contact is reached ("over LoRa,
   3 hops"); tapping a message tells how it went.
5. **Contact**: name, "verified in person", Message / Call / Route, the
   person's devices (phone, PC at home, radio in the car) and which one is
   active, safety number and address.
6. **You** (tap your picture): your QR code; Account (my devices, backup,
   name and position), This phone (connection, notifications, background),
   Advanced (radio and role, host settings, server mode).
7. **Network**: one line of status in plain words, four tiles (airtime,
   post offices, carrying, backbone), and Map, Alerts, Neighbours and All
   numbers under "More".

## Look

Inter, a light grey-green ground (#F7F8F6), the Må:rse green #2F7D5B as the
one accent, amber only for warnings and post-office waiting. Touch targets
at least 44 px. UI text stays in English (project rule).

## Next

Waiting for a go (or changes) before `android/app` is rebuilt to match.
