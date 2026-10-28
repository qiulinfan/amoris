"""Generate Emberlight's original audio: a seamless observatory drone, a lamp bell, a door rumble
and a dawn swell. Only Python's standard library is used; the WAVs are committed beside this
script because they are generated assets.

Run from the repository root:

    python3 samples/emberlight/build_audio.py
"""

import math
import random
import struct
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OUT = ROOT / "audio"
RATE = 44100
TAU = 2.0 * math.pi


def write(name, samples, peak=0.9):
    top = max(1e-9, max(abs(x) for x in samples))
    scale = peak / top
    OUT.mkdir(exist_ok=True)
    path = OUT / name
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        frames = bytearray()
        for x in samples:
            v = int(max(-1.0, min(1.0, x * scale)) * 32767.0)
            frames.extend(struct.pack("<h", v))
        w.writeframes(bytes(frames))
    print(f"{name}: {len(samples) / RATE:.2f} s, {path.stat().st_size / 1e6:.2f} MB")


def ambience(duration=24.0):
    """A looping D-minor drone with a soft periodic wind. Every sine completes an integer number
    of cycles over the loop, so the end meets the start without a fade or a click."""
    n = int(RATE * duration)
    rng = random.Random(19)
    # The harmonic bed: root, fifth, octave and a breathing minor third.
    roots = [(1, 55.0, 0.30, 0.0), (1, 73.42, 0.18, 1.2), (1, 110.0, 0.12, 2.4),
             (1, 146.83, 0.08, 0.6), (1, 174.61, 0.05, 3.1), (1, 220.0, 0.035, 4.2)]
    # A high, very quiet shimmer.
    for k in range(6):
        roots.append((1, 440.0 + 55.0 * k, 0.004, rng.uniform(0, TAU)))
    # Quantize every frequency to a whole number of cycles over the loop.
    quant = []
    for _, f, a, ph in roots:
        cycles = max(1, round(f * duration))
        quant.append((cycles / duration, a, ph))
    # A periodic wind: a spread of sines with 1/f amplitudes and random phases.
    wind = []
    for k in range(36):
        f = rng.uniform(35.0, 1400.0)
        cycles = max(1, round(f * duration))
        wind.append((cycles / duration, 0.018 / math.sqrt(1.0 + k), rng.uniform(0, TAU)))
    out = []
    for i in range(n):
        t = i / RATE
        v = 0.0
        for f, a, ph in quant:
            # Two detuned copies one loop-cycle apart make a beat that also loops.
            v += a * (math.sin(TAU * f * t + ph)
                      + 0.35 * math.sin(TAU * (f + 1.0 / duration) * t + ph * 1.7))
        breath = 0.72 + 0.28 * math.sin(TAU * t / duration * 2.0 + 0.3)
        air = 0.0
        for f, a, ph in wind:
            air += a * math.sin(TAU * f * t + ph)
        air *= 0.55 + 0.45 * math.sin(TAU * t / duration * 3.0 + 1.1)
        out.append((v * breath + air) * 0.6)
    return out


def bell(duration=3.4, fundamental=659.25):
    """A struck bronze bell: an inharmonic partial set, a fast strike and a long decay."""
    n = int(RATE * duration)
    partials = [(1.0, 0.55, 1.8), (2.01, 0.28, 1.5), (2.76, 0.22, 1.2),
                (4.07, 0.10, 0.9), (5.40, 0.07, 0.7), (8.93, 0.035, 0.45),
                (12.1, 0.015, 0.3)]
    rng = random.Random(5)
    out = []
    for i in range(n):
        t = i / RATE
        strike = 1.0 if t < 0.012 else 0.0
        v = 0.0
        for ratio, amp, decay in partials:
            env = math.exp(-decay * t) * (1.0 - math.exp(-min(t, 0.2) * 900.0))
            v += amp * env * math.sin(TAU * fundamental * ratio * t + ratio)
        # The hammer's noise, very short.
        if strike:
            v += rng.uniform(-1.0, 1.0) * 0.20
        out.append(v * 0.8)
    return out


def door(duration=3.0):
    """A slab of stone rising: a low grind that glides down and settles."""
    n = int(RATE * duration)
    rng = random.Random(23)
    low = 0.0
    out = []
    for i in range(n):
        t = i / RATE
        env = math.sin(math.pi * min(t / duration, 1.0)) ** 1.6
        f = 66.0 - 18.0 * min(t / duration, 1.0)
        low = 0.985 * low + 0.015 * rng.uniform(-1.0, 1.0)
        grind = (0.35 * math.sin(TAU * f * t)
                 + 0.15 * math.sin(TAU * (f * 2.02) * t + 1.0))
        out.append((grind + low * 2.2) * env)
    return out


def dawn(duration=6.0):
    """A rising major chord with a far bell at its crest."""
    n = int(RATE * duration)
    chord = [146.83, 185.0, 220.0, 293.66, 369.99, 440.0]
    out = []
    for i in range(n):
        t = i / RATE
        rise = (t / duration) ** 1.35
        v = 0.0
        for k, f in enumerate(chord):
            env = 0.25 + 0.75 * rise
            vib = 1.0 + 0.002 * math.sin(TAU * 0.35 * t + k)
            v += (0.16 / (1.0 + k * 0.4)) * env * math.sin(TAU * f * vib * t + k)
        # Bell partials bloom as the chord opens.
        for ratio, amp, decay in [(1.0, 0.18, 1.1), (2.76, 0.10, 0.8), (5.4, 0.05, 0.6)]:
            env = math.exp(-max(0.0, t - 1.2) * decay) * min(1.0, t / 1.2)
            v += amp * env * math.sin(TAU * 587.33 * ratio * t)
        out.append(v * 0.9)
    return out


def main():
    write("ambient.wav", ambience(), 0.42)
    write("lamp.wav", bell(), 0.7)
    write("door.wav", door(), 0.55)
    write("dawn.wav", dawn(), 0.65)


if __name__ == "__main__":
    main()
