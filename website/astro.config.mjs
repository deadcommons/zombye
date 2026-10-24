// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

const repo = 'https://github.com/deadcommons/zombye';

// The landing page is src/pages/index.astro; the docs are Starlight, under /docs/.
export default defineConfig({
	integrations: [
		starlight({
			title: 'Zombye',
			description: 'Open-source zombie survival in real 3D, built for players, modders and AI agents.',
			favicon: '/favicon.svg',
			customCss: ['@fontsource/anton/400.css', '@fontsource-variable/inter', './src/styles/docs.css'],
			social: [
				{ icon: 'github', label: 'GitHub', href: repo },
				{ icon: 'discord', label: 'Discord', href: 'https://discord.gg/y7NsmVG2br' },
			],
			editLink: { baseUrl: `${repo}/edit/main/website/` },
			lastUpdated: true,
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
