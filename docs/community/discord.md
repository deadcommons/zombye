# Discord server

The Zombye community server, as set up on 2026-10-23. This page records the setup so it can
be recreated or audited. Change it in the same PR whenever the server changes.

- **Name:** Zombye. Renamed from Shamblewood on 2026-10-23 (server, webhook and #welcome).
- **Invite:** <https://discord.gg/y7NsmVG2br> (never expires, no use limit, lands in #welcome)
- **Access:** invite only. Not Discoverable, no "Apply to Join".
- **Bots and integrations:** one webhook, **Zombye GitHub**, posting to #devlog (Server
  Settings → Integrations → Webhooks). Its URL is a secret: keep it out of the repo, and put it
  only in GitHub's webhook settings, with `/github` appended and content type
  `application/json`.
- **Wording:** Discord copy doesn't name other games or studios. The clean-room and piracy rules
  are written generically.

## Channels

| Category | Channel | Type | Notes |
|---|---|---|---|
| Info | #welcome | Text | Read-only. What Zombye is, links, rules. Community rules channel. |
| Info | #announcements | Text | Read-only. |
| Info | #devlog | Text | Read-only. Progress posts; GitHub feed later. |
| Community | #general | Text | Server system messages go here. |
| Community | #ideas | Forum | Tags: gameplay, combat, crafting, world, UI. Tags optional. |
| Community | #clips-and-screenshots | Text | |
| Build | #help | Forum | |
| Build | #dev | Text | Code and engine. |
| Build | #modding | Text | Opt-in through onboarding. |
| Build | #agents | Text | People building with AI agents and MCP. Opt-in through onboarding. |
| Voice | Safehouse | Voice | |
| (none) | #moderator-only | Text | Private, staff only. Created by Discord when Community was enabled. Receives Community updates, safety notifications and AutoMod alerts. |

The Info category denies @everyone **Send Messages and Create Posts**, **Send Messages in Threads
and Posts**, **Create Public Threads** and **Create Private Threads**. Maintainer is allowed the
first three, so maintainers can post devlogs. The three Info channels are synced to the category.

#announcements and #devlog are plain read-only text channels, not Announcement channels. Converting
them makes the server profile visible outside the server, so that waits for a decision.

**Not created yet:** #bugs and #playtest. They come when there's a playable build.

## Roles

Highest first. Colours are muted, desaturated small-town tones.

| Role | Colour | Permissions |
|---|---|---|
| Dead Commons | `#9C5B4A` | Administrator (staff) |
| Maintainer | `#A8935A` | Moderation only: Manage Messages, Manage Threads, Kick, Ban, Time out, View Audit Log, Manage Nicknames, Mute Members, Move Members |
| Contributor | `#7E9076` | None beyond @everyone |
| Modder | `#5F8A8B` | None beyond @everyone |
| Agent Builder | `#6E7F99` | None beyond @everyone |
| Playtester | `#8E7A8F` | None beyond @everyone (opt-in) |
| @everyone | | Discord defaults, with the Community "risky permissions" removed (Create Expressions, Create Events, Mention @everyone) |

Only Dead Commons and Maintainer have admin or moderation rights.

## Safety

- **Community:** enabled. Rules channel #welcome; Community updates and safety notifications go to
  #moderator-only. Default notifications: mentions only.
- **Verification level:** Medium (verified email, account older than 5 minutes).
- **Explicit media filter:** scan media from all members.
- **DM and spam protection:** all Discord defaults on (hide suspicious DMs, filter DMs from unknown
  users, warn before outbound links, hide and delete suspected spammers).
- **Rules screening (Access → Server Rules):** on, with the six rules below.
- **Server description:** "An open-source survival sandbox: isometric zombie survival in real 3D,
  built in Rust. Moddable by default, agent-first. Pre-alpha."

### AutoMod

| Rule | Trigger | Action | Exempt |
|---|---|---|---|
| Block Mention Spam | Discord default mention limit | Block message | Admins |
| Block Suspected Spam Content | Discord spam detection | Block message, alert #moderator-only | Admins, #moderator-only |
| No piracy | Keywords: `*torrent*`, `warez`, `fitgirl`, `skidrow`, `steamunlocked`, `igg-games`, `cracked version`, `cracked copy`, `pirated copy`, `free crack`, `crack download` | Block message, alert #moderator-only | Admins, Maintainer, #moderator-only |
| No invite links (non-staff) | Keywords: `*discord.gg/*`, `*discord.com/invite*`, `*discordapp.com/invite*`, `*dsc.gg/*` | Block message | Admins, Maintainer |

Members with Administrator or Manage Server are always exempt from AutoMod.

## Onboarding

Enabled. Server Guide not set up (kept lean).

**Default channels:** #welcome, #announcements, #devlog, #general, #ideas, #clips-and-screenshots,
#help, #dev, Safehouse.

**Pre-join question: "What brings you here?"** Multiple answers allowed, not required.

| Answer | Description | Role | Channels |
|---|---|---|---|
| Play it | Get the Playtester role and a ping when there's a build to try | Playtester | #clips-and-screenshots, #ideas |
| Mod it | Get the Modder role and the #modding channel | Modder | #modding |
| Build it (code) | Get the Contributor role and the #dev channel | Contributor | #dev |
| Build with AI agents | Get the Agent Builder role and the #agents channel (MCP, bots, tools) | Agent Builder | #agents |
| Just watching | Follow #devlog and #announcements, no role needed | (none) | #devlog, #announcements |

## Posted messages

Channel mentions are written as `<#channel-id>` when posting, so they render as links. Shown here
by name.

### #welcome: welcome

```markdown
# Welcome to Zombye
Zombye is an open-source survival sandbox: isometric zombie survival in real 3D, built in Rust with Bevy. Wake up in an ordinary early-1990s small town, scavenge, sneak or fight, and see how long you last.

**What's different**
• Real 3D, viewed from a fixed isometric camera
• Fast: 1,000+ zombies simulated, on a headless, deterministic server
• Moddable by default: every gameplay value lives in a data file
• Agent-first: the server has an MCP endpoint, so AI agents can play, test and build mods
• Host it anywhere: self-hosting is free. Official one-click hosting comes later and pays for development.

**Status:** pre-alpha (milestone M0, tech spike). Not playable yet. Follow #devlog for progress.

**How to help**
• Code: read CONTRIBUTING.md first, especially the clean-room rules, then say hi in #dev
• Mods and data: #modding · AI agents and MCP: #agents
• Ideas go in #ideas. Stuck? Ask in #help.

**Links**
• Source: <https://github.com/deadcommons/zombye>
• Licence: MIT OR Apache-2.0. The Zombye and Dead Commons names and logos are trademarks.
```

### #welcome: rules

The same six rules, without the heading and footer, are used for rules screening.

```markdown
# Rules
1. **Be kind.** No harassment, insults, threats, discrimination, sexual content or doxxing. Critique work, not people.
2. **No piracy.** Don't share or ask for cracked copies of any game.
3. **Respect the clean room.** Don't post files, code, assets or mods taken from other games. Talking about games you love, and sharing your own clips of them, is fine.
4. **Label AI-generated content.** Say when a post, image, mod or code was made with AI.
5. **No spam**, including agent-generated spam: mass posts, automated replies, low-effort promotion.
6. **Report problems** to anyone with the **Dead Commons** or **Maintainer** role, by DM or with Discord's report button. Reports stay confidential.

The full code of conduct applies here: <https://github.com/deadcommons/zombye/blob/main/CODE_OF_CONDUCT.md>. Maintainers may remove content, warn, time out or ban, in proportion to what happened.
```

Write list items as `1. **Title.** text`. Discord turns `**1. Title.**` into a broken list.

### #announcements: first post

```markdown
# Zombye is in the open: here's where we are
Hi all. Zombye is an open-source survival sandbox from Dead Commons: isometric zombie survival in real 3D, built in Rust with Bevy, so that people and AI agents can read it, mod it and host it.

**Where we are: M0, the tech spike.** It isn't a game yet. What exists today:
• A deterministic simulation: tile grid, zombies and flow-field pathfinding
• A headless server with a benchmark mode and an MCP endpoint. Agents can observe the map, step time, set goals and spawn zombies.
• A Bevy client: an isometric camera over a test town, and a horde of 1,000+ zombies that chases you

**Next up: M1, world and visibility.** Building templates, a seeded town generator, wall cutaways, fog of war and day/night. The full roadmap is on the Game development board (https://github.com/orgs/deadcommons/projects/2), and #devlog will carry progress as it lands.

**Get involved**
• Your answers to "What brings you here?" open the matching channels. You can change them any time under Channels & Roles.
• Read CONTRIBUTING.md before writing code, especially the clean-room rules.
• Ideas go in #ideas, questions in #help.

Bug reports and playtests get their own channels once there's a playable build. Thanks for being here early.
```

## Later

- #bugs (forum) and #playtest, when there's a playable build. Add Playtester to the playtest
  channel and ping it for builds.
- The GitHub webhook to #devlog was connected on 2026-10-23 (pushes and releases).
- Tags for the #help forum, once we see what people ask about.
- Decide whether #announcements and #devlog become Announcement channels (makes the server
  profile visible outside the server).
- Server icon and banner from the brand assets.
