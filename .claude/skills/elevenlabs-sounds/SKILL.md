---
name: elevenlabs-sounds
description: Make Zombye's real sound effects (#78) with ElevenLabs Sound Effects over its MCP, then trim, clean, audition and promote them with tools/audio/zsound.py. Use when replacing a placeholder sound, adding a sound to a cue in assets/data/sounds.ron, or spending ElevenLabs credits. Not for music (Suno, by hand) or zombie groans we record ourselves.
---

# Making sounds with ElevenLabs

The client plays cues (`assets/data/sounds.ron`, see Sound in AGENTS.md); every file there began as a
placeholder from `tools/audio/synth.py`. This skill replaces them **file for file**: same names, so the
code and `sounds.ron` don't change unless a cue gains or loses variants.

Sources agreed in #78: **sound effects from ElevenLabs Sound Effects on a paid plan**, music from Suno (by
hand), zombie groans recorded by us, ambience still generated in code unless a generated loop is clearly
better. **No stock libraries or asset packs, even ones the owner bought** (rule 6; their licences don't
allow the raw files in a public repo).

## Rules first

- **ElevenLabs output never goes in the public repo** (#154). Its Sound Effects and Music terms forbid
  publishing it "as isolated audio files" or as a library without their written OK, and this repo hands
  out every file under CC BY-NC-SA. `zsound.py promote` sends any spec whose generator is ElevenLabs to the
  art repo's `release-audio/` instead (release builds carry it; the client's `release_audio.rs`). The public
  repo keeps the synth placeholders and sounds we record ourselves. `release_only: false` in a spec only
  once ElevenLabs has agreed in writing. Voice Changer output counts as ElevenLabs output too.
- **Credits are money.** Before the first paid call of a session, say what you'll generate, how many
  takes, and the expected credits, and **wait for the owner's typed OK**. Check the balance with the
  MCP's `check_subscription` (free) before and after each batch and keep a running total in the work
  folder's `meta.yml` `notes`. Stop and ask before going past the agreed budget. **Never retry a call
  blindly:** after an error, check the balance and the output folder first (it may have billed).
- **Paid plan only.** The free plan's output is for non-commercial use with attribution; paid plans give
  commercial rights. Check the tier in `check_subscription` and record it in the spec's `generator`
  (e.g. `ElevenLabs Sound Effects (Creator)`); `promote` refuses a spec still saying `PLAN`.
- **Clean room.** Prompts are our own words describing the sound itself: the material, the action, the
  space, the distance. Never name or imitate another game, film or sound library.
- **Work in progress is private.** Takes, specs and auditions live in the art repo (`~/games/zombye-art`,
  `audio/<cue>/`; without it, `target/audio-wip/`). Only promoted files, their ATTRIBUTION.md rows and
  (since the rows quote them) their prompts come into this repo.
- **Keep the raw takes** byte for byte in `raw/`, and list every take in the spec's `takes`, kept or
  not: that's the record of what was spent and what each file came from.
- **Where:** the ElevenLabs MCP (`uvx elevenlabs-mcp`) and its key are only on the owner's Mac. From a
  cloud or build machine, ask for a Remote Control session on the Mac for the generating step; clean-up
  and promotion run anywhere with Python 3 (ffmpeg for MP3 takes).

## Steps

| Step | What | Credits |
|---|---|---|
| 1. Brief | `python3 tools/audio/zsound.py start <cue-name> <Cue>` (e.g. `start footstep-hard FootstepHard`): the work folder, a spec to fill in (`cue`, `folder`, `what`), `meta.yml`. Listen to the placeholder and read the cue's line in `sounds.ron` (how loud, how far, how many at once) and where it plays in `audio.rs`. | 0 |
| 2. Prompts | Write one prompt per take into the spec's `takes` (see Prompts below). Show the owner the list with the cost. | 0 |
| 3. Generate | MCP `text_to_sound_effects` with the prompt, a `duration_seconds` (long enough for several separate hits: one take can give every variant), `output_directory` the folder's `raw/`; WAV/PCM output if the tool offers it, else MP3. Rename each to `take_N.<ext>` and note it in `takes`. Two or three takes per cue, not more. | per take (see Costs) |
| 4. Slice | `zsound.py split raw/take_N.mp3` lists the separate sounds it finds as `files` lines (`--db -35` hears quieter ones, `--gap 80` splits closer ones). Pick the best, name them as the files they replace (`footstep_hard_1` ...). | 0 |
| 5. Clean | `zsound.py clean <cue-name>`: export/*.wav, with each file's length, peak, loudness and how much was trimmed. Tune `clean` in the spec (high-pass, trim, fades, `max_secs`, per-file `gain_db` to even variants out) and run it again. | 0 |
| 6. Review | `zsound.py audition <cue-name>` writes `renders/audition.wav` (every file in order, loops twice). Send it to the owner (attach it, or play it on the Mac) with the placeholder for comparison. Then try it in game: copy `export/` over the files in a scratch `--data` copy, or promote to a branch and run `cargo run -p zombye-client --profile play`. **Gate: the owner approves in words**; set `status: ready` in `meta.yml`. | 0 |
| 7. Promote | `zsound.py promote <cue-name>` copies ElevenLabs files to the art repo's `release-audio/audio/<folder>/` with rows in its ATTRIBUTION.md (commit them there), and our own recordings to `assets/audio/<folder>/` with ATTRIBUTION.md rows here; it notes files `sounds.ron` doesn't list yet (for release-only ones, `release-audio/data/sounds.ron`). Adjust the cue's `volume`, `pitch`, `voices` in `sounds.ron` by ear if needed. `zsound.py check` must say `ok`, `cargo test -p zombye-client --release audio` must pass. Commit (`Refs #78`), then `zsound.py record <commit> <cue-name>`. | 0 |

When the last placeholder of a kind goes, drop its maker from `synth.py` (its `add(...)` line), so
running it again doesn't overwrite a real sound.

## Prompts

Describe one sound as heard close up in a quiet space, dry (the client adds distance, muffling and
panning), in plain physical terms: what hits what, how hard, the material, the size of the space. Ask for
several separate sounds with silence between when you want variants. For example:

- *Footstep hard:* "Four single footsteps of a work boot on a concrete floor, walking pace, each separate
  with a second of silence between, close and dry, no reverb, no music."
- *Blunt hit:* "A wooden baseball bat striking a heavy wet sack of meat, a dull thud with a short crunch,
  three separate hits with silence between, close and dry, no voice."
- *Door thump:* "A heavy fist pounding once on a wooden interior door, the door rattling in its frame,
  four separate blows with pauses, recorded from the other side of the door."
- *Glass:* "A single pane of window glass smashed by a blunt object, shards falling onto a wooden floor."

Avoid voices in SFX takes unless the cue is a voice (player hurt): ask for "no voice, no music".
Loops (`loop: true` in `clean`) want a steady sound with no events near the ends, 6 to 12 s.

## Costs (check them: they change)

ElevenLabs bills sound effects in credits per generation, more for a set duration than an automatic one,
and plans differ in credits a month. **Read the cost of the first take off `check_subscription` (before
and after)** and use that for the rest of the estimate; write it here when you know it. A cue of four
variants from one or two takes is the usual case; the core set (footsteps, swings, hits, body fall,
stomp, shove, doors, windows, glass, thumps, gunshot, reload, rummage, pickup) is about 20 cues.

## The files

16-bit PCM WAV, mono, 44.1 kHz by default (stereo for ambience loops, `stereo: true`), peak at -1 dBFS:
the same peak as the placeholders, so the volumes in `sounds.ron` still mean what they did. Keep one-shots
short (a footstep under 0.3 s, a hit under 0.6 s, a door under 1.5 s): the client plays them often and
`voices` caps how many play at once.
