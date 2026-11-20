# Outbound backbone gateway

## First boot and Wi-Fi recovery

1. Join the open 2.4 GHz network `maarse-gate`. If the setup page does not open automatically, open `http://192.168.4.1/`.
2. Select a scanned network or enter a hidden SSID manually. Enter its password and the six-digit setup code on the display. The code authorizes a settings change. After five wrong codes a new code is shown on the display. The setup Wi-Fi and page are unencrypted, so configure the device in a trusted location.
3. To provision the outgoing link, expand **Backbone connection** and enter its DNS name, issuing CA PEM certificate and gateway token. The default port is 5056. Leave these fields blank to keep the previously saved backbone settings. A private `backbone_config.h` can also provide build-time defaults. An image built without that header accepts runtime settings in the portal.
4. Choose **Save and restart**. The firmware tests the new network for up to 30 seconds and commits it after receiving an IP address. If the test fails, it tries the previous settings. If there are no previous credentials, or the previous network also fails, it reopens `maarse-gate`. A later loss of Wi-Fi also triggers a reconnect attempt before setup is reopened.

The screen shows the setup address and code, or the current Wi-Fi address and backbone status. It sleeps after 30 seconds and a short press wakes it. Hold the user button for at least five seconds and release to clear the saved gateway settings and reboot into setup. The modem's LoRa identity lives separately.

## Build

Use the [gateway build script](../../README.md#build-on-windows). The outbound gateway is available for Heltec V4 OLED and TFT displays.

## Backbone transport

The gateway establishes the clock using SNTP before connecting. On the TLS
stream it sends `MAARSE-KISS/1 <gateway-token>\n` and waits for `OK\n`.
Following authentication, both directions use standard MeshCore KISS frames.
A new connection clears the modem's previous KISS session. The firmware
retries after disconnects. This variant has no inbound TCP listener.

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
