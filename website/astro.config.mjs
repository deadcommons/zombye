// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import starlightLlmsTxt from 'starlight-llms-txt';

const repo = 'https://github.com/deadcommons/zombye';

// The landing page is src/pages/index.astro; the docs are Starlight, under /docs/. Every docs page is also served as
// Markdown at the same path plus .md (src/pages/[...slug].md.ts), and /llms.txt lists them for language models.
// SITE_URL is where the site is deployed; llms.txt and the sitemap need absolute links.
export default defineConfig({
	site: process.env.SITE_URL ?? 'http://localhost:4321',
	integrations: [
		starlight({
			title: 'Zombye',
			plugins: [
				starlightLinksValidator(),
				starlightLlmsTxt({
					projectName: 'Zombye',
					description:
						'Open-source zombie survival in real 3D (Rust + Bevy). Its server speaks MCP, so AI agents can play, test and mod it; every gameplay value is in plain RON data files.',
					details:
						'To play: build `zombye-server`, add `zombye-server --mcp` to your MCP client (see Connect your agent), then call `observe`. Time only advances when a tool steps it. To mod: copy `assets/data`, edit it, run with `--data DIR`. Every docs page is also available as Markdown at its URL with `.md` added.',
					promote: ['docs/agents*', 'docs/modding*'],
					customSets: [
						{
							label: 'Playing with agents',
							description: 'connecting an MCP client to zombye-server, a first session, testing, and every MCP tool',
							paths: ['docs/agents/**', 'docs/agents'],
						},
						{
							label: 'Modding',
							description: 'the data files (items, weapons, recipes, loot, buildings, population, sounds) and how to change them',
							paths: ['docs/modding/**', 'docs/modding'],
						},
					],
				}),
			],
			description: 'Open-source zombie survival in real 3D, built for players, modders and AI agents.',
			favicon: '/favicon.svg',
			customCss: ['@fontsource/anton/400.css', '@fontsource-variable/inter', './src/styles/docs.css'],
			social: [
				{ icon: 'github', label: 'GitHub', href: repo },
				{ icon: 'discord', label: 'Discord', href: 'https://discord.gg/y7NsmVG2br' },
			],
			editLink: { baseUrl: `${repo}/edit/main/website/` },
			lastUpdated: true,
			components: { PageTitle: './src/components/PageTitle.astro' },
			sidebar: [
				{
					label: 'Start here',
					items: [
						{ label: 'What Zombye is', slug: 'docs' },
						{ label: 'Build and run', slug: 'docs/start/install' },
					],
				},
				{
					label: 'Play',
					items: [
						{ label: 'Controls', slug: 'docs/play/controls' },
						{ label: 'Your first night', slug: 'docs/play/first-night' },
						{ label: 'How the world works', slug: 'docs/play/world' },
					],
				},
				{
					label: 'Play with agents',
					items: [
						{ label: 'Agents and MCP', slug: 'docs/agents' },
						{ label: 'Connect your agent', slug: 'docs/agents/connect' },
						{ label: 'Tutorial: an agent survives', slug: 'docs/agents/tutorial' },
						{ label: 'Testing and debugging', slug: 'docs/agents/testing' },
						{ label: 'MCP tool reference', slug: 'docs/agents/tools' },
					],
				},
				{
					label: 'Modding',
					items: [
						{ label: 'How modding works', slug: 'docs/modding' },
						{ label: 'Tutorial: a nail bat', slug: 'docs/modding/first-mod' },
						{ label: 'Modding with an agent', slug: 'docs/modding/with-agents' },
						{ label: 'Items', slug: 'docs/modding/items' },
						{ label: 'Weapons', slug: 'docs/modding/weapons' },
						{ label: 'Recipes', slug: 'docs/modding/recipes' },
						{ label: 'Loot and containers', slug: 'docs/modding/loot' },
						{ label: 'Buildings', slug: 'docs/modding/buildings' },
						{ label: 'Zombie population', slug: 'docs/modding/population' },
						{ label: 'Sounds', slug: 'docs/modding/sounds' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'Command-line options', slug: 'docs/reference/cli' },
						{ label: 'Saves', slug: 'docs/reference/saves' },
					],
				},
			],
		}),
	],
});
