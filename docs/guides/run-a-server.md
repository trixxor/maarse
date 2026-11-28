# How to run a server

> **Type:** how-to · **Goal:** run `maarse-server` on a machine with a public address, so it acts as backbone node, DHT node and post office · **Assumes:** a Debian or Ubuntu machine with root access and an open TCP port.
>
> **Status:** installer `released`; Settings tab and hardening in the `working copy`. Long-form details: [GUIDE.md, Dedicated server](../GUIDE.md#dedicated-server-maarse-server).

## Install

```text
curl -fsSL https://raw.githubusercontent.com/trixxor/maarse/main/scripts/install-server.sh -o install-server.sh
sudo bash install-server.sh
```

The script installs the packages and Rust, builds `maarse-server` from source
(5-15 minutes the first time) and asks for your public address, port, peers,
regions, post office and quota, and how to reach the dashboard (HTTPS through
Caddy, an SSH tunnel, or plain HTTP). It creates the `maarse` system user and a
hardened systemd service, and opens the ports in `ufw` if it is active. It
prints the admin password once.

**Check:** the service is running and the dashboard asks for the admin password.

```text
systemctl status maarse-server
```

## Change settings

Open the dashboard and use the **Settings** tab. *Save and restart* writes
`settings.txt` in `/var/lib/maarse-server` and restarts the server. You can edit
`settings.txt` by hand too, one option per line. The full list is the
[settings reference](../reference/settings.md).

## Update

Run the installer again. Your settings are kept unless you choose to change them.

## Put the dashboard on HTTPS

Run the web listener on loopback with the access key required, and put Caddy in
front of it. The Caddyfile and options are in
[GUIDE.md](../GUIDE.md#public-web-access-with-caddy-and-lets-encrypt). Keep the
backbone TCP port restricted if the backbone is private.

## Files

| Path | Contents |
| --- | --- |
| `/usr/local/bin/maarse-server` | the program |
| `/var/lib/maarse-server/settings.txt` | your settings |
| `/var/lib/maarse-server` | identity, history, mail, telemetry |
| `/var/lib/maarse-server-state` | a counter that notices a restored old data folder; leave it out of data backups. The server sends no encrypted messages if it cannot write it (since 0.9.13) |
| `/opt/maarse/src` | source used to build |

Related: [Architecture overview](../architecture/overview.md) · [Post offices and carry](../concepts/post-offices-and-carry.md) · [Threat model](../security/threat-model.md)
