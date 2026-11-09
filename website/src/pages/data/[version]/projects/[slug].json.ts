import type { APIRoute } from 'astro';
import { snapshot } from '../../../../lib/snapshot.mjs';

export function getStaticPaths() {
  return [...snapshot.details.keys()].map((slug) => ({
    params: { version: snapshot.pointer.version, slug },
  }));
}

export const GET: APIRoute = ({ params }) => new Response(JSON.stringify(snapshot.details.get(params.slug!)), {
  headers: { 'Content-Type': 'application/json; charset=utf-8' },
});
