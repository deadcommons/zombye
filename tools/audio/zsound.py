"""Real sounds for Zombye (#78): from a generator's raw takes to game-ready WAVs, and into the game.
Plain Python 3, standard library only (ffmpeg, when it's on the PATH, decodes MP3 and anything else).

One work folder per sound set, in the private art repo (`$ZOMBYE_ART`, default ~/games/zombye-art),
`audio/<cue>/` (e.g. audio/footstep-hard/), or `target/audio-wip/<cue>/` here without it:

    <cue>.ron      the spec: the cue, the prompts, which slice of which take becomes which file, clean-up
    meta.yml       status (wip -> review -> ready -> promoted), tool and plan, author, the promoting commit
    raw/           generator output exactly as downloaded, never edited
    export/        the game-ready files `clean` writes
    renders/       audition.wav: every export one after another, for the owner to listen to

    python3 tools/audio/zsound.py start footstep-hard FootstepHard   # make the folder and a spec to fill in
    python3 tools/audio/zsound.py split audio/footstep-hard/raw/take_1.mp3   # find the separate sounds in a take
    python3 tools/audio/zsound.py clean footstep-hard              # spec + raw/ -> export/*.wav, a report
    python3 tools/audio/zsound.py audition footstep-hard           # export/ -> renders/audition.wav
    python3 tools/audio/zsound.py promote footstep-hard            # export/ -> assets/audio/, ATTRIBUTION.md rows
                                                                   # (ElevenLabs output: -> the art repo's release-audio/)
    python3 tools/audio/zsound.py record <commit> footstep-hard    # after committing it in this repo
    python3 tools/audio/zsound.py check                            # every cue's files: there, 16-bit, attributed

The spec (RON; fields left out take the defaults in DEFAULT_CLEAN):

    (
        cue: "FootstepHard",                 // the Cue in assets/data/sounds.ron
        folder: "sfx",                       // assets/audio/<folder>/
        what: "footstep on a hard floor or road",
        generator: "ElevenLabs Sound Effects (Creator)",   // tool and paid plan, for ATTRIBUTION.md
        takes: [                             // every generation made, kept or not (the record of spend)
            (file: "raw/take_1.mp3", prompt: "Single boot footstep on concrete, ...", duration: Some(4.0), influence: 0.5),
        ],
        files: [                             // what ships: a slice of a take each
            (name: "footstep_hard_1", take: "raw/take_1.mp3", start: 0.12, end: 0.55),
            (name: "footstep_hard_2", take: "raw/take_1.mp3", start: 1.10, end: 1.52, gain_db: -1.5),
        ],
        clean: (highpass_hz: 60.0, peak_db: -1.0),
        release_only: true,                  // default: true for ElevenLabs output (see Release audio)
    )

Release audio (#154): ElevenLabs' terms forbid publishing its output "as isolated audio files", so a
spec whose generator is ElevenLabs (or that says `release_only: true`) is promoted into the art repo's
`release-audio/` (audio/<folder>/*.wav, its own ATTRIBUTION.md, data/sounds.ron for cues that gain
files), which release builds carry beside the game's binary (the client's `release_audio.rs`), never
into this public repo. `release_only: false` overrides it only once the generator's owner has said in
writing that public files are fine. Sounds we record ourselves go into assets/audio/ as before.

Clean-up, in order: decode (mono unless `stereo: true`), resample to `rate`, the slice, a high-pass,
trim silence at both ends below `trim_db` of the peak (keeping `pad_ms`), cut to `max_secs`, `gain_db`,
normalise the peak to `peak_db` (as synth.py does: loudness between sounds is set in sounds.ron, not in
the files), fade in and out, or for a loop (`loop: true`) cross-fade the tail into the head; written as
16-bit PCM WAV with a little dither.
"""

import datetime
import math
import os
import random
import shutil
import struct
import subprocess
import sys
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "blender"))
from ronlite import read_ron  # noqa: E402

GAME = Path(__file__).resolve().parents[2]
AUDIO = GAME / "assets" / "audio"
SOUNDS = GAME / "assets" / "data" / "sounds.ron"
ATTRIBUTION = GAME / "ATTRIBUTION.md"
STATUSES = ("wip", "review", "ready", "promoted")

DEFAULT_CLEAN = {
    "rate": 44100,
    "stereo": False,
    "highpass_hz": 40.0,
    "trim_db": -45.0,
    "pad_ms": 4.0,
    "max_secs": None,
    "peak_db": -1.0,
    "normalise": True,
    "fade_in_ms": 2.0,
    "fade_out_ms": 25.0,
    "loop": False,
    "loop_xfade_ms": 400.0,
}
FILE_FIELDS = {"name", "take", "start", "end", "gain_db", "clean"}


# ---------------------------------------------------------------- folders and specs

def art_root():
    p = Path(os.environ.get("ZOMBYE_ART") or Path.home() / "games" / "zombye-art").expanduser()
    return p if (p / "PROMOTED.md").exists() else None


def work_dir(cue_dir):
    """`footstep-hard` (or `audio/footstep-hard`, or a path) -> its work folder."""
    p = Path(cue_dir)
    if p.is_dir() and (p / f"{p.name}.ron").exists():
        return p.resolve()
    name = p.name
    root = art_root()
    return (root / "audio" / name) if root else (GAME / "target" / "audio-wip" / name)


def load_spec(d):
    path = d / f"{d.name}.ron"
    spec = read_ron(path.read_text())
    for field in ("cue", "folder", "what", "generator", "files"):
        if field not in spec:
            raise SystemExit(f"{path}: spec has no `{field}`")
    names = set()
    for f in spec["files"]:
        unknown = set(f) - FILE_FIELDS
        if unknown:
            raise SystemExit(f"{path}: file {f.get('name')}: unknown fields {sorted(unknown)}")
        if f["name"] in names:
            raise SystemExit(f"{path}: {f['name']} listed twice")
        names.add(f["name"])
    clean = dict(DEFAULT_CLEAN)
    unknown = set(spec.get("clean", {})) - set(DEFAULT_CLEAN)
    if unknown:
        raise SystemExit(f"{path}: clean: unknown fields {sorted(unknown)}")
    clean.update(spec.get("clean", {}))
    spec["clean"] = clean
    return spec


def read_meta(d):
    meta = {}
    p = d / "meta.yml"
    if p.exists():
        for line in p.read_text().splitlines():
            if ":" in line and not line.startswith("#"):
                k, v = line.split(":", 1)
                meta[k.strip()] = v.strip()
    return meta


def write_meta(d, meta):
    (d / "meta.yml").write_text("".join(f"{k}: {v}\n" for k, v in meta.items()))


# ---------------------------------------------------------------- decoding and writing

def decode(path, rate, stereo):
    """A file -> (channels: list of float lists, rate). ffmpeg if there is one (any format, resampled to
    `rate`), else WAV (8/16/24/32-bit PCM) or raw `.pcm` (16-bit LE mono, rate from a `_44100` in the name)."""
    path = Path(path)
    ch = 2 if stereo else 1
    if shutil.which("ffmpeg"):
        out = subprocess.run(
            ["ffmpeg", "-v", "error", "-i", str(path), "-f", "s16le", "-ac", str(ch), "-ar", str(rate), "-"],
            check=True, capture_output=True).stdout
        n = len(out) // 2
        vals = struct.unpack(f"<{n}h", out[: n * 2])
        return [[v / 32768.0 for v in vals[c::ch]] for c in range(ch)], rate
    if path.suffix.lower() == ".pcm":
        digits = [p for p in path.stem.split("_") if p.isdigit()]
        src_rate = int(digits[-1]) if digits else 44100
        data = path.read_bytes()
        n = len(data) // 2
        chans = [[v / 32768.0 for v in struct.unpack(f"<{n}h", data[: n * 2])]]
    elif path.suffix.lower() == ".wav":
        with wave.open(str(path), "rb") as w:
            src_rate, nch, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
            data = w.readframes(w.getnframes())
        if width == 1:
            vals = [(b - 128) / 128.0 for b in data]
        elif width == 2:
            vals = [v / 32768.0 for v in struct.unpack(f"<{len(data) // 2}h", data)]
        elif width == 3:
            vals = [int.from_bytes(data[i:i + 3], "little", signed=True) / 8388608.0 for i in range(0, len(data), 3)]
        else:
            vals = [v / 2147483648.0 for v in struct.unpack(f"<{len(data) // 4}i", data)]
        chans = [vals[c::nch] for c in range(nch)]
    else:
        raise SystemExit(f"{path}: needs ffmpeg to decode (or ask the generator for WAV or PCM)")
    if ch == 1 and len(chans) > 1:
        chans = [[sum(s) / len(s) for s in zip(*chans)]]
    elif ch == 2 and len(chans) == 1:
        chans = [chans[0], list(chans[0])]
    if src_rate != rate:
        chans = [resample(c, src_rate, rate) for c in chans]
    return chans, rate


def resample(sig, src, dst):
    """Linear interpolation; fine for the odd WAV when ffmpeg isn't there."""
    n = int(len(sig) * dst / src)
    out = []
    for i in range(n):
        x = i * src / dst
        j = int(x)
        t = x - j
        a = sig[j] if j < len(sig) else 0.0
        b = sig[j + 1] if j + 1 < len(sig) else a
        out.append(a + (b - a) * t)
    return out


def write_wav(path, chans, rate, seed=0):
    path.parent.mkdir(parents=True, exist_ok=True)
    rng = random.Random(seed)
    frames = []
    for s in zip(*chans):
        for x in s:
            d = (rng.random() - rng.random()) / 32768.0  # TPDF dither, one LSB
            frames.append(int(round(max(-1.0, min(1.0, x + d)) * 32767)))
    with wave.open(str(path), "wb") as w:
        w.setnchannels(len(chans))
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(struct.pack(f"<{len(frames)}h", *frames))


# ---------------------------------------------------------------- processing

def db(x):
    return 20.0 * math.log10(x) if x > 1e-9 else -180.0


def peak(chans):
    return max((abs(x) for c in chans for x in c), default=0.0)


def rms(chans):
    n = sum(len(c) for c in chans)
    return math.sqrt(sum(x * x for c in chans for x in c) / n) if n else 0.0


def highpass(sig, f, rate):
    """Second-order Butterworth high-pass (RBJ), to take out rumble and DC."""
    if not f:
        return sig
    w = 2.0 * math.pi * f / rate
    c, s = math.cos(w), math.sin(w)
    a = s / (2.0 * 0.7071)
    a0 = 1 + a
    b0, b1, b2 = (1 + c) / 2 / a0, -(1 + c) / a0, (1 + c) / 2 / a0
    a1, a2 = -2 * c / a0, (1 - a) / a0
    x1 = x2 = y1 = y2 = 0.0
    out = []
    for x in sig:
        y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, x, y1, y
        out.append(y)
    return out


def envelope(chans, rate, win_ms=5.0):
    """Peak per window of `win_ms`, over all channels."""
    w = max(1, int(rate * win_ms / 1000))
    n = len(chans[0])
    return [max(abs(c[j]) for c in chans for j in range(i, min(n, i + w))) for i in range(0, n, w)], w


def trim(chans, rate, trim_db, pad_ms):
    env, w = envelope(chans, rate)
    top = max(env, default=0.0)
    if top <= 0:
        return chans, 0.0, 0.0
    floor = top * 10 ** (trim_db / 20)
    loud = [i for i, e in enumerate(env) if e >= floor]
    pad = int(rate * pad_ms / 1000)
    a = max(0, loud[0] * w - pad)
    b = min(len(chans[0]), (loud[-1] + 1) * w + pad)
    return [c[a:b] for c in chans], a / rate, (len(chans[0]) - b) / rate


def fade(chans, rate, fade_in_ms, fade_out_ms):
    out = []
    for c in chans:
        c = list(c)
        n = len(c)
        fi = min(n, int(rate * fade_in_ms / 1000))
        fo = min(n, int(rate * fade_out_ms / 1000))
        for i in range(fi):
            c[i] *= i / fi
        for i in range(fo):
            c[n - 1 - i] *= math.sin(0.5 * math.pi * i / fo)
        out.append(c)
    return out


def seamless(chans, rate, xfade_ms):
    """Cross-fade the last `xfade_ms` into the first (equal power), so the loop repeats without a seam."""
    k = int(rate * xfade_ms / 1000)
    out = []
    for c in chans:
        k2 = min(k, len(c) // 3)
        head, body, tail = c[:k2], c[k2:len(c) - k2], c[len(c) - k2:]
        mixed = [tail[i] * math.cos(0.5 * math.pi * i / k2) + head[i] * math.sin(0.5 * math.pi * i / k2)
                 for i in range(k2)]
        out.append(mixed + body)
    return out


def process(take_path, f, clean):
    rate = clean["rate"]
    chans, rate = decode(take_path, rate, clean["stereo"])
    a = int((f.get("start") or 0.0) * rate)
    b = int(f["end"] * rate) if f.get("end") is not None else len(chans[0])
    chans = [highpass(c[a:b], clean["highpass_hz"], rate) for c in chans]
    trimmed = (0.0, 0.0)
    if not clean["loop"]:
        chans, ta, tb = trim(chans, rate, clean["trim_db"], clean["pad_ms"])
        trimmed = (ta, tb)
    if clean["max_secs"]:
        chans = [c[: int(clean["max_secs"] * rate)] for c in chans]
    g = 10 ** ((f.get("gain_db") or 0.0) / 20)
    chans = [[x * g for x in c] for c in chans]
    if clean["normalise"]:
        p = peak(chans)
        if p > 0:
            g = 10 ** (clean["peak_db"] / 20) / p
            chans = [[x * g for x in c] for c in chans]
    if clean["loop"]:
        chans = seamless(chans, rate, clean["loop_xfade_ms"])
    else:
        chans = fade(chans, rate, clean["fade_in_ms"], clean["fade_out_ms"])
    return chans, rate, trimmed


# ---------------------------------------------------------------- commands

def cmd_start(name, cue):
    d = work_dir(name)
    if (d / f"{d.name}.ron").exists():
        raise SystemExit(f"{d} already has a spec")
    for sub in ("raw", "export", "renders"):
        (d / sub).mkdir(parents=True, exist_ok=True)
    (d / f"{d.name}.ron").write_text(f"""(
    cue: "{cue}",
    folder: "sfx",
    what: "",
    generator: "ElevenLabs Sound Effects (PLAN)",
    takes: [
        // (file: "raw/take_1.mp3", prompt: "", duration: Some(3.0), influence: 0.5),
    ],
    files: [
        // (name: "{d.name.replace('-', '_')}_1", take: "raw/take_1.mp3", start: 0.0, end: 1.0),
    ],
    clean: (),
)
""")
    write_meta(d, {"name": cue, "status": "wip", "tool": "ElevenLabs Sound Effects", "plan_owns_output": "",
                   "author": "Dead Commons", "notes": "", "promoted": ""})
    print(f"started {d}")


def cmd_split(take, threshold_db=-30.0, min_gap_ms=120.0):
    """Print the separate sounds in a take (onsets by a peak envelope), as `files` lines to paste."""
    chans, rate = decode(take, 44100, False)
    env, w = envelope(chans, rate, 10.0)
    top = max(env, default=0.0)
    floor = top * 10 ** (threshold_db / 20)
    gap = int(min_gap_ms / 10)
    segs, start, quiet = [], None, 0
    for i, e in enumerate(env):
        if e >= floor:
            if start is None:
                start = i
            quiet = 0
        elif start is not None:
            quiet += 1
            if quiet >= gap:
                segs.append((start, i - quiet + 1))
                start, quiet = None, 0
    if start is not None:
        segs.append((start, len(env)))
    shown = f"raw/{Path(take).name}" if Path(take).parent.name == "raw" else take
    print(f"{take}: {len(env) * w / rate:.2f} s, peak {db(top):.1f} dBFS, {len(segs)} sounds over {threshold_db:.0f} dB:")
    for k, (a, b) in enumerate(segs, 1):
        s, e = max(0, a - 2) * w / rate, (b + 3) * w / rate
        loud = db(max(env[a:b]))
        print(f'    (name: "NAME_{k}", take: "{shown}", start: {s:.3f}, end: {e:.3f}),   // {e - s:.2f} s, peak {loud:.1f} dBFS')


def cmd_clean(name):
    d = work_dir(name)
    spec = load_spec(d)
    out = d / "export"
    out.mkdir(exist_ok=True)
    print(f"{spec['cue']}: {len(spec['files'])} files")
    for k, f in enumerate(spec["files"]):
        clean = dict(spec["clean"])
        clean.update(f.get("clean") or {})
        chans, rate, (ta, tb) = process(d / f["take"], f, clean)
        write_wav(out / f"{f['name']}.wav", chans, rate, seed=k)
        secs = len(chans[0]) / rate
        print(f"  {f['name']}.wav  {secs:.2f} s  peak {db(peak(chans)):.1f}  rms {db(rms(chans)):.1f} dBFS"
              f"  trimmed {ta * 1000:.0f}/{tb * 1000:.0f} ms" + ("  loop" if clean["loop"] else ""))


def cmd_audition(name, gap=0.6):
    d = work_dir(name)
    spec = load_spec(d)
    rate = spec["clean"]["rate"]
    ch = 2 if spec["clean"]["stereo"] else 1
    track = [[] for _ in range(ch)]
    for f in spec["files"]:
        chans, _ = decode(d / "export" / f"{f['name']}.wav", rate, ch == 2)
        reps = 2 if spec["clean"]["loop"] else 1
        for c in range(ch):
            track[c] += chans[c] * reps + [0.0] * int(gap * rate)
    write_wav(d / "renders" / "audition.wav", track, rate)
    print(f"wrote {d / 'renders' / 'audition.wav'} ({len(track[0]) / rate:.1f} s, in spec order)")


def attribution_row(spec, f):
    take = next((t for t in spec.get("takes", []) if t.get("file") == f["take"]), {})
    prompt = take.get("prompt", "").replace("|", "/")
    rel = f"assets/audio/{spec['folder']}/{f['name']}.wav"
    recorded = "record" in spec["generator"].lower()
    source = f"{spec['generator']}" + (f" ({prompt})" if recorded and prompt else f", prompt \"{prompt}\"" if prompt else "")
    notes = "recorded by us" if recorded else "AI-generated (paid plan: we own the output), edited by us"
    return (f"| `{rel}` ({spec['what']}) | Dead Commons | {source}; "
            f"trimmed, filtered and normalised by `tools/audio/zsound.py` | CC BY-NC-SA 4.0 | {notes} |")


def release_only(spec):
    """Whether this set may only ship in release builds, not as public files (#154)."""
    flag = spec.get("release_only")
    return "elevenlabs" in spec["generator"].lower() if flag is None else bool(flag)


RELEASE_HEADER = """# Release audio

Sounds and music that Zombye's release builds carry but that are not published in the public repo
and are not under its CC licence (their generator's terms forbid handing them out as separate
files). Copied beside the game's binary as `release-audio/` (see the client's `release_audio.rs`).

| File | Author | Source | Licence | Notes |
|---|---|---|---|---|
"""


def promote_release(d, spec, release):
    """Copy a release-only set into the art repo's release-audio/ with its own attribution rows."""
    rows_file = release / "ATTRIBUTION.md"
    (release / "audio" / spec["folder"]).mkdir(parents=True, exist_ok=True)
    if not rows_file.exists():
        rows_file.write_text(RELEASE_HEADER)
    lines = rows_file.read_text().rstrip("\n").split("\n")
    sounds_file = release / "data" / "sounds.ron"
    sounds = read_ron((sounds_file if sounds_file.exists() else SOUNDS).read_text())["sounds"]
    listed = set(sounds.get(spec["cue"], {}).get("files", []))
    for f in spec["files"]:
        rel = f"{spec['folder']}/{f['name']}.wav"
        shutil.copyfile(d / "export" / f"{f['name']}.wav", release / "audio" / rel)
        row = attribution_row(spec, f).replace(" | CC BY-NC-SA 4.0 | ", " | release builds only, not CC | ")
        hit = [i for i, l in enumerate(lines) if l.startswith(f"| `assets/audio/{rel}`")]
        if hit:
            lines[hit[0]] = row
        else:
            lines.append(row)
        if rel not in listed:
            where = sounds_file if sounds_file.exists() else f"{sounds_file} (copy assets/data/sounds.ron there first)"
            print(f"  note: {rel} isn't in {spec['cue']}'s files yet: add it in {where}")
        print(f"  release-audio/audio/{rel}")
    rows_file.write_text("\n".join(lines) + "\n")


def cmd_promote(name):
    d = work_dir(name)
    spec = load_spec(d)
    meta = read_meta(d)
    if meta.get("status") not in ("ready", "promoted"):
        raise SystemExit(f"{d.name} is `{meta.get('status')}`: the owner approves the audition first (status: ready)")
    if "PLAN" in spec["generator"]:
        raise SystemExit(f"{d.name}: say which paid plan made it in `generator`")
    if release_only(spec):
        root = art_root()
        if root is None:
            raise SystemExit(f"{d.name} is release-only (#154): it goes into the art repo's release-audio/, "
                             "and there's no art repo here ($ZOMBYE_ART)")
        promote_release(d, spec, root / "release-audio")
        meta["status"] = "promoted"
        write_meta(d, meta)
        print(f"promoted {spec['cue']} to the art repo's release-audio/ (not this repo): commit it there")
        return
    sounds = read_ron(SOUNDS.read_text())["sounds"]
    listed = set(sounds.get(spec["cue"], {}).get("files", []))
    lines = ATTRIBUTION.read_text().split("\n")
    for f in spec["files"]:
        rel = f"{spec['folder']}/{f['name']}.wav"
        shutil.copyfile(d / "export" / f"{f['name']}.wav", AUDIO / rel)
        row = attribution_row(spec, f)
        hit = [i for i, l in enumerate(lines) if l.startswith(f"| `assets/audio/{rel}`")]
        if hit:
            lines[hit[0]] = row
        else:
            last = max(i for i, l in enumerate(lines) if l.startswith("| `assets/audio/"))
            lines.insert(last + 1, row)
        if rel not in listed:
            print(f"  note: {rel} isn't in {spec['cue']}'s files in sounds.ron yet")
        print(f"  {rel}")
    ATTRIBUTION.write_text("\n".join(lines))
    meta["status"] = "promoted"
    write_meta(d, meta)
    print(f"promoted {spec['cue']}: commit assets/audio and ATTRIBUTION.md, then `zsound.py record <commit> {d.name}`")


def cmd_record(commit, name):
    d = work_dir(name)
    meta = read_meta(d)
    meta["promoted"] = f"{commit} ({datetime.date.today().isoformat()})"
    write_meta(d, meta)
    root = art_root()
    if root:
        with open(root / "PROMOTED.md", "a") as fh:
            fh.write(f"- audio/{d.name} -> zombye {commit} ({datetime.date.today().isoformat()})\n")
    print(f"recorded {commit} for {d.name}")


def cmd_check():
    sounds = read_ron(SOUNDS.read_text())["sounds"]
    attribution = ATTRIBUTION.read_text()
    bad = 0
    for cue, s in sounds.items():
        rows = []
        for rel in s["files"]:
            p = AUDIO / rel
            if not p.exists():
                print(f"  {cue}: {rel} missing")
                bad += 1
                continue
            with wave.open(str(p), "rb") as w:
                info = (w.getsampwidth(), w.getframerate(), w.getnchannels())
            if info[0] != 2:
                print(f"  {cue}: {rel} is not 16-bit")
                bad += 1
            if f"`assets/audio/{rel}`" not in attribution:
                print(f"  {cue}: {rel} has no ATTRIBUTION.md row")
                bad += 1
            synth = f"`assets/audio/{rel}`" in attribution and "synth.py" in attribution.split(f"`assets/audio/{rel}`")[1].split("\n")[0]
            chans, rate = decode(p, info[1], info[2] == 2)
            rows.append(f"{Path(rel).name} {len(chans[0]) / rate:.2f}s rms {db(rms(chans)):.0f}" + (" (synth)" if synth else ""))
        print(f"{cue:13} vol {s['volume']:.2f}  " + ", ".join(rows))
    print("ok" if not bad else f"{bad} problems")
    return 1 if bad else 0


def main(argv):
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    cmd, args = argv[0], argv[1:]
    if cmd == "start":
        cmd_start(args[0], args[1] if len(args) > 1 else "Cue")
    elif cmd == "split":
        kw = {}
        if "--db" in args:
            kw["threshold_db"] = float(args[args.index("--db") + 1])
        if "--gap" in args:
            kw["min_gap_ms"] = float(args[args.index("--gap") + 1])
        cmd_split(args[0], **kw)
    elif cmd == "clean":
        cmd_clean(args[0])
    elif cmd == "audition":
        cmd_audition(args[0])
    elif cmd == "promote":
        cmd_promote(args[0])
    elif cmd == "record":
        cmd_record(args[0], args[1])
    elif cmd == "check":
        return cmd_check()
    else:
        raise SystemExit(f"unknown command {cmd}: start, split, clean, audition, promote, record, check")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
