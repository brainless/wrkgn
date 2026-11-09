import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { loadSnapshot } from '../lib/public-snapshot.mjs';
import { matchingProjects, optionsFrom, pageCount, readState, stateUrl } from '../src/lib/discovery-state.mjs';

const fixture = path.resolve('tests/fixtures/representative');
const projects = loadSnapshot(fixture).discovery;
const options = optionsFrom(projects);

test('snapshot contract accepts empty and representative publications', () => {
  assert.equal(loadSnapshot('data/public').discovery.length, 0);
  assert.equal(projects.length, 13);
  assert.equal(projects[0].slug, 'field-notes-2222222222');
  assert.equal(projects[1].id, 1);
  assert.equal(projects.at(-1).id, 13);
});

test('state normalizes invalid values and preserves repeated selected values', () => {
  const state = readState('https://example.test/projects/?q=%20field%20&category=Education&category=Productivity&category=unknown&looking=feedback&page=999', options);
  assert.deepEqual(state.category, ['Education', 'Productivity']);
  assert.deepEqual(state.looking, ['feedback']);
  assert.equal(state.q, 'field');
  assert.equal(state.page, 999);
  assert.equal(pageCount(matchingProjects(projects, state), 12), 1);
  assert.equal(stateUrl({ ...state, page: 1 }), '/projects/?q=field&category=Education&category=Productivity&looking=feedback');
  assert.equal(readState('https://example.test/projects/page/2/?page=bad&audience=unknown', options).page, 1);
});

test('searches the full dataset with OR within and AND across dimensions', () => {
  const base = { q: '', category: [], audience: [], looking: [], page: 1 };
  assert.equal(matchingProjects(projects, { ...base, q: 'samplebuilder' }).length, 2);
  assert.equal(matchingProjects(projects, { ...base, category: ['Education', 'Productivity'] }).length, 13);
  assert.equal(matchingProjects(projects, { ...base, category: ['Education'], audience: ['Researchers'] }).length, 0);
  assert.equal(matchingProjects(projects, { ...base, category: ['Productivity'], audience: ['Researchers'], looking: ['feedback'] }).length, 1);
  assert.equal(matchingProjects(projects, { ...base, looking: ['feedback'], q: 'not present' }).length, 0);
  assert.equal(stateUrl({ ...base, page: 2 }), '/projects/page/2/');
});

test('rejects a pointer outside the selected snapshot', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'wrkgn-invalid-'));
  try {
    fs.cpSync(fixture, temp, { recursive: true });
    const pointer = JSON.parse(fs.readFileSync(path.join(temp, 'current.json')));
    pointer.snapshot = '../private.sqlite';
    fs.writeFileSync(path.join(temp, 'current.json'), JSON.stringify(pointer));
    assert.throws(() => loadSnapshot(temp), /manifest path does not match version/);
  } finally { fs.rmSync(temp, { recursive: true, force: true }); }
});
