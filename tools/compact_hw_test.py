"""Hardware test of the compact forward-secrecy frame between two Må:rse radios.

Sends a series of 40-byte direct messages both ways (alternating, then bursts
in one direction) and prints, per message, whether it arrived and was
acknowledged and how much airtime the two radios used for it (stats key 40,
the sum over both nodes: the message and its acknowledgement). The first
messages of a session are the long frame; later ones the compact frame, so
the airtime per message drops.

Usage: python tools/compact_hw_test.py COM6 COM7
"""
import struct
import sys
import time

from radio_test import Node, pump_all


def airtime(node):
    node.send(0x09)
    for typ, b in node.events(2):
        if typ == 0x91:
            items = {}
            for i in range(0, len(b) - 4, 5):
                k, v = struct.unpack_from("<Bi", b, i)
                items[k] = v
            return items.get(40, 0), items.get(32, 0)
    return None, None


def main():
    nodes = [Node(p) for p in sys.argv[1:3]]
    time.sleep(0.5)
    for n in nodes:
        n.send(0x0f, struct.pack("<I", int(time.time())))
        n.send(0x01)
        n.events(1).__next__ if False else None
        for _ in n.events(2):
            pass
    # Info frames were consumed above without logging; ask again to get addresses.
    for n in nodes:
        n.send(0x01)
        for typ, b in n.events(2):
            if typ == 0x81:
                n.addr = b[:16]
    if not all(n.addr for n in nodes):
        print("FAIL: no address from both nodes")
        return 1
    a, b = nodes
    plan = [(a, b), (b, a)] * 6 + [(a, b)] * 8 + [(b, a)] * 8
    rows = []
    ok_all = True
    prev = [airtime(n)[0] for n in nodes]
    for i, (src, dst) in enumerate(plan):
        txt = (f"msg {i:02d} " + "x" * 40)[:40].encode()
        src.send(0x02, struct.pack("<H", 100 + i) + dst.addr + txt)
        seen = pump_all(nodes, 12)
        got = any(t == 0x83 and txt in body for t, body in seen[dst.port])
        acked = any(t == 0x85 for t, _ in seen[src.port])
        cur = [airtime(n)[0] for n in nodes]
        used = sum((c or 0) - (p or 0) for c, p in zip(cur, prev))
        prev = cur
        ok_all &= got and acked
        rows.append((i, src.port, got, acked, used))
        print(f"  #{i:02d} {src.port}->{dst.port} received={got} acked={acked} airtime(both radios)={used} ms")
    print("RESULT:", "OK" if ok_all else "FAIL")
    first = [r[4] for r in rows[:4] if r[4] > 0]
    later = [r[4] for r in rows[12:] if r[4] > 0]
    if first and later:
        print(f"average airtime per message: first 4 = {sum(first)/len(first):.0f} ms, later = {sum(later)/len(later):.0f} ms")
    return 0 if ok_all else 1


if __name__ == "__main__":
    sys.exit(main())
