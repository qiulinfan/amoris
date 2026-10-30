#!/usr/bin/env python3
"""Synthesizes Thornkeep's sounds into audio/*.wav (44.1 kHz, 16-bit mono). Original, no samples.

    python samples/thornkeep/build_audio.py

The fight's events play these at their subject (project.toml's [sounds]); the wind and the
braziers' fire are looped AudioSources (build_scene.py). Blades are inharmonic partials (a struck
bar's modes), blows are a low thump under a noise crack, cloth and dirt are band-passed noise; each
event with several variants gets the variant number from the script's `variant` field. The loops'
noise is filtered circularly over the whole loop, so their ends meet their starts.
"""
from pathlib import Path
import wave

import numpy as np

RATE = 44100
OUT = Path(__file__).resolve().parent/'audio'
rng = np.random.default_rng(11)


def write(name, x, peak=0.8):
    x = np.asarray(x, dtype=np.float64)
    x = x / max(1e-9, np.max(np.abs(x))) * peak
    OUT.mkdir(exist_ok=True)
    with wave.open(str(OUT/name), 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes((x * 32767).astype('<i2').tobytes())
    print(name, f'{len(x) / RATE:.2f} s')


def t_of(seconds):
    return np.arange(int(seconds * RATE)) / RATE


def band(noise, lo, hi):
    """Circular band-pass: the spectrum outside lo..hi Hz removed, smoothly."""
    f = np.fft.rfftfreq(len(noise), 1 / RATE)
    s = np.fft.rfft(noise)
    gain = 1 / (1 + (lo / np.maximum(f, 1)) ** 4) / (1 + (f / hi) ** 4)
    return np.fft.irfft(s * gain, len(noise))


def envelope(n, attack, decay):
    t = np.arange(n) / RATE
    return np.minimum(1, t / max(attack, 1e-4)) * np.exp(-t / decay)


def sweep_noise(seconds, f0, f1, q=0.35):
    """Noise through a band whose centre glides f0 -> f1 (a blade or a body through the air)."""
    n = int(seconds * RATE)
    out = np.zeros(n)
    hop = 512
    noise = rng.standard_normal(n + hop)
    for i in range(0, n, hop):
        c = f0 * (f1 / f0) ** (i / n)
        seg = band(noise[i:i + hop * 2], c * (1 - q), c * (1 + q))[:min(hop, n - i)]
        out[i:i + len(seg)] = seg
    return out


def struck(seconds, f, partials, decay, bright=1.0):
    """A struck metal bar: inharmonic modes, the higher dying faster."""
    t = t_of(seconds)
    x = np.zeros_like(t)
    for ratio, amp in partials:
        x += amp * bright ** (ratio - 1) * np.sin(2 * np.pi * f * ratio * t + rng.uniform(0, 6)) \
            * np.exp(-t / (decay / ratio ** 0.6))
    return x


BAR = [(1.0, 1.0), (2.76, 0.6), (5.40, 0.4), (8.93, 0.25), (13.34, 0.15)]


def thump(seconds, f0, f1, decay):
    t = t_of(seconds)
    phase = 2 * np.pi * np.cumsum(f0 * (f1 / f0) ** (t / seconds)) / RATE
    return np.sin(phase) * envelope(len(t), 0.002, decay)


def crack(seconds, lo, hi, decay):
    n = int(seconds * RATE)
    return band(rng.standard_normal(n), lo, hi) * envelope(n, 0.001, decay)


def swing(v):
    s = 0.32 + 0.04 * v
    x = sweep_noise(s, 500 + 120 * v, 2600 + 300 * v)
    t = t_of(s)
    return x * np.sin(np.pi * np.minimum(1, t / s)) ** 2


def hit(v):
    s = 0.45
    x = 1.0 * thump(s, 120 - 10 * v, 50, 0.09) + 0.5 * crack(s, 300, 2500, 0.04 + 0.01 * v)
    return x + 0.25 * crack(s, 2000, 7000, 0.015)


def block():
    s = 0.7
    return 0.8 * struck(s, 410, BAR, 0.18, 0.8) + 0.6 * thump(s, 160, 70, 0.05) + 0.4 * crack(s, 800, 5000, 0.02)


def parry():
    s = 1.4
    return struck(s, 880, BAR, 0.5, 1.0) + 0.3 * crack(s, 3000, 9000, 0.01)


def clang(v):
    s = 0.9
    return struck(s, 620 + 70 * v, BAR, 0.3, 0.9) + 0.3 * crack(s, 1500, 7000, 0.015)


def impact():
    s = 1.0
    return 1.0 * thump(s, 70, 32, 0.25) + 0.6 * crack(s, 60, 900, 0.15) + 0.2 * crack(s, 900, 4000, 0.05)


def roll():
    s = 0.6
    n = int(s * RATE)
    t = t_of(s)
    cloth = band(rng.standard_normal(n), 300, 3000) * np.sin(np.pi * t / s) ** 1.5
    ground = 0.6 * thump(s, 90, 45, 0.06)
    return cloth + np.roll(ground, int(0.32 * RATE))


def step(v):
    s = 0.22
    return 0.8 * thump(s, 95 + 8 * v, 50, 0.03) + 0.35 * crack(s, 400, 3000, 0.025)


def death():
    s = 1.6
    fall = np.roll(1.0 * thump(s, 80, 35, 0.2) + 0.5 * crack(s, 100, 1500, 0.12), int(0.35 * RATE))
    sigh = sweep_noise(s, 700, 250, 0.25) * envelope(int(s * RATE), 0.05, 0.25)
    return fall + 0.4 * sigh


def drink():
    s = 1.3
    t = t_of(s)
    x = np.zeros_like(t)
    for k in range(4):
        at = int((0.15 + 0.26 * k) * RATE)
        g = t_of(0.12)
        gulp = np.sin(2 * np.pi * np.cumsum(260 - 900 * g) / RATE) * np.exp(-g / 0.04)
        x[at:at + len(gulp)] += gulp
    return x + 0.1 * band(rng.standard_normal(len(t)), 1500, 5000) * envelope(len(t), 0.05, 0.4)


def horn():
    """A guard's alarm: a short, rough bone horn, two notes."""
    s = 1.3
    t = t_of(s)
    f = np.where(t < 0.45, 196.0, 233.0)
    phase = 2 * np.pi * np.cumsum(f * (1 + 0.004 * np.sin(2 * np.pi * 5.5 * t))) / RATE
    tone = sum(np.sin(k * phase) / k ** 0.8 for k in range(1, 9))
    env = np.minimum(1, t / 0.06) * np.where(t > s - 0.25, (s - t) / 0.25, 1)
    return tone * env + 0.15 * band(rng.standard_normal(len(t)), 200, 1800) * env


def lock():
    s = 0.12
    return struck(s, 1800, [(1.0, 1.0), (2.0, 0.3)], 0.02)


def launch():
    s = 0.7
    whoosh = sweep_noise(s, 3000, 600, 0.4) * envelope(int(s * RATE), 0.01, 0.25)
    spark = 0.4 * struck(s, 1300, BAR, 0.08, 1.0)
    return whoosh + spark


def roar():
    """The Warden's second phase: a low growl over grinding thorn-wood."""
    s = 2.6
    t = t_of(s)
    f = 70 + 25 * np.sin(2 * np.pi * 0.7 * t)
    phase = 2 * np.pi * np.cumsum(f) / RATE
    growl = sum(np.sin(k * phase + rng.uniform(0, 6)) / k for k in range(1, 14))
    growl *= 1 + 0.5 * band(rng.standard_normal(len(t)), 10, 40)
    creak = band(rng.standard_normal(len(t)), 300, 1200) * (1 + np.sin(2 * np.pi * 9 * t)) * 0.4
    env = np.minimum(1, t / 0.3) * np.exp(-np.maximum(0, t - 1.6) / 0.4)
    return (growl + creak) * env


def chime(freqs, s=3.0, decay=1.2):
    x = sum(struck(s, f, [(1.0, 1.0), (2.0, 0.25), (3.0, 0.1), (4.2, 0.06)], decay, 1.0) for f in freqs)
    return x


def wind():
    seconds = 30.0
    t = t_of(seconds)
    n = len(t)
    low = band(rng.standard_normal(n), 40, 380)
    high = band(rng.standard_normal(n), 800, 3600)
    gust = 0.6 + 0.4 * np.sin(2 * np.pi * t / 10 + 0.7) * np.sin(2 * np.pi * t / 6 + 1.3)
    return low * (0.5 + 0.5 * gust) + 0.35 * high * gust ** 2


def fire():
    seconds = 12.0
    n = int(seconds * RATE)
    roar_ = band(rng.standard_normal(n), 60, 600)
    x = 0.6 * roar_
    # Crackles: short bright pops at random, wrapping round the loop's end.
    for _ in range(140):
        at = rng.integers(0, n)
        pop = crack(0.03, 1500, 8000, 0.004) * rng.uniform(0.3, 1.5)
        idx = (at + np.arange(len(pop))) % n
        x[idx] += pop
    return x


def main():
    for v in range(1, 4):
        write(f'swing{v}.wav', swing(v), 0.5)
        write(f'hit{v}.wav', hit(v), 0.9)
        write(f'clang{v}.wav', clang(v), 0.6)
        write(f'step{v}.wav', step(v), 0.35)
    write('block.wav', block(), 0.8)
    write('parry.wav', parry(), 0.8)
    write('impact.wav', impact(), 0.9)
    write('roll.wav', roll(), 0.5)
    write('death.wav', death(), 0.8)
    write('drink.wav', drink(), 0.6)
    write('horn.wav', horn(), 0.6)
    write('lock.wav', lock(), 0.3)
    write('launch.wav', launch(), 0.6)
    write('roar.wav', roar(), 0.9)
    write('shrine.wav', chime([392.0, 587.3]), 0.6)
    write('freed.wav', chime([261.6, 329.6, 392.0, 523.3], 6.0, 2.5), 0.7)
    write('wind.wav', wind(), 0.45)
    write('fire.wav', fire(), 0.5)


if __name__ == '__main__':
    main()
