# Outbound backbone gateway

## First boot and Wi-Fi recovery

1. Join the open 2.4 GHz network `maarse-gate`. If the setup page does not open automatically, open `http://192.168.4.1/`.
2. Select a scanned network or enter a hidden SSID manually. Enter its password and the six-digit setup code on the display. The code authorizes a settings change. After five wrong codes a new code is shown on the display. The setup Wi-Fi and page are unencrypted, so configure the device in a trusted location.
3. To provision the outgoing link, expand **Backbone connection** and paste the **pairing code** from the backbone dashboard (**Connections → Pair a gateway**). The code is one short line (about 90 characters) with the server's name and port, a one-time secret that works once, for ten minutes, and a pin of the server's certificate. Leave the backbone fields blank to keep the previously saved backbone settings. As an alternative to a pairing code, enter the DNS name, port (default 5056), issuing CA PEM certificate and gateway token by hand. A private `backbone_config.h` can also provide build-time defaults. An image built without that header accepts runtime settings in the portal.
4. Choose **Save and restart**. The firmware tests the new network for up to 30 seconds and commits it after receiving an IP address. If the test fails, it tries the previous settings. If there are no previous credentials, or the previous network also fails, it reopens `maarse-gate`. A later loss of Wi-Fi also triggers a reconnect attempt before setup is reopened.

The screen shows the setup address and code, or the current Wi-Fi address and backbone status. It sleeps after 30 seconds and a short press wakes it. Hold the user button for at least five seconds and release to clear the saved gateway settings and reboot into setup. The modem's LoRa identity lives separately.

## Build

Use the [web flasher](../../README.md#flash-from-the-browser) or the [gateway build scripts](../../README.md#build-on-linux). The outbound gateway is available for Heltec V4 OLED and TFT displays.

## Backbone transport

The gateway establishes the clock using SNTP before connecting. On the TLS
stream it sends `MAARSE-KISS/1 <gateway-token>\n` and waits for `OK\n`.
Following authentication, both directions use standard MeshCore KISS frames.
A new connection clears the modem's previous KISS session. The firmware
retries after disconnects. This variant has no inbound TCP listener.

## Pairing

A pairing code has the form `MP1;<host>;<port>;<secret>;<pin>`, at most 120
characters. The secret is 20 hex characters (80 bits) and the pin is the first 16
bytes of the SHA-256 of the server's certificate, 32 hex characters. After the
Wi-Fi connects, a gateway that holds a pairing code (stored as the token
`pair-<secret>-<pin>`) opens a TLS connection. It does not know the server's
authority yet, so it accepts the certificate for now and compares the pin with
the SHA-256 of the certificate it is shown. If they differ the gateway stops and
the display says **Wrong server**; nothing secret was sent. If they match it sends
`MAARSE-PAIR/1 <secret>\n`. The server answers `PAIRED <token>\n` with a new
token of its own for this gateway, then a second line with the authority
certificate (the base64 body), or `ERR\n`, and closes the connection. The
gateway saves the token and the authority, restarts, and from then on verifies
the server with that authority and connects as usual. After `ERR` (the code
expired, was used or was wrong) the gateway stops trying and the display says
**Pairing refused**; hold the button for five seconds to start over. A code
works once and for ten minutes, and the server cancels it after five wrong
secrets. Pairing replaces the previous token, so the server accepts one gateway
at a time. The pin is checked before the secret is sent, so a server that is not
the one the code was made for never receives the secret. The authority only
reaches the gateway over that checked connection.

The Må:rse backbone accepts this handshake with `--meshcore-gateway`. It
bridges the KISS session to a loopback worker socket. Start the worker before
the gateway connects. The backbone waits up to one second for the worker
after authentication, then the gateway retries. Consult the Må:rse guide for
the listener options and TLS files.

The earlier gateway firmware was verified on a physical Heltec V4 against a local backbone over TLS. The
worker received a GetVersion response and discovered one repeater over LoRa. The Wi-Fi provisioning and display changes have been compiled for OLED and TFT. Their on-device behavior has not been verified.

## Local radio worker

Run `meshcore-host --service --tcp 127.0.0.1:5057 --data <backbone-data>/meshcore-nodes` on
Windows. Use a private data directory, with one worker process per directory.
The worker restores saved identities when it starts. `ensure <person-address>`
on standard input creates a node if needed and prints its public key. The
address is the 32-character lowercase hexadecimal Må:rse person address.
`node <person-address> <command>` targets an already provisioned node.

This is a local, trusted development interface. User authentication and grant
checks belong in the backbone service that calls it. Do not give clients
access to the worker's standard input. Worker output is for diagnostics.

The backbone and worker share the `meshcore-nodes` directory for client
provisioning. After authorizing a device, the backbone writes a provision
request for that person. The worker atomically publishes `public-key.txt`
after persisting the corresponding identity. Clients receive only the public
key through the backbone WebSocket.
