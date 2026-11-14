# Security

## Reporting a problem

Please don't report a security problem in a public issue, a discussion or on Discord.

Report it privately on GitHub instead: on this repository's **Security** tab, choose
**Report a vulnerability**. Only the maintainers see it. Say what you found, how to make it happen,
and which commit or build you used. Check a report a tool wrote before you send it ([AI.md](AI.md)).

We'll answer within a week, keep you told while we fix it, and credit you in the release notes
unless you'd rather we didn't.

## What counts

Anything that lets someone do more than the game means them to, for example:

- a client that can crash, take over or read the memory of a server or another client;
- a mod or a server's downloaded mod data getting out of the script sandbox, or reading or writing
  files it shouldn't;
- a way round a server's password, bans or the identity check on joining;
- the MCP endpoints (`--mcp-http`, `--admin-mcp`) answering without their token.

Cheating that a fair server should stop (seeing players you shouldn't, for example) is a bug: an
ordinary issue is fine for that.

## Supported versions

The game is pre-alpha. Fixes go into `playtest` and then `main`; there are no older versions to patch.
