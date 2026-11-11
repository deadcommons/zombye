# Zombye website

The one-page landing (`src/pages/index.astro`) and the docs (`src/content/docs/docs/`, served at `/docs/`), built with
[Astro Starlight](https://starlight.astro.build).

```bash
npm install
npm run dev      # http://localhost:4321
npm run build    # static site in dist/
```

- **Docs pages** are Markdown in `src/content/docs/docs/`. Add a page there and list it in the `sidebar` in
  `astro.config.mjs`.
- **The landing page** is plain Astro with its own styles. Its pictures are real screenshots of the game in
  `src/assets/shots/`. The Steam buttons say "coming soon" until `steam` at the top of `index.astro` is set to the
  store page's address.
- **The agent terminal** on the landing shows `src/assets/observe.txt`, real output of the MCP `observe` tool.
- **Theme:** bone, ink and blood; Anton for display, Inter for text. Docs colours are in `src/styles/docs.css`.

When the game changes, change its docs in the same pull request: options, keys, MCP tools and data fields are all
described here.
