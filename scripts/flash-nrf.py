"""Flasher en Må:rse-UF2 på et nRF52840-board uden at trykke på reset.

Brug: python scripts/flash-nrf.py <COM-port> <fil.uf2>

Åbner porten med 1200 baud og lukker den igen ("1200 baud-touch"), så
firmwaren genstarter i UF2-bootloaderen. Derefter findes USB-drevet (det
med INFO_UF2.TXT), og filen kopieres over. Virker kun, hvis boardet
allerede kører Må:rse-firmware med 1200 baud-touch; ellers tryk to gange
på reset og kør scriptet igen (det venter på drevet).
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
            print(f"1200 baud-touch sendt til {port}; venter på bootloader-drevet ...")
        except serial.SerialException as e:
            print(f"Kunne ikke åbne {port} ({e}). Tryk to gange på reset; jeg venter på drevet ...")
    for _ in range(300):
        drives = uf2_drives()
        if drives:
            break
        time.sleep(0.1)
    else:
        print("Fandt intet UF2-drev efter 30 s.")
        return 1
    drive = drives[0]
    info = open(os.path.join(drive, "INFO_UF2.TXT"), errors="replace").read().strip().splitlines()
    print(f"Drev {drive}: " + " | ".join(info[:3]))
    print(f"Kopierer {uf2} ...")
    try:
        shutil.copyfile(uf2, os.path.join(drive, os.path.basename(uf2)))
    except OSError:
        pass  # Boardet genstarter, så snart filen er skrevet; det kan give en fejl til sidst.
    print("Færdig. Tryk én gang på reset, så USB kommer op igen (kendt fejl efter UF2-flash).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
