// Every docs page as plain Markdown at its URL plus .md (/docs/agents/connect.md), for agents and language models.
// MDX imports are left out; components show as their tags.
import type { APIRoute, GetStaticPaths } from 'astro';
import { getCollection, type CollectionEntry } from 'astro:content';

export const getStaticPaths: GetStaticPaths = async () => {
	const docs = await getCollection('docs');
	return docs.map((entry) => ({ params: { slug: entry.id }, props: { entry } }));
};

export const GET: APIRoute = ({ props }) => {
	const { entry } = props as { entry: CollectionEntry<'docs'> };
	const body = (entry.body ?? '').replace(/^import .*\n/gm, '').trimStart();
	const head = `# ${entry.data.title}\n\n${entry.data.description ? `> ${entry.data.description}\n\n` : ''}`;
	return new Response(head + body, { headers: { 'Content-Type': 'text/markdown; charset=utf-8' } });
};
