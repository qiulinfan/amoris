#!/usr/bin/env python3
"""Synthesizes Wellspring's sounds into audio/*.wav (44.1 kHz, 16-bit mono). Original, no samples.

    python samples/wellspring/build_audio.py

- wind.wav: a 32 s seamless loop of highland wind, gusting, with far-off bird calls.
- turn.wav: the iron wheel's quarter turn, a ratchet and a creak.
- open.wav: the gate grinding up its grooves and the released water's rush.
- handle.wav: a bronze handle picked up, a small chime.
- bell.wav: the shrine's bronze bell, struck once, ringing out for nine seconds.
- reached.wav: the river reaching the lake, a low swell.
The loop's noise is filtered circularly (in the frequency domain over the whole loop), so its end
meets its start without a fade.
"""
from pathlib import Path
import wave

import numpy as np

RATE = 44100
OUT = Path(__file__).resolve().parent/'audio'
rng = np.random.default_rng(7)


def write(name, x, peak=0.8):
    x = np.asarray(x, dtype=np.float64)
    x = x / max(1e-9, np.max(np.abs(x))) * peak
    OUT.mkdir(exist_ok=True)
    with wave.open(str(OUT/name), 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes((x * 32767).astype('<i2').tobytes())
    print(name, f'{len(x) / RATE:.1f} s')


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


def wind():
    seconds = 32.0
    t = t_of(seconds)
    n = len(t)
    low = band(rng.standard_normal(n), 40, 420)
    high = band(rng.standard_normal(n), 900, 4200)
    # Gusts: slow periodic swells whose periods divide the loop, so it joins.
    gust = 0.55 + 0.25 * np.sin(2 * np.pi * t * 3 / seconds) + 0.2 * np.sin(2 * np.pi * t * 7 / seconds + 1.3)
    whistle = 0.5 + 0.5 * np.sin(2 * np.pi * t * 5 / seconds + 0.4)
    x = low * gust / np.std(low) * 0.5 + high * gust ** 2 * whistle / np.std(high) * 0.08
    # Far-off birds: short descending two-note calls at a few places in the loop.
    for at, f0 in ((3.1, 2900), (11.7, 3300), (12.3, 3150), (21.4, 2700), (27.9, 3500)):
        for k, (dt, f) in enumerate(((0, f0), (0.16, f0 * 0.82))):
            m = int(0.12 * RATE)
            s = int((at + dt) * RATE)
            tt = np.arange(m) / RATE
            call = np.sin(2 * np.pi * (f * tt - f * 0.25 * tt ** 2 / 0.12)) * np.sin(np.pi * tt / 0.12) ** 2
            x[s:s + m] += call * 0.05
    return x


def turn():
    t = t_of(1.4)
    x = np.zeros(len(t))
    # Ratchet: eight pawl clicks, each a short metallic ping.
    for k in range(8):
        s = int((0.08 + k * 0.13) * RATE)
        m = int(0.05 * RATE)
        tt = np.arange(m) / RATE
        ping = (np.sin(2 * np.pi * 2100 * tt) + 0.6 * np.sin(2 * np.pi * 3470 * tt)) * np.exp(-tt / 0.008)
        x[s:s + m] += ping * (0.8 + 0.2 * rng.random())
    # Creak: a rough, slowly sliding tone.
    f = 180 + 60 * np.sin(2 * np.pi * 0.9 * t)
    phase = 2 * np.pi * np.cumsum(f) / RATE
    creak = np.sign(np.sin(phase)) * (0.5 + 0.5 * rng.random(len(t)))
    creak = band(creak, 150, 1800) * np.sin(np.pi * t / t[-1]) ** 2
    return x + creak / np.std(creak) * 0.12


def open_gate():
    t = t_of(4.5)
    n = len(t)
    grind = band(rng.standard_normal(n), 60, 900) * np.clip(1.2 - t / 2.2, 0, 1) * np.minimum(1, t / 0.2)
    rush = band(rng.standard_normal(n), 200, 6000) * np.clip((t - 0.8) / 1.5, 0, 1) * np.clip((4.5 - t) / 1.2, 0, 1)
    thump = np.sin(2 * np.pi * 55 * t) * np.exp(-t / 0.25)
    return grind / np.std(grind) * 0.4 + rush / np.std(rush) * 0.35 + thump * 0.5


def handle():
    t = t_of(1.6)
    x = sum(a * np.sin(2 * np.pi * f * t) * np.exp(-t / d)
            for f, a, d in ((1318, 1.0, 0.5), (1976, 0.6, 0.35), (2637, 0.35, 0.25), (3951, 0.2, 0.12)))
    return x * np.minimum(1, t / 0.003)


def bell():
    """A struck church-bell spectrum: hum, prime, tierce, quint, nominal and upper partials, each
    with its own decay, the strike's noise on top."""
    t = t_of(9.0)
    f = 196.0
    partials = ((0.5, 0.9, 7.5), (1.0, 1.0, 5.5), (1.183, 0.7, 4.0), (1.506, 0.45, 3.2), (2.0, 0.8, 2.6),
                (2.514, 0.35, 1.8), (2.662, 0.3, 1.5), (3.011, 0.25, 1.2), (4.166, 0.15, 0.7))
    x = np.zeros(len(t))
    for ratio, a, d in partials:
        beat = 1 + 0.004 * np.sin(2 * np.pi * 1.3 * ratio * t)
        x += a * np.sin(2 * np.pi * f * ratio * t * beat) * np.exp(-t / d)
    strike = band(rng.standard_normal(len(t)), 800, 9000) * np.exp(-t / 0.02)
    return (x + strike * 0.5) * np.minimum(1, t / 0.002)


def reached():
    t = t_of(5.0)
    swell = np.sin(np.pi * t / 5.0) ** 2
    chord = sum(np.sin(2 * np.pi * f * t) * a for f, a in ((110, 1), (165, 0.7), (220, 0.5), (277.2, 0.35)))
    water = band(rng.standard_normal(len(t)), 300, 5000)
    return chord * swell * 0.6 + water / np.std(water) * swell * 0.25


if __name__ == '__main__':
    write('wind.wav', wind(), 0.6)
    write('turn.wav', turn())
    write('open.wav', open_gate())
    write('handle.wav', handle(), 0.6)
    write('bell.wav', bell(), 0.9)
    write('reached.wav', reached(), 0.7)
