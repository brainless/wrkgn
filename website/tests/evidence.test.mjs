import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { loadSnapshot } from '../lib/public-snapshot.mjs';
test('v5 accepts title evidence and rejects unsafe evidence', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'wrkgn-evidence-'));
  const write = (file, value) => fs.writeFileSync(path.join(temp, file), JSON.stringify(value));
  try {
    fs.cpSync('tests/fixtures/representative', temp, { recursive: true });
    const old = loadSnapshot(temp), prefix = `snapshots/${old.pointer.version}/`;
    write('current.json', { ...old.pointer, schema_version: 5 });
    write(`${prefix}manifest.json`, { ...old.manifest, schema_version: 5, audiences: [{ slug: 'researchers', label: 'Researchers', description: 'Researchers' }] });
    write(`${prefix}discovery.json`, old.discovery.map(({ looking_for, ...row }) => ({ ...row, audience_labels: [], comment_count: 0 })));
    for (const [slug, detail] of old.details) {
      detail.mentions = detail.mentions.map(({ requests, supporting, builder_username, ...mention }) => ({ ...mention, source_excerpt: '', builder_usernames: builder_username ? [builder_username] : [], evidence: [{ source_item_id: mention.source_item_id, source_url: mention.source_url, source_field: 'title', value: 'Show HN: Example', supports: ['name'], author: mention.author, shared_at: mention.shared_at }] }));
      write(`${prefix}projects/${slug}.json`, detail);
    }
    const detail = loadSnapshot(temp).details.values().next().value;
    const evidence = detail.mentions[0].evidence[0];
    assert.equal(evidence.source_field, 'title');
    for (const [field, bad] of [['source_field', 'private'], ['supports', ['unknown']], ['source_url', 'javascript:alert(1)'], ['source_item_id', -1], ['value', '']]) {
      const original = evidence[field]; evidence[field] = bad;
      write(`${prefix}projects/${detail.slug}.json`, detail);
      assert.throws(() => loadSnapshot(temp), /Invalid public snapshot/);
      evidence[field] = original;
    }
    evidence.source_field = 'url'; evidence.value = 'javascript:alert(1)';
    write(`${prefix}projects/${detail.slug}.json`, detail);
    assert.throws(() => loadSnapshot(temp), /URL must use/);
  } finally { fs.rmSync(temp, { recursive: true, force: true }); }
});
