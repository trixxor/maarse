"""Loopback test of observer reports and the public channel feed.

Run cargo build -p maarse-host --bins first. Requires websocket-client.
Two modem byte streams stand in for LoRa receivers. A separate scenario
exercises a TCP-only position-sharing node and automatic channel names.
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
    name = arguments[32:] if command == 0x13 else arguments
    ws.send_binary(frame(bytes([command]) + arguments))
    while True:
        data = ws.recv()
        while len(data) >= 2:
            size = struct.unpack_from("<H", data)[0]
            event, data = data[2:2 + size], data[2 + size:]
            if event and event[0] == 0x88 and event[17:] == name:
                return event[1:17]


def send_channel(ws, channel, message):
    ws.send_binary(frame(b"\x03\x01\x00" + channel + message.encode()))


def public_api(http, resource):
    try:
        with urlopen(f"http://{http}/api/public/{resource}", timeout=1) as reply:
            return json.load(reply)
    except OSError:
        return {}


def test_tcp_node(start):
    backbone = reserve_endpoint()
    collector_http = reserve_endpoint()
    relay = reserve_endpoint()
    web = reserve_endpoint()
    start("collector-auto", "--listen", backbone, "--public-addr", backbone,
          "--min-peers", "0", "--http", collector_http, "--public-map",
          "--public-traffic", "--share-open-channels")
    wait_for(lambda: public_api(collector_http, "network"))
    start("relay", "--listen", relay, "--http", "off", "--peer", backbone)
    start("phone", "--listen", "off", "--http", web, "--peer", relay)
    wait_for(lambda: public_api(collector_http, "network").get("packets"))
    ws = websocket.create_connection(f"ws://{web}/ws", origin=f"http://{web}", timeout=5)
    try:
        name = "TCP-only test node"
        ws.send_binary(frame(b"\x24" + name.encode()))
        # Choosing a precision before the phone has a GPS fix leaves no marker.
        ws.send_binary(frame(struct.pack("<BBiiB", 0x22, 2, 0, 0, 0)))
        assert all(n["name"] != name for n in public_api(collector_http, "map").get("nodes", []))
        ws.send_binary(frame(struct.pack("<BBiiB", 0x22, 2, 568500123, 99210023, 0)))
        ws.send_binary(frame(b"\x05"))

        def map_node():
            return next((n for n in public_api(collector_http, "map").get("nodes", [])
                         if n["name"] == name), None)

        marker = wait_for(map_node, timeout=30)
        assert (marker["lat"], marker["lon"], marker["precision"]) == (56.855, 9.925, 2), marker
        view = public_api(collector_http, "network")
        assert any(o.get("subject") == marker["addr"] and o["transport"] == "backbone"
                   for p in view["packets"] if p["kind"] == "Announce"
                   for o in p["observations"]), view
        print("PASS TCP-only node position and verified announce without observer registration")

        private = join(ws, 0x13, os.urandom(32) + b"#new channel@dk")
        send_channel(ws, private, "Private invite marker")
        public = join(ws, 0x04, b"public")
        send_channel(ws, public, "Built-in public marker")
        newly_open = join(ws, 0x04, b"#new channel@dk")
        send_channel(ws, newly_open, "New open marker")

        def both_messages():
            view = public_api(collector_http, "network")
            return view if len(view.get("messages", [])) >= 2 else None

        view = wait_for(both_messages)
        assert set(view["channels"]) == {"public", "#new channel@dk"}, view
        assert {m["text"] for m in view["messages"]} == {"Built-in public marker", "New open marker"}, view
        assert any(p["kind"] == "Channel" for p in view["packets"]), view
        print("PASS built-in and new regional open channels across two TCP links, private text excluded")

        ws.send_binary(frame(struct.pack("<BBiiB", 0x22, 0, 568500123, 99210023, 0)))
        wait_for(lambda: not map_node(), timeout=30)
        assert all(m["text"] != "Private invite marker"
                   for m in public_api(collector_http, "network")["messages"])
        print("PASS switching off position sharing removes the node from the public map")
    finally:
        ws.close()


def main():
    with tempfile.TemporaryDirectory(prefix="maarse-observers-") as directory:
        root = Path(directory)
        children = []

        def start(name, *args):
            exe = "maarse-server" if name.startswith("collector") else "maarse-host"
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
                return public_api(http, "network")

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
            test_tcp_node(start)
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=5)


if __name__ == "__main__":
    main()
