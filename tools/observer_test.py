"""Loopback test of observer reports and the public channel feed.

Run cargo build -p maarse-host --bins first. Requires websocket-client.
Two modem byte streams stand in for LoRa receivers, so no hardware is needed.
"""
import json
import os
from pathlib import Path
import re
import socket
import struct
import subprocess
import sys
import tempfile
import time
from urllib.request import urlopen

import websocket


BIN = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug").resolve()
SUFFIX = ".exe" if os.name == "nt" else ""


def wait_for(check, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if result := check():
            return result
        time.sleep(0.1)
    raise AssertionError("Timed out waiting for the expected result")


def listen():
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    listener.settimeout(15)
    return listener


def endpoint(listener):
    return f"127.0.0.1:{listener.getsockname()[1]}"


def reserve_endpoint():
    with listen() as listener:
        return endpoint(listener)


def frame(body):
    return struct.pack("<H", len(body)) + body


def join(ws, command, arguments):
    ws.send_binary(frame(bytes([command]) + arguments))
    while True:
        data = ws.recv()
        while len(data) >= 2:
            size = struct.unpack_from("<H", data)[0]
            event, data = data[2:2 + size], data[2 + size:]
            if event and event[0] == 0x88:
                return event[1:17]


def send_channel(ws, channel, message):
    ws.send_binary(frame(b"\x03\x01\x00" + channel + message.encode()))


def main():
    with tempfile.TemporaryDirectory(prefix="maarse-observers-") as directory:
        root = Path(directory)
        children = []

        def start(name, *args):
            exe = "maarse-server" if name == "collector" else "maarse-host"
            data = root / name
            data.mkdir(exist_ok=True)
            with (root / f"{name}.log").open("wb") as log:
                proc = subprocess.Popen([str(BIN / (exe + SUFFIX)), "--data", str(data), "--no-browser", *args],
                                        stdout=log, stderr=subprocess.STDOUT)
            children.append(proc)
            return proc

        backbone = reserve_endpoint()
        http = reserve_endpoint()
        web = reserve_endpoint()
        keys = {}
        try:
            # Generate the three stable keys without connecting any nodes.
            for name in ("collector", "west", "east"):
                options = ("--public-addr", backbone, "--min-peers", "0") if name == "collector" else ()
                proc = start(name, "--listen", "off", "--http", "off", *options)
                pattern = r"Bulletin key \(for --bulletin-key on receivers\): ([a-f0-9]{128})"
                match = wait_for(lambda: re.search(pattern, (root / f"{name}.log").read_text(encoding="utf-8")))
                keys[name] = match[1]
                proc.terminate()
                proc.wait(timeout=5)

            start("collector", "--listen", backbone, "--public-addr", backbone, "--min-peers", "0", "--http", http,
                  "--public-map", "--observe-allow", keys["west"], "--observe-allow", keys["east"],
                  "--public-channel", "#open@dk")

            def feed():
                try:
                    with urlopen(f"http://{http}/api/public/network", timeout=1) as reply:
                        return json.load(reply)
                except OSError:
                    return {}

            wait_for(feed)
            with listen() as west_modem, listen() as east_modem:
                for name, modem in (("west", west_modem), ("east", east_modem)):
                    start(name, "--listen", "off", "--http", web if name == "west" else "off",
                          "--peer", backbone, "--observe-to", f"{backbone}@{keys['collector']}",
                          "--modem-tcp", endpoint(modem))
                with west_modem.accept()[0] as west, east_modem.accept()[0] as east:
                    # Both observers receive the same Hello over their modem,
                    # with distinct SNRs. Hello frames are local to the radio.
                    hello = bytes([12 << 2, 0]) + os.urandom(16) + bytes([0, 0, 255, 255])
                    west.sendall(frame(bytes([0xC0, 4]) + hello))
                    east.sendall(frame(bytes([0xC0, 9]) + hello))

                    def matching_group():
                        for group in feed().get("packets", []):
                            if group["kind"] != "Hello":
                                continue
                            reports = [o for o in group["observations"] if o["transport"] == "radio" and o["direction"] == "rx"]
                            if sorted(o["snr"] for o in reports) == [4, 9]:
                                return group

                    result = wait_for(matching_group)
                    assert result["observer_count"] == 2, result
                    today = feed()["radio_today"]
                    assert (today["unique_packets"], today["observations"], today["sites"]) == (1, 2, 2), today
                    print("PASS two modem observations group under one packet via existing TCP peers")

                    ws = websocket.create_connection(f"ws://{web}/ws", origin=f"http://{web}", timeout=5)
                    try:
                        private = join(ws, 0x13, os.urandom(32) + b"#open@dk")
                        send_channel(ws, private, "Private marker")
                        unlisted = join(ws, 0x04, b"#unlisted")
                        send_channel(ws, unlisted, "Unlisted marker")
                        global_channel = join(ws, 0x04, b"#open")
                        send_channel(ws, global_channel, "Wrong region marker")
                        allowed = join(ws, 0x04, b"#open@dk")
                        send_channel(ws, allowed, "Shared channel message <b>&")
                        def view_with_messages():
                            view = feed()
                            return view if view.get("messages") else None

                        view = wait_for(view_with_messages)
                        assert view["channels"] == ["#open@dk"], view
                        assert [m["text"] for m in view["messages"]] == ["Shared channel message <b>&"], view
                        print("PASS only the explicitly shared open channel and wire region are published")
                    finally:
                        ws.close()
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=5)


if __name__ == "__main__":
    main()
