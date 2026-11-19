"""Calibrate the simulator's radio model against field measurements.

The Denmark simulator predicts the received power of a link as

    rssi = tx_dbm + antenna gains + height gains - (L1m + 10 n log10 d) - offset

(maarse-sim radio.rs and maarse-sim-wasm engine.rs: L1m = free space at 1 m,
height gain = 10 log10(h / 1.5 m) per end, +6 dBi for fixed antennas). Out of
the box n = 3 and offset = 0. This tool fits n and offset to real range-test
receptions and prints the "radio_cal" block for a scenario file
(Build > Scenario file > Import), plus how far the old and new model are off.

Input: a CSV with one row per received beacon. Columns (names as below; extra
columns are ignored):

    distance_m            or  tx_lat, tx_lon, rx_lat, rx_lon
    rssi                  received power, dBm (tools/range_test.py writes it)
    tx_dbm                optional, default 22 (the firmware's default)
    tx_h, rx_h            optional antenna heights in metres, default 1.5
    tx_ant_db, rx_ant_db  optional antenna gains, default 0 (handhelds)
    freq_mhz              optional, default 869.525

tools/range_test.py --csv gives rssi; add the distance (or the positions from
the web client's Range test page) and the heights for each place you stood.
Use several distances (at least three, ideally 0.5-20 km) and note the
terrain; fit open land, town and forest separately.

    python tools/calibrate_radio.py drive.csv
    python tools/calibrate_radio.py drive.csv --json cal.json
"""
import argparse, csv, json, math, sys


def km(a, b):
    """Distance in km between (lat, lon) points (equirectangular, fine below 100 km)."""
    dy = (a[0] - b[0]) * 111.32
    dx = (a[1] - b[1]) * 111.32 * math.cos(math.radians((a[0] + b[0]) / 2))
    return math.hypot(dx, dy)


def height_db(h):
    return 10 * math.log10(max(h, 1.0) / 1.5)


def loss_1m(freq_mhz):
    return 31.2 if abs(freq_mhz - 868.0) < 1e-9 else 20 * math.log10(freq_mhz) - 27.55


def num(row, key, default):
    v = (row.get(key) or "").strip()
    return float(v) if v else default


def load(path):
    pts = []
    with open(path, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            try:
                if (row.get("distance_m") or "").strip():
                    d = float(row["distance_m"])
                else:
                    d = km((float(row["tx_lat"]), float(row["tx_lon"])), (float(row["rx_lat"]), float(row["rx_lon"]))) * 1000
                rssi = float(row["rssi"])
            except (KeyError, ValueError):
                continue
            if d < 1:
                continue
            f_mhz = num(row, "freq_mhz", 869.525)
            gains = num(row, "tx_ant_db", 0) + num(row, "rx_ant_db", 0) + height_db(num(row, "tx_h", 1.5)) + height_db(num(row, "rx_h", 1.5))
            # y = rssi - tx - gains + L1m = -10 n log10 d - offset
            y = rssi - num(row, "tx_dbm", 22) - gains + loss_1m(f_mhz)
            pts.append((math.log10(d), y, d, rssi))
    return pts


def fit(pts):
    n = len(pts)
    mx = sum(p[0] for p in pts) / n
    my = sum(p[1] for p in pts) / n
    sxx = sum((p[0] - mx) ** 2 for p in pts)
    if sxx < 1e-9:
        return None
    slope = sum((p[0] - mx) * (p[1] - my) for p in pts) / sxx
    icpt = my - slope * mx
    return -slope / 10, -icpt


def rms(pts, n, off):
    return math.sqrt(sum((p[1] - (-10 * n * p[0] - off)) ** 2 for p in pts) / len(pts))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("csv")
    ap.add_argument("--json", help="also write the radio_cal block to this file")
    a = ap.parse_args()
    pts = load(a.csv)
    if len(pts) < 3:
        sys.exit(f"Need at least 3 usable rows (distance and rssi); got {len(pts)}.")
    span = max(p[2] for p in pts) / min(p[2] for p in pts)
    r = fit(pts)
    if r is None or span < 3:
        sys.exit("The distances are too close together to fit a slope: measure at more different distances (at least 3x apart).")
    n, off = r
    print(f"{len(pts)} receptions, {min(p[2] for p in pts) / 1000:.2f}-{max(p[2] for p in pts) / 1000:.2f} km")
    print(f"model today (n = 3.00, offset 0.0 dB): error {rms(pts, 3.0, 0.0):.1f} dB rms")
    print(f"fitted      (n = {n:.2f}, offset {off:+.1f} dB): error {rms(pts, n, off):.1f} dB rms")
    if not 2.0 <= n <= 5.0:
        print("Warning: n outside 2-5 is unusual; check the distances, heights and tx power.")
    cal = {"radio_cal": {"path_n": round(n, 3), "offset_db": round(off, 2)}}
    print("\nPut this in the scenario file (top level):")
    print(json.dumps(cal, indent=2))
    if a.json:
        with open(a.json, "w", encoding="utf-8") as f:
            json.dump(cal, f, indent=2)


if __name__ == "__main__":
    main()
