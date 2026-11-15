import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';
// Compile the actual browser module, rather than maintaining a JS test copy.
const compiled = await build({ entryPoints: [new URL('../src/canonical.ts', import.meta.url).pathname], bundle: true, write: false, format: 'esm', target: 'es2022' });
const { resolveUrl, stateUrl, readState } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString('base64')}`);
const fixture = JSON.parse(await readFile(new URL('../../tests/fixtures/canonical-urls.json', import.meta.url), 'utf8'));
const projects = Array.from({ length: fixture.project_count }, (_, i) => ({
  category: (i + 1) % 2 === 0 ? 'education' : 'productivity',
  audience: 'researchers', searchable: 'field notes café 🦀',
}));
const count = state => projects.filter(project =>
  project.searchable.includes(state.q.toLowerCase())
  && (!state.category.length || state.category.includes(project.category))
  && (!state.audience.length || state.audience.includes(project.audience))).length;
const options = { category: ['education', 'productivity'], audience: ['researchers'] };
for (const item of fixture.cases) test(item.name, () => {
  assert.deepEqual(resolveUrl(item.input, options, count), { status: item.status, location: item.location });
});
test('query cap counts Unicode code points and trims newly trailing spaces', () => {
  const q = '🦀'.repeat(99) + ' a';
  assert.equal(readState('/', new URLSearchParams({ q }).toString()).q, '🦀'.repeat(99));
});
test('forms serialize repeated filters and default state canonically', () => {
  const state = readState('/', 'q=+field+&category=productivity&category=education&category=education&sort=date-desc&page=1');
  assert.equal(stateUrl(state), '/?q=field&category=education&category=productivity');
});
