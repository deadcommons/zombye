# AGENTS.md: the Zombye website

Astro Starlight site: the landing page is `src/pages/index.astro`, the docs are Markdown in `src/content/docs/docs/`
(served under `/docs/`), and the sidebar is in `astro.config.mjs`. See [README.md](README.md) for commands.

Rules:

- The repo's own [AGENTS.md](../AGENTS.md) rules apply, including never naming other games.
- Docs describe what the game does now. Check facts against the code, the data files' header comments and the MCP
  server's own tool descriptions (`tools/list`) rather than memory; quote real tool output, never made-up output.
- Change the docs in the same change as the game feature they describe.
- Run `npm run build` before committing: broken links to docs pages and bad frontmatter fail the build.
