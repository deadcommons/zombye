---
title: How the world works
description: The rules behind the town, the zombies, your body and your skills.
---

Zombye doesn't hide its rules. Everything below is simulated on the server the same way for every player, person or
agent, and every number is in a [data file](/docs/modding/) you can read.

## The town

Each world is generated from a seed: a main street with shops and offices, a town square, side streets of houses,
cul-de-sacs, and forest all round with roads running into it. Houses come from templates (bungalows, family houses,
townhouses, cottages, offices, a pharmacy, a hardware store) that are turned and mirrored to fit their lots.

One tile is one metre. Walls, doors and windows sit on the edges between tiles. A shut door blocks sight and movement;
a shut window blocks movement but not sight; a fence blocks movement only.

## Zombies

A zombie is always doing one thing:

| State | What it means |
|---|---|
| **Wander** | Drifting about. |
| **Investigate** | Walking to a noise it heard. |
| **Chase** | It has seen you. If it loses sight of you it goes to where it last saw you and searches. |
| **Lunge** and **Attack** | It's close enough to grab you. |
| **Thump** | It's battering a door or window it thinks you're behind. Others join in. |
| **Staggered** and **Down** | You hit or shoved it. Down zombies get back up after a few seconds unless you stomp them. |

Zombies that get stuck give up after a while and go back to wandering, bored, ignoring noises that aren't yours.

Far from every player, zombies are simulated more cheaply: they still wander and drift toward loud noises, so a big
enough noise (a door breaking, a smashed window) can draw a horde in from hundreds of metres away over several
minutes.

## Your body

You have one pool of health and wounds on ten body parts. Each wound has a kind (bruise, scratch, gash, bite, glass
cut), bleeds or not, and heals over game hours. Wounds slow you down where they are: hurt legs mean no running, hurt
arms mean weaker, slower swings, a hurt torso means slower recovery.

Clothing protects the parts it covers. Layers add up, and a thick jacket can turn a bite into a bruise.

Bites (and now and then a gash) from zombies may carry the infection. You won't know for hours. Then comes the fever, then the turn, and when
you die, you get back up as one of them, wearing what you wore.

## Needs

Hunger, thirst and tiredness go from fine to worst over game hours. Each has stages, and each stage has effects:
hungry means slower recovery, starving means losing health; thirsty and parched are worse and faster. Tiredness makes
your swings slow. Sleep brings tiredness down and health back, faster in a bed.

A game hour is two and a half real minutes by default.

## Skills

Nine skills from 0 to 5, raised only by doing the thing:

| Skill | You get better by | And it gives |
|---|---|---|
| Fitness | Running | Running tires you less |
| Strength | Swinging heavy weapons, carrying too much | Carry more, knock them back further |
| Bludgeon | Hitting zombies with blunt weapons | More damage, less tiring |
| Edged | Hitting zombies with knives and axes | More damage, less tiring |
| Brawl | Punching, shoving, stomping | Better punches, more knockdowns |
| Stealth | Sneaking near zombies | Quieter steps, harder to see |
| First Aid | Treating wounds | Faster treatment |
| Scavenging | Searching containers | Faster, quieter searches |
| Handiwork | Crafting | Faster crafting |

Press **K** in game to see where you stand.
