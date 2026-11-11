# Generates RustCord's own notification ping: two short sine notes with a
# soft attack and an exponential fade. Original; no samples used.
import math, struct, sys
RATE = 22050
notes = [(880.0, 0.07), (1318.5, 0.15)]
samples = []
for freq, dur in notes:
    n = int(RATE * dur)
    for i in range(n):
        t = i / RATE
        attack = min(1.0, i / (RATE * 0.004))
        decay = math.exp(-t * (14.0 if dur > 0.1 else 6.0))
        tail = min(1.0, (n - i) / (RATE * 0.004))
        v = math.sin(2 * math.pi * freq * t) + 0.25 * math.sin(4 * math.pi * freq * t)
        samples.append(int(max(-1.0, min(1.0, 0.55 * v * attack * decay * tail)) * 32767))
data = b"".join(struct.pack("<h", s) for s in samples)
hdr = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, RATE, RATE * 2, 2, 16) + b"data" + struct.pack("<I", len(data))
open(sys.argv[1], "wb").write(hdr + data)
