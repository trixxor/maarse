# Tutorial: your first Må:rse node

> **Type:** tutorial · **Reader:** a complete beginner with a Windows or Linux PC · **Time:** about 15 minutes after the first build (the first build takes a few minutes)
>
> **Status:** `working copy`. The steps come from `scripts/start-host.*`, [GUIDE.md](../GUIDE.md#getting-started-windows-and-linux) and `tools/e2e_test.py`. **This walkthrough has not been re-run from a clean machine since it was written (2026-11-27).** If a step does not give the result described, that is a bug in this page: fix it here.

You will start a Må:rse host on your computer, open its web client in the
browser, and find your own address. Then you will start a second node on the
same computer and send a message between the two. No radio is needed.

## Before you start

- A Windows or Linux computer with internet access.
- About 2 GB free disk space (the Rust build).
- A recent Chrome or Edge.

## 1. Install Rust

Install Rust from <https://rustup.rs>. Accept the defaults.

Check it worked. Open a new terminal and run:

```text
cargo --version
```

You should see a version number such as `cargo 1.xx.x`. If the command is not
found, close and reopen the terminal.

## 2. Get the code and start the host

```text
git clone https://github.com/trixxor/maarse.git
cd maarse
```

Start the host:

- **Windows:** double-click `scripts\start-host.bat`
- **Linux:** run `./scripts/start-host.sh`

The first start builds the program, which takes a few minutes. After that it
starts right away.

**You should see:** a console window with log lines, and your browser opening
`http://127.0.0.1:8080`. The web client connects by itself.

## 3. Find your address

In the web client, open the page that shows your identity (the *Status* page or
the top of the main page). You should see a **16-byte address** written as 32
hexadecimal digits, for example `3eb445ec4cad3c49…`.

This address is a hash of your public key. It is the same over radio, over TCP
and over the internet. Nobody gave it to you: your host made it the first time
it started and stored it in the `maarse-data` folder.

## 4. Start a second node

A message needs someone to send it to. Start a second host on the same computer
with its own data folder and ports. In a second terminal, in the `maarse` folder:

```text
cargo run --release -p maarse-host --bin maarse-host -- --data maarse-data-b --http 127.0.0.1:8081 --listen 127.0.0.1:4243 --peer 127.0.0.1:4242
```

(This is the one place in the documentation where command-line options are the
right tool: two hosts on one computer need different folders and ports, and
`settings.txt` is per data folder. For everything else use the web client's
Settings. See the [settings reference](../reference/settings.md).)

**You should see:** a second browser window or tab at `http://127.0.0.1:8081`
with a different address. Within a few seconds the first node should show up
there as a contact, because hosts announce themselves over the link between them.

## 5. Send a message

In the second client, open the contact for the first node and send a message
such as `hello`.

**You should see:** the message appear in the first client, and a *delivered*
mark appear next to it in the second. The *delivered* mark is the end-to-end
acknowledgement: it means the other node decrypted the message.

## What you did

- You ran two Må:rse nodes with separate identities.
- They found each other over a TCP link and exchanged an end-to-end encrypted
  message with a delivery confirmation.
- Your messages and keys are stored in the two data folders.

## Next steps

- Add a real radio: [How to flash a board](../guides/flash-a-board.md).
- Run a node that stays on all the time: [How to run a server](../guides/run-a-server.md).
- Understand what happened: [Architecture overview](../architecture/overview.md) and [Routing](../concepts/routing.md).
- Look up every option: [Settings reference](../reference/settings.md).
