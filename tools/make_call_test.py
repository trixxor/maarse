"""Makes music.bin for the call test service from any audio file.

    python tools/make_call_test.py song.mp3 <data>/calltest/music.bin

Encodes with ffmpeg to Opus as calls use it (48 kHz mono, 20 ms frames,
16 kbit/s) and stores each frame as u16 LE length | frame.
"""
import os, struct, subprocess, sys, tempfile


def ogg_packets(data):
    """Yields the packets of an Ogg stream."""
    i, part = 0, b""
    while i + 27 <= len(data):
        assert data[i:i + 4] == b"OggS", "not an Ogg stream"
        n = data[i + 26]
        lacing = data[i + 27:i + 27 + n]
        i += 27 + n
        for l in lacing:
            part += data[i:i + l]; i += l
            if l < 255:
                yield part; part = b""


def main(src, dst):
    with tempfile.TemporaryDirectory() as t:
        ogg = os.path.join(t, "a.ogg")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", src, "-ac", "1", "-ar", "48000", "-c:a", "libopus",
                        "-b:a", "16k", "-frame_duration", "20", "-application", "audio", ogg], check=True)
        packets = list(ogg_packets(open(ogg, "rb").read()))[2:]  # skip OpusHead and OpusTags
    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    with open(dst, "wb") as f:
        for p in packets:
            assert 0 < len(p) <= 255, f"frame of {len(p)} bytes"
            f.write(struct.pack("<H", len(p)) + p)
    print(f"{dst}: {len(packets)} frames ({len(packets) * 0.02:.0f} s)")


if __name__ == "__main__":
    main(*sys.argv[1:3])
