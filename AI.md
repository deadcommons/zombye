# Using AI on Zombye

Zombye is built for AI agents to read, play and mod it, and we use AI to build it. Help made with AI is
welcome. So is help made without it. These rules keep the work good and the reviews fair. They add to
[CONTRIBUTING.md](CONTRIBUTING.md); they don't replace it.

## You answer for what you send

Whoever opens a pull request has read and checked all of it, and can explain it and fix it in review.
That includes everything an agent wrote. Your DCO sign-off (`git commit -s`) says so. An agent never
signs off on its own.

## Say when AI did a real part of the work

Add a trailer to the commit, one line per tool:

```
Assisted-by: Claude (Anthropic)
```

Use it for code, text, art or sound that a tool wrote or generated. Spell-checking and autocomplete
don't count. Don't credit a tool with `Co-authored-by:`, which is for people.

## Show what it does

An agent that changes something a player sees or hears renders it and attaches the pictures or a clip to
the pull request, as [CONTRIBUTING.md](CONTRIBUTING.md) asks of everyone. The person it works for looks
at them before the pull request goes out: a picture is how you check a change you didn't make by hand.

## Worth reviewing

Open an issue first for anything bigger than a fix, as for any change. A pull request should cost us
less time to review than it saves. Large unreviewed AI output will be closed and labelled
`extractive`. Maintainers may block anyone who keeps sending it.

## Agents need a person

An agent may open issues and pull requests only for a named person, who reads them before they go out
and answers for them. No agent works in our spaces unattended: not on GitHub, Discord or our servers.
In Discord, talk to people in your own words.

## Art, models, animation and sound

AI-made or AI-assisted assets are welcome under the same rules as any asset
([ATTRIBUTION.md](ATTRIBUTION.md)):

- **A tool whose terms let you licence the output to us openly**, which usually means a paid plan.
  Name the tool and plan in the asset's ATTRIBUTION.md row.
- **Your own prompts and references.** No other games' art, screenshots or files, no living artist's
  name or style asked for by name, nothing of someone else's uploaded.
- **Rework it.** Clean up the mesh, fix the scale, check the UVs, fit it to our skeleton and palette.
  Say in the row what you changed. Purely generated files may not be protected by copyright, and we
  would rather ship work with a person's hand in it.
- **Some tools forbid publishing their output as files.** Their output can't go in this repo. Ask us
  first.

We tell players on Steam which AI-made content ships in the official game, and ATTRIBUTION.md is our
record for that. A row left blank holds that up.

## Bug and security reports

Report what you saw yourself. Check a report a tool wrote before you send it.

## Hand-made work

Hand-made work is just as welcome, and a large part of why we are asking artists to join. Some players
care how a thing was made, so say if yours is all your own work.
