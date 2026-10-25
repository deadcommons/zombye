---
title: Sounds
description: sounds.ron, which files play for each sound in the game.
---

`sounds.ron` maps every sound the game makes to its files and how it plays. The code only knows the names of the
sounds (cues); everything about how they sound is here. Change a sound by changing its files.

```rust
(
    sounds: {
        Groan: (files: ["sfx/groan_1.wav", "sfx/groan_2.wav", "sfx/groan_3.wav"], volume: 0.42,
            pitch: (0.85, 1.15), reach: 40.0, muffle: 0.45, voices: 3),
        DoorClose: (files: ["sfx/door_close_1.wav", "sfx/door_close_2.wav"], volume: 0.7,
            pitch: (0.92, 1.08), reach: 35.0, voices: 3),
        // ...
    },
)
```

| Field | What it is |
|---|---|
| `files` | Paths under the `audio` folder. One is picked at random each time. |
| `volume` | Loudness at the source, 0 to 2 (1 is as recorded). |
| `pitch` | Playback speed, picked in this range each time for variety. |
| `reach` | Metres at which it fades to nothing. It falls off with the square of the distance. |
| `muffle` | How much gets through a wall, shut door or window, low-passed. |
| `voices` | At most this many playing at once; the nearest win. |

## Cues

Every cue must be in the file:

| Group | Cues |
|---|---|
| Moving | `FootstepHard`, `FootstepSoft`, `Shuffle` (zombies dragging their feet) |
| Fighting | `SwingLight`, `SwingHeavy`, `HitBlunt`, `HitBlade`, `Punch`, `BodyFall`, `Stomp`, `Shove`, `PlayerHurt` |
| Zombies | `Groan`, `ThumpDoor`, `ThumpWindow` |
| Doors and windows | `DoorOpen`, `DoorClose`, `WindowOpen`, `WindowClose`, `GlassBreak`, `DoorBreak` |
| Things | `Rummage`, `Pickup`, `LevelUp` |
| Ambience | `Bird`, and the loops `Wind`, `Room` (indoors) and `Crickets` (at night). These play where you are, so `reach` and `muffle` don't apply. |

## Files

16-bit PCM WAV, mono or stereo, 8 to 96 kHz. A data folder with its own `sounds.ron` reads its files from an `audio`
folder beside it:

```text
mods/my-sounds/
  data/
    sounds.ron
    ...
  audio/
    sfx/groan_1.wav
    ...
```

The sounds in the game now are placeholders made by `tools/audio/synth.py`; the real ones replace them file for file.
Every sound file in the repository needs a row in `ATTRIBUTION.md`.

Sounds are only what the client plays. What zombies hear is decided by the simulation (a door is 10 m of noise
whatever it sounds like), so changing a sound never changes how the game plays.
