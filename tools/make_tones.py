# Writes the built-in notification tones as WAV files for the Android app
# (android/app/src/main/res/raw). The web client synthesizes the same notes in
# JavaScript (TONES in maarse-client/index.html): keep the two tables equal.
import math, os, struct, wave

RATE = 22050
# name: list of (frequency Hz, start s, length s, harmonics)
TONES = {
    "tone_ding": [(1320, 0.00, 0.45, 1)],
    "tone_chime": [(880, 0.00, 0.35, 1), (1175, 0.16, 0.5, 1)],
    "tone_pop": [(520, 0.00, 0.10, 1)],
    "tone_drops": [(1000, 0.00, 0.12, 1), (800, 0.13, 0.12, 1), (1000, 0.26, 0.16, 1)],
    "tone_bell": [(660, 0.00, 0.8, 2)],
    "tone_low": [(330, 0.00, 0.30, 1), (247, 0.18, 0.40, 1)],
}

out = os.path.join("android", "app", "src", "main", "res", "raw")
os.makedirs(out, exist_ok=True)
for name, notes in TONES.items():
    total = max(s + d for _, s, d, _ in notes) + 0.05
    buf = [0.0] * int(total * RATE)
    for f, s, d, h in notes:
        for i in range(int(d * RATE)):
            t = i / RATE
            env = math.exp(-5.0 * t / d) * min(1.0, t / 0.004)
            v = sum(math.sin(2 * math.pi * f * (k + 1) * t) / (k + 1) ** 2 for k in range(h))
            j = int(s * RATE) + i
            if j < len(buf):
                buf[j] += 0.45 * env * v
    with wave.open(os.path.join(out, name + ".wav"), "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", max(-32767, min(32767, int(x * 32767)))) for x in buf))
    print(name, len(buf) / RATE, "s")
