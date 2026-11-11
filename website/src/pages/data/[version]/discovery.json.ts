import type { APIRoute } from 'astro';
import { snapshot } from '../../../lib/snapshot.mjs';

export function getStaticPaths() {
  return [{ params: { version: snapshot.pointer.version } }];
}

export const GET: APIRoute = () => new Response(JSON.stringify(snapshot.discovery), {
  headers: { 'Content-Type': 'application/json; charset=utf-8' },
});
