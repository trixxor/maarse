"""Flashes a Må:rse UF2 onto an nRF52840 board without pressing reset.

Usage: python scripts/flash-nrf.py <COM port> <file.uf2>

Opens the port at 1200 baud and closes it again ("1200 baud touch"), so
the firmware restarts into the UF2 bootloader. Then the USB drive (the one
with INFO_UF2.TXT) is found, and the file is copied over. Only works if the board
already runs Må:rse firmware with 1200 baud touch; otherwise press reset twice
and run the script again (it waits for the drive).
"""
import os, shutil, string, sys, time

import serial


def uf2_drives():
    if os.name == "nt":
        roots = [f"{d}:\\" for d in string.ascii_uppercase]
    else:
        roots = [os.path.join(b, n) for b in ("/media/" + os.environ.get("USER", ""), "/run/media/" + os.environ.get("USER", ""), "/Volumes")
                 if os.path.isdir(b) for n in os.listdir(b)]
    return [r for r in roots if os.path.isfile(os.path.join(r, "INFO_UF2.TXT"))]


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        return 1
    port, uf2 = sys.argv[1], sys.argv[2]
    before = set(uf2_drives())
    if not before:
        try:
            s = serial.Serial(port, 1200)
            time.sleep(0.1)
            s.dtr = False
            s.close()
            print(f"1200 baud touch sent to {port}; waiting for the bootloader drive ...")
        except serial.SerialException as e:
            print(f"Could not open {port} ({e}). Press reset twice; waiting for the drive ...")
    for _ in range(300):
        drives = uf2_drives()
        if drives:
            break
        time.sleep(0.1)
    else:
        print("Found no UF2 drive after 30 s.")
        return 1
    drive = drives[0]
    info = open(os.path.join(drive, "INFO_UF2.TXT"), errors="replace").read().strip().splitlines()
    print(f"Drive {drive}: " + " | ".join(info[:3]))
    print(f"Copying {uf2} ...")
    try:
        shutil.copyfile(uf2, os.path.join(drive, os.path.basename(uf2)))
    except OSError:
        pass  # The board restarts as soon as the file is written; that can give an error at the end.
    print("Done. USB comes back after about a minute (the first start hangs and the watchdog restarts the board).
"
          "If it has not after two minutes, or the firmware is older than the watchdog, press reset once.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
