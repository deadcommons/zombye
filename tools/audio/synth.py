"""Placeholder sound effects and ambience for Zombye, synthesised from noise, tones and filters.

Plain Python 3, standard library only, and deterministic: every sound is seeded from its own name,
so running it again writes the same files. These are placeholders, ours outright, until the real
sounds of #78 replace them file for file (the client only knows the names in
`assets/data/sounds.ron`).

    python3 tools/audio/synth.py                 # write assets/audio/{sfx,ambience}/*.wav
    python3 tools/audio/synth.py --out DIR       # somewhere else
    python3 tools/audio/synth.py --attribution   # print the ATTRIBUTION.md rows

Files are mono, 16-bit, 22,050 Hz WAV, normalised to the same peak; loudness between sounds is set
in `sounds.ron`, not here. Loops (`ambience/*_loop.wav`) cross-fade their tail into their head so
they repeat without a click.
"""

import math
import os
import random
import struct
import sys
import wave
import zlib

RATE = 22050
PEAK = 0.89


# --- building blocks -------------------------------------------------------------------------

def samples(sec):
    return int(sec * RATE)


def silence(sec):
    return [0.0] * samples(sec)


def white(sec, rng):
    return [rng.uniform(-1.0, 1.0) for _ in range(samples(sec))]


def brown(sec, rng, leak=0.995):
    out, y = [], 0.0
    for _ in range(samples(sec)):
        y = y * leak + rng.uniform(-1.0, 1.0) * 0.1
        out.append(y)
    return out


class Biquad:
    """RBJ cookbook filter: 'lp', 'hp' or 'bp' (band-pass with 0 dB peak)."""

    def __init__(self, kind, f, q=0.707):
        self.kind = kind
        self.x1 = self.x2 = self.y1 = self.y2 = 0.0
        self.set(f, q)

    def set(self, f, q):
        f = min(max(f, 10.0), RATE * 0.45)
        w = 2.0 * math.pi * f / RATE
        c, s = math.cos(w), math.sin(w)
        a = s / (2.0 * q)
        if self.kind == "lp":
            b0, b1, b2 = (1 - c) / 2, 1 - c, (1 - c) / 2
        elif self.kind == "hp":
            b0, b1, b2 = (1 + c) / 2, -(1 + c), (1 + c) / 2
        else:
            b0, b1, b2 = a, 0.0, -a
        a0 = 1 + a
        self.b0, self.b1, self.b2 = b0 / a0, b1 / a0, b2 / a0
        self.a1, self.a2 = -2 * c / a0, (1 - a) / a0

    def run(self, x):
        y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2
        self.x2, self.x1, self.y2, self.y1 = self.x1, x, self.y1, y
        return y


def filt(sig, kind, f, q=0.707):
    b = Biquad(kind, f, q)
    return [b.run(x) for x in sig]


def sweep(sig, kind, f_of_t, q=0.707):
    """Filter with a centre that moves: `f_of_t(seconds)`, updated every 32 samples."""
    b = Biquad(kind, f_of_t(0.0), q)
    out = []
    for i, x in enumerate(sig):
        if i % 32 == 0:
            b.set(f_of_t(i / RATE), q)
        out.append(b.run(x))
    return out


def env(sec, attack, decay, hold=0.0):
    """Linear attack, a hold, then an exponential decay with time constant `decay`."""
    out = []
    for i in range(samples(sec)):
        t = i / RATE
        if t < attack:
            out.append(t / attack if attack > 0 else 1.0)
        elif t < attack + hold:
            out.append(1.0)
        else:
            out.append(math.exp(-(t - attack - hold) / decay))
    return out


def bell(sec, peak_at=0.5):
    """0 -> 1 -> 0, peaking `peak_at` of the way through."""
    n = samples(sec)
    out = []
    for i in range(n):
        u = i / max(n - 1, 1)
        v = u / peak_at if u < peak_at else (1 - u) / (1 - peak_at)
        out.append(math.sin(v * math.pi / 2) ** 2)
    return out


def mul(a, b):
    return [x * y for x, y in zip(a, b)]


def gain(sig, g):
    return [x * g for x in sig]


def osc(sec, f_of_t, shape="sine", phase=0.0):
    out, ph = [], phase
    for i in range(samples(sec)):
        f = f_of_t(i / RATE)
        if shape == "sine":
            out.append(math.sin(2 * math.pi * ph))
        elif shape == "saw":
            out.append(2.0 * ph - 1.0)
        else:
            out.append(1.0 - 4.0 * abs(ph - 0.5))
        ph = (ph + f / RATE) % 1.0
    return out


def thud(sec, f0, f1, decay, glide=None):
    """A low knock: a sine gliding from f0 down to f1, decaying."""
    glide = glide or decay
    tone = osc(sec, lambda t: f1 + (f0 - f1) * math.exp(-t / glide))
    return mul(tone, env(sec, 0.002, decay))


def burst(sec, rng, kind, f, q, attack, decay):
    return mul(filt(white(sec, rng), kind, f, q), env(sec, attack, decay))


def ring(sec, f, decay, partials=((1.0, 1.0),)):
    """A struck resonance: decaying sines at f times each ratio."""
    out = [0.0] * samples(sec)
    for ratio, g in partials:
        part = mul(osc(sec, lambda t, ff=f * ratio: ff), env(sec, 0.001, decay / ratio ** 0.5))
        out = [a + b * g for a, b in zip(out, part)]
    return out


def crackle(sec, rng, density, f, q, decay_after=None):
    """Sparse clicks (impulses) through a band-pass: grit, splinters, gravel."""
    n = samples(sec)
    imp = []
    for i in range(n):
        t = i / RATE
        d = density * (math.exp(-t / decay_after) if decay_after else 1.0)
        imp.append(rng.uniform(-1.0, 1.0) if rng.random() < d else 0.0)
    return filt(imp, "bp", f, q)


def place(dst, src, at, g=1.0):
    start = samples(at)
    need = start + len(src)
    if need > len(dst):
        dst.extend([0.0] * (need - len(dst)))
    for i, x in enumerate(src):
        dst[start + i] += x * g
    return dst


def fade(sig, fade_in=0.002, fade_out=0.02):
    n, a, b = len(sig), samples(fade_in), samples(fade_out)
    out = list(sig)
    for i in range(min(a, n)):
        out[i] *= i / a
    for i in range(min(b, n)):
        out[n - 1 - i] *= i / b
    return out


def normalise(sig, peak=PEAK):
    top = max((abs(x) for x in sig), default=0.0)
    return [x * peak / top for x in sig] if top > 0 else sig


def seamless(sig, overlap):
    """Cross-fade the last `overlap` seconds into the first, for a loop with no seam."""
    k = samples(overlap)
    body, tail = sig[:-k], sig[-k:]
    for i in range(k):
        w = i / k
        body[i] = body[i] * w + tail[i] * (1 - w)
    return body


# --- the sounds ------------------------------------------------------------------------------

def footstep_hard(rng):
    s = silence(0.14)
    place(s, burst(0.05, rng, "bp", rng.uniform(1700, 2600), 1.2, 0.0005, 0.007), 0.0, 0.5)
    place(s, thud(0.1, rng.uniform(120, 150), 70, 0.025), 0.004, 0.8)
    place(s, burst(0.1, rng, "lp", 450, 0.7, 0.001, 0.03), 0.004, 0.5)
    return s


def footstep_soft(rng):
    s = silence(0.2)
    place(s, burst(0.18, rng, "lp", rng.uniform(800, 1200), 0.7, 0.01, 0.05), 0.0, 0.8)
    place(s, crackle(0.12, rng, 0.03, rng.uniform(2600, 3400), 1.5, 0.05), 0.0, 1.2)
    place(s, thud(0.1, 95, 60, 0.03), 0.008, 0.4)
    return s


def shuffle(rng):
    dur = rng.uniform(0.4, 0.55)
    drag = sweep(white(dur, rng), "bp", lambda t: 500 + 300 * math.sin(t * 9), 0.9)
    s = mul(drag, bell(dur, 0.3))
    place(s, crackle(dur, rng, 0.01, 2200, 1.2), 0.0, 0.8)
    return s


def swing(rng, heavy):
    dur = 0.42 if heavy else 0.28
    lo, span = (250, 1300) if heavy else (450, 2300)
    whoosh = sweep(white(dur, rng), "bp", lambda t: lo + span * math.sin(math.pi * t / dur) ** 2, 1.6)
    return mul(whoosh, bell(dur, 0.45))


def hit_blunt(rng):
    s = silence(0.32)
    place(s, thud(0.3, rng.uniform(90, 110), 45, 0.07), 0.0, 1.0)
    place(s, burst(0.1, rng, "lp", 900, 0.7, 0.0005, 0.025), 0.0, 0.7)
    place(s, burst(0.02, rng, "hp", 3000, 0.7, 0.0003, 0.003), 0.0, 0.3)
    place(s, burst(0.25, rng, "bp", rng.uniform(220, 300), 4.0, 0.002, 0.06), 0.0, 0.5)
    return s


def gurgle(sec, rng, rate):
    """Wet, uneven amplitude: low-passed noise used as a level."""
    lfo = filt(white(sec, rng), "lp", rate, 0.7)
    top = max(abs(x) for x in lfo) or 1.0
    return [0.4 + 0.6 * abs(x) / top for x in lfo]


def hit_blade(rng):
    s = silence(0.34)
    place(s, burst(0.06, rng, "bp", rng.uniform(2200, 3000), 1.0, 0.0005, 0.012), 0.0, 0.6)
    wet = mul(burst(0.3, rng, "lp", 1200, 0.9, 0.003, 0.09), gurgle(0.3, rng, 40))
    place(s, wet, 0.005, 0.8)
    place(s, thud(0.12, 150, 80, 0.03), 0.0, 0.5)
    return s


def punch(rng):
    s = silence(0.2)
    place(s, thud(0.15, rng.uniform(150, 180), 90, 0.03), 0.0, 1.0)
    place(s, burst(0.08, rng, "lp", 1500, 0.7, 0.0005, 0.015), 0.0, 0.6)
    place(s, burst(0.02, rng, "hp", 4000, 0.7, 0.0003, 0.004), 0.0, 0.35)
    return s


def body_fall(rng):
    s = silence(0.6)
    place(s, thud(0.3, 70, 40, 0.09), 0.0, 1.0)
    place(s, thud(0.3, 60, 38, 0.08), rng.uniform(0.08, 0.12), 0.8)
    place(s, burst(0.4, rng, "lp", 500, 0.7, 0.003, 0.12), 0.0, 0.7)
    place(s, mul(filt(white(0.35, rng), "bp", 900, 0.8), bell(0.35, 0.2)), 0.05, 0.25)
    return s


def stomp(rng):
    s = silence(0.36)
    place(s, thud(0.3, 85, 40, 0.06), 0.0, 1.0)
    place(s, crackle(0.08, rng, 0.25, rng.uniform(1500, 2100), 1.0, 0.02), 0.002, 1.5)
    wet = mul(burst(0.25, rng, "lp", 900, 0.8, 0.004, 0.07), gurgle(0.25, rng, 30))
    place(s, wet, 0.01, 0.6)
    return s


def shove(rng):
    s = silence(0.32)
    cloth = lambda d: mul(filt(white(d, rng), "bp", rng.uniform(800, 1000), 0.7), env(d, 0.03, 0.06))
    place(s, cloth(0.2), 0.0, 1.0)
    place(s, cloth(0.15), rng.uniform(0.07, 0.1), 0.6)
    place(s, thud(0.12, 130, 90, 0.03), 0.02, 0.5)
    return s


def voice(sec, rng, f0, vowel_a, vowel_b, breath=0.2, rough=0.3):
    """A crude voiced sound: a jittery saw through three moving formants, plus breath."""
    wander = filt(white(sec + 0.1, rng), "lp", 3, 0.7)
    top = max(abs(x) for x in wander) or 1.0
    contour = lambda t: f0 * (0.85 + 0.25 * math.sin(math.pi * min(t / sec, 1.0))) * (1 + 0.12 * wander[min(samples(t), len(wander) - 1)] / top)
    src = osc(sec, contour, "saw")
    # Vocal roughness: a subharmonic flutter and a slow growl.
    src = [x * (1 - rough + rough * abs(math.sin(math.pi * i * f0 / 2 / RATE)))
           * (1 - 0.25 * rough + 0.25 * rough * math.sin(2 * math.pi * 28 * i / RATE)) for i, x in enumerate(src)]
    src = filt(src, "lp", 3200, 0.7)
    out = [0.0] * len(src)
    for k, g in ((0, 1.0), (1, 0.55), (2, 0.25)):
        fa, fb = vowel_a[k], vowel_b[k]
        part = sweep(src, "bp", lambda t, fa=fa, fb=fb: fa + (fb - fa) * min(t / sec, 1.0), 5.0 + 2 * k)
        out = [a + b * g for a, b in zip(out, part)]
    air = filt(white(sec, rng), "bp", 1400, 0.8)
    return [a + b * breath for a, b in zip(out, air)]


VOWELS = {"uh": (600, 1000, 2400), "oh": (480, 820, 2300), "ah": (740, 1150, 2500), "oo": (380, 800, 2200)}


def groan(rng):
    dur = rng.uniform(1.0, 1.8)
    a, b = rng.sample(sorted(VOWELS), 2)
    v = voice(dur, rng, rng.uniform(70, 105), VOWELS[a], VOWELS[b], breath=rng.uniform(0.25, 0.45), rough=rng.uniform(0.3, 0.5))
    shape = [min(1.0, x * 1.6) for x in bell(dur, rng.uniform(0.3, 0.55))]
    return mul(v, shape)


def player_hurt(rng):
    s = silence(0.5)
    for k in range(3):
        stroke = mul(filt(white(0.05, rng), "bp", rng.uniform(3000, 4200), 2.0), bell(0.05, 0.3))
        place(s, stroke, k * 0.035, 0.5)
    grunt = voice(0.3, rng, rng.uniform(130, 160), VOWELS["uh"], VOWELS["ah"], breath=0.35, rough=0.2)
    place(s, mul(grunt, env(0.3, 0.02, 0.12)), 0.03, 1.0)
    return s


def latch(rng):
    s = burst(0.01, rng, "hp", 3000, 0.7, 0.0002, 0.002)
    return place(s, ring(0.05, rng.uniform(1900, 2300), 0.015, ((1, 1), (2.7, 0.4))), 0.0, 0.5)


def creak(sec, rng, rate_lo, rate_hi):
    """Stick-slip: a train of clicks whose rate wobbles, through two wooden resonances."""
    n = samples(sec)
    imp, ph = [], 0.0
    wob = rng.uniform(2, 4)
    for i in range(n):
        t = i / RATE
        rate = rate_lo + (rate_hi - rate_lo) * (0.5 + 0.5 * math.sin(2 * math.pi * wob * t)) * (1 + rng.uniform(-0.15, 0.15))
        ph += rate / RATE
        if ph >= 1.0:
            ph -= 1.0
            imp.append(1.0)
        else:
            imp.append(0.0)
    a = filt(imp, "bp", rng.uniform(800, 1000), 3.0)
    b = filt(imp, "bp", rng.uniform(1700, 2000), 4.0)
    return mul([x + 0.6 * y for x, y in zip(a, b)], bell(sec, 0.35))


def door_open(rng):
    s = silence(0.9)
    place(s, latch(rng), 0.0, 0.8)
    place(s, creak(0.7, rng, 160, 320), 0.08, 1.0)
    return s


def door_close(rng):
    s = silence(0.7)
    place(s, creak(0.28, rng, 260, 420), 0.0, 0.6)
    place(s, thud(0.3, 95, 50, 0.08), 0.3, 1.0)
    place(s, burst(0.2, rng, "lp", 700, 0.7, 0.001, 0.05), 0.3, 0.6)
    place(s, latch(rng), 0.32, 0.6)
    return s


def slide(sec, rng):
    rub = filt(white(sec, rng), "bp", rng.uniform(1200, 1600), 2.0)
    flutter = gurgle(sec, rng, 25)
    return mul(mul(rub, flutter), bell(sec, 0.25))


def window_open(rng):
    s = silence(0.62)
    place(s, slide(0.5, rng), 0.0, 1.0)
    place(s, ring(0.08, rng.uniform(2300, 2700), 0.02, ((1, 1), (1.6, 0.5))), 0.5, 0.5)
    return s


def window_close(rng):
    s = silence(0.55)
    place(s, slide(0.32, rng), 0.0, 0.9)
    place(s, ring(0.15, 2800, 0.04, ((1, 1), (1.32, 0.7), (1.54, 0.5))), 0.32, 0.5)
    place(s, thud(0.12, 140, 90, 0.025), 0.32, 0.6)
    return s


def glass_break(rng):
    s = silence(1.4)
    place(s, burst(0.2, rng, "hp", 1500, 0.7, 0.0005, 0.06), 0.0, 1.0)
    place(s, thud(0.15, 160, 90, 0.03), 0.0, 0.5)
    for _ in range(45):
        t = rng.expovariate(1 / 0.25)
        if t > 1.1:
            continue
        f = rng.uniform(2500, 7000)
        place(s, ring(0.12, f, rng.uniform(0.015, 0.07), ((1, 1), (1.47, 0.4))), t, rng.uniform(0.08, 0.35) * math.exp(-t * 1.5))
        place(s, burst(0.01, rng, "hp", 4000, 0.7, 0.0002, 0.002), t, 0.15)
    return s


def door_break(rng):
    s = silence(1.2)
    place(s, burst(0.1, rng, "hp", 1200, 0.7, 0.0003, 0.015), 0.0, 1.0)
    place(s, crackle(0.3, rng, 0.08, rng.uniform(1300, 2500), 1.0, 0.1), 0.0, 1.6)
    place(s, thud(0.5, 75, 40, 0.15), 0.02, 1.0)
    place(s, burst(0.5, rng, "bp", 300, 5.0, 0.002, 0.2), 0.01, 0.7)
    for t in (0.3, 0.45, 0.62):
        place(s, thud(0.15, rng.uniform(90, 130), 60, 0.03), t + rng.uniform(-0.03, 0.03), 0.35)
    return s


def thump_door(rng):
    s = silence(0.55)
    place(s, thud(0.5, rng.uniform(70, 85), 45, 0.11), 0.0, 1.0)
    place(s, burst(0.3, rng, "lp", 500, 0.7, 0.001, 0.06), 0.0, 0.6)
    place(s, burst(0.5, rng, "bp", rng.uniform(200, 250), 6.0, 0.002, 0.18), 0.0, 0.5)
    place(s, crackle(0.1, rng, 0.05, 1500, 2.0), 0.03, 0.4)
    return s


def thump_window(rng):
    s = silence(0.5)
    place(s, thud(0.2, rng.uniform(110, 130), 70, 0.04), 0.0, 0.8)
    rattle = ring(0.4, rng.uniform(2000, 2400), 0.12, ((1, 1), (1.41, 0.7), (1.95, 0.5)))
    trem = [0.6 + 0.4 * math.sin(2 * math.pi * 35 * i / RATE) for i in range(len(rattle))]
    place(s, mul(rattle, trem), 0.004, 0.35)
    place(s, burst(0.1, rng, "bp", 900, 1.0, 0.001, 0.03), 0.0, 0.5)
    return s


def rummage(rng):
    dur = 0.55
    s = silence(dur)
    for _ in range(rng.randint(4, 7)):
        t = rng.uniform(0.0, dur - 0.12)
        kind = rng.random()
        if kind < 0.5:
            place(s, mul(filt(white(0.1, rng), "bp", rng.uniform(900, 1500), 0.8), env(0.1, 0.01, 0.03)), t, 0.6)
        elif kind < 0.8:
            place(s, ring(0.08, rng.uniform(1800, 4000), 0.025, ((1, 1), (2.3, 0.4))), t, 0.25)
        else:
            place(s, crackle(0.08, rng, 0.05, 3500, 1.5), t, 0.6)
    return s


def pickup(rng):
    s = silence(0.28)
    place(s, mul(filt(white(0.18, rng), "bp", 1100, 0.8), bell(0.18, 0.3)), 0.0, 0.8)
    place(s, thud(0.1, 200, 130, 0.02), 0.15, 0.4)
    return s


def level_up(rng):
    s = silence(0.8)
    for at, f in ((0.0, 659.3), (0.12, 880.0)):
        place(s, ring(0.6, f, 0.25, ((1, 1), (2, 0.3), (3, 0.1))), at, 0.6)
    return s


def bird(rng):
    s = silence(1.0)
    t = 0.0
    for _ in range(rng.randint(2, 5)):
        d = rng.uniform(0.05, 0.12)
        a, b = rng.uniform(2500, 4500), rng.uniform(2500, 4500)
        tweet = osc(d, lambda u, a=a, b=b, d=d: a + (b - a) * (u / d) + 120 * math.sin(2 * math.pi * 30 * u))
        place(s, mul(tweet, bell(d, 0.3)), t, 0.6)
        t += d + rng.uniform(0.03, 0.12)
    return s


LOOP = 8.0  # seconds; every LFO below fits a whole number of cycles in it


def wind_loop(rng):
    dur = LOOP + 1.0
    base = brown(dur, rng)
    gust = lambda t: 0.6 + 0.4 * math.sin(2 * math.pi * t / LOOP) * math.sin(2 * math.pi * 3 * t / LOOP)
    body = sweep(base, "lp", lambda t: 500 + 350 * math.sin(2 * math.pi * 2 * t / LOOP), 0.9)
    body = [x * gust(i / RATE) for i, x in enumerate(body)]
    whistle = sweep(white(dur, rng), "bp", lambda t: 650 + 120 * math.sin(2 * math.pi * 3 * t / LOOP), 9.0)
    whistle = [x * 0.15 * max(0.0, math.sin(2 * math.pi * 2 * i / RATE / LOOP)) for i, x in enumerate(whistle)]
    return seamless([a + b for a, b in zip(body, whistle)], 1.0)


def room_loop(rng):
    dur = 6.0 + 0.5
    hum = [0.0] * samples(dur)
    for f, g in ((55, 1.0), (110, 0.5), (165, 0.2)):
        hum = [a + b * g for a, b in zip(hum, osc(dur, lambda t, f=f: f))]
    air = filt(brown(dur, rng), "lp", 400, 0.7)
    top = max(abs(x) for x in air) or 1.0
    return seamless([0.25 * h + 0.6 * a / top for h, a in zip(hum, air)], 0.5)


def crickets_loop(rng):
    dur = 6.0 + 0.5
    s = [0.0] * samples(dur)
    for k in range(3):
        f = rng.uniform(4200, 5200)
        every = 6.0 / rng.choice((6, 7, 8, 9))
        at = rng.uniform(0, every)
        g = rng.uniform(0.4, 1.0)
        while at < dur - 0.1:
            for syl in range(rng.choice((3, 4))):
                pulse = mul(osc(0.014, lambda t, f=f: f), bell(0.014, 0.3))
                place(s, pulse, at + syl * 0.026, g)
            at += every
    return seamless(s, 0.5)


# name -> (folder, maker, what it is)
SOUNDS = {}


def add(folder, name, maker, what, variants=1):
    for v in range(1, variants + 1):
        key = f"{name}_{v}" if variants > 1 else name
        SOUNDS[key] = (folder, maker, what)


add("sfx", "footstep_hard", footstep_hard, "footstep on a hard floor or road", 4)
add("sfx", "footstep_soft", footstep_soft, "footstep on grass or dirt", 4)
add("sfx", "shuffle", shuffle, "a zombie's dragging step", 3)
add("sfx", "swing_light", lambda r: swing(r, False), "whoosh of a light swing or punch", 2)
add("sfx", "swing_heavy", lambda r: swing(r, True), "whoosh of a heavy swing", 2)
add("sfx", "hit_blunt", hit_blunt, "a blunt weapon hitting a body", 3)
add("sfx", "hit_blade", hit_blade, "a blade hitting a body", 3)
add("sfx", "punch", punch, "a punch landing", 2)
add("sfx", "body_fall", body_fall, "a body falling to the ground", 2)
add("sfx", "stomp", stomp, "a stomp on a downed zombie", 2)
add("sfx", "shove", shove, "a shove", 2)
add("sfx", "player_hurt", player_hurt, "a survivor scratched or bitten, with a grunt", 3)
add("sfx", "groan", groan, "a zombie groan", 6)
add("sfx", "door_open", door_open, "a door opening", 2)
add("sfx", "door_close", door_close, "a door shutting", 2)
add("sfx", "window_open", window_open, "a window sliding open", 1)
add("sfx", "window_close", window_close, "a window sliding shut", 1)
add("sfx", "glass_break", glass_break, "a window smashing", 2)
add("sfx", "door_break", door_break, "a door breaking", 2)
add("sfx", "thump_door", thump_door, "a zombie battering a door", 3)
add("sfx", "thump_window", thump_window, "a zombie battering a window", 2)
add("sfx", "rummage", rummage, "searching a container", 3)
add("sfx", "pickup", pickup, "picking something up", 1)
add("sfx", "level_up", level_up, "a skill levelling up", 1)
add("sfx", "bird", bird, "a bird call by day", 3)
add("ambience", "wind_loop", wind_loop, "wind outdoors (loop)")
add("ambience", "room_loop", room_loop, "room tone indoors (loop)")
add("ambience", "crickets_loop", crickets_loop, "crickets at night (loop)")


def write_wav(path, sig):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1.0, min(1.0, x)) * 32767)) for x in sig))


def render(name):
    folder, maker, _ = SOUNDS[name]
    rng = random.Random(zlib.crc32(name.encode()))
    sig = maker(rng)
    if not name.endswith("_loop"):
        sig = fade(sig)
    return folder, normalise(sig)


def main(argv):
    if "--attribution" in argv:
        for name, (folder, _, what) in SOUNDS.items():
            print(f"| `assets/audio/{folder}/{name}.wav` ({what}) | Dead Commons | Synthesised by `tools/audio/synth.py` "
                  f"(ours: noise, tones and filters, no samples or generator) | CC BY-NC-SA 4.0 | Placeholder until the real sounds (#78) |")
        return
    out = argv[argv.index("--out") + 1] if "--out" in argv else os.path.join(os.path.dirname(__file__), "..", "..", "assets", "audio")
    total = 0
    for name in SOUNDS:
        folder, sig = render(name)
        path = os.path.join(out, folder, name + ".wav")
        write_wav(path, sig)
        total += len(sig)
    print(f"wrote {len(SOUNDS)} sounds ({total / RATE:.1f} s) to {os.path.normpath(out)}")


if __name__ == "__main__":
    main(sys.argv[1:])
