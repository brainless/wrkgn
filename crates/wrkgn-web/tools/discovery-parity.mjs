#!/usr/bin/env node
// Deliberate maintainer check. Reads only the staged public JSON and local HTML.
import assert from 'node:assert/strict';
import path from 'node:path';
import { loadSnapshot } from '../../../../website/lib/public-snapshot.mjs';
import { matchingProjects, optionsFrom, stateUrl, SORT_OPTIONS } from '../../../../website/src/lib/discovery-state.mjs';
const [root = 'website/data/public', origin = 'http://127.0.0.1:3000'] = process.argv.slice(2);
const snapshot = loadSnapshot(path.resolve(root));
const options = optionsFrom(snapshot.discovery);
const health = await (await fetch(new URL('/healthz', origin))).json();
if (!JSON.stringify(health).includes(snapshot.pointer.version)) throw new Error('Server and JSON snapshots differ');
const first = snapshot.discovery[0];
const queries = ['', 'a', 'é', 'café', 'no-such-project-🦀', first?.builder_usernames[0], first?.name?.slice(0, 8)].filter(q => q != null);
const filters = [
  { category: [], audience: [] },
  ...options.category.map(value => ({ category: [value], audience: [] })),
  ...options.audience.map(value => ({ category: [], audience: [value] })),
  { category: options.category.slice(0, 2), audience: options.audience.slice(0, 2) },
];
let checks = 0;
for (const { value: sort } of SORT_OPTIONS) for (const q of new Set(queries)) for (const filter of filters) {
  const state = { q: [...q.trim()].slice(0, 100).join(''), ...filter, sort, page: 1 };
  const matches = matchingProjects(snapshot.discovery, state);
  const pages = Math.max(1, Math.ceil(matches.length / 30));
  for (const page of new Set([1, Math.min(2, pages), pages])) {
    state.page = page;
    const url = stateUrl(state);
    const response = await fetch(new URL(url, origin));
    assert.equal(response.status, 200, url);
    const html = await response.text();
    const ids = [...html.matchAll(/data-project-id="(\d+)"/g)].map(m => Number(m[1]));
    assert.deepEqual(ids, matches.slice((page - 1) * 30, page * 30).map(p => p.id), url);
    assert.match(html, new RegExp(`data-snapshot-version="${snapshot.pointer.version}"`), url);
    checks++;
  }
}
console.log(`Discovery parity passed: ${checks} states, snapshot ${snapshot.pointer.version}`);
