import fs from 'node:fs';
import path from 'node:path';

const VERSION = /^[a-f0-9]{16}$/;
const SLUG = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const REQUESTS = new Set(['feedback', 'contributors', 'collaborators']);

function fail(where, message) {
  throw new Error(`Invalid public snapshot at ${where}: ${message}`);
}
function object(value, where, keys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(where, 'expected an object');
  const actual = Object.keys(value);
  if (actual.length !== keys.length || actual.some((key) => !keys.includes(key))) {
    fail(where, `expected only ${keys.join(', ')}`);
  }
  return value;
}
function string(value, where, nullable = false) {
  if (nullable && value === null) return;
  if (typeof value !== 'string') fail(where, 'expected a string');
}
function integer(value, where, positive = false) {
  if (!Number.isSafeInteger(value) || (positive ? value <= 0 : value < 0)) fail(where, 'expected a valid integer');
}
function list(value, where, check) {
  if (!Array.isArray(value)) fail(where, 'expected an array');
  value.forEach((entry, index) => check(entry, `${where}[${index}]`));
}
function strings(value, where) {
  list(value, where, string);
  if (new Set(value).size !== value.length) fail(where, 'duplicate value');
}
function webUrl(value, where) {
  string(value, where);
  let parsed;
  try { parsed = new URL(value); } catch { fail(where, 'invalid URL'); }
  if (!['https:', 'http:'].includes(parsed.protocol)) fail(where, 'URL must use http or https');
}
function source(value, where) {
  integer(value.source_item_id, `${where}.source_item_id`, true);
  webUrl(value.source_url, `${where}.source_url`);
  const url = new URL(value.source_url);
  if (url.origin !== 'https://news.ycombinator.com' || url.pathname !== '/item' || url.searchParams.get('id') !== String(value.source_item_id)) {
    fail(where, 'source URL must match the HN item ID');
  }
}
function date(value, where, nullable = false) {
  if (nullable && value === null) return;
  integer(value, where);
}
function request(value, where) {
  object(value, where, ['kind', 'source_item_id', 'source_url', 'evidence', 'shared_at']);
  if (!REQUESTS.has(value.kind)) fail(`${where}.kind`, 'unknown request kind');
  source(value, where);
  string(value.evidence, `${where}.evidence`);
  if (!value.evidence.trim()) fail(`${where}.evidence`, 'empty evidence');
  date(value.shared_at, `${where}.shared_at`, true);
}
function supporting(value, where) {
  object(value, where, ['source_item_id', 'source_url', 'summary', 'excerpt', 'author', 'shared_at']);
  source(value, where);
  string(value.summary, `${where}.summary`);
  string(value.excerpt, `${where}.excerpt`, true);
  string(value.author, `${where}.author`, true);
  date(value.shared_at, `${where}.shared_at`, true);
}
function evidence(value, where) {
  object(value, where, ['source_item_id', 'source_field', 'value', 'supports', 'author', 'shared_at', 'source_url']);
  source(value, where);
  if (!['title', 'url', 'body'].includes(value.source_field)) fail(where, 'unknown evidence source field');
  string(value.value, `${where}.value`);
  if (!value.value.trim()) fail(where, 'empty evidence');
  if (value.source_field === 'url') webUrl(value.value, `${where}.value`);
  strings(value.supports, `${where}.supports`);
  const fields = ['name', 'urls', 'description', 'problem', 'audience', 'builders', 'categories', 'audience_labels'];
  if (value.supports.some((field) => !fields.includes(field))) fail(where, 'unknown supported field');
  string(value.author, `${where}.author`, true);
  date(value.shared_at, `${where}.shared_at`);
}
function mention(value, where, schema = 2) {
  object(value, where, ['name', 'description', 'problem', 'audience', 'builder_username', 'source_item_id', 'source_url', 'author', 'shared_at', 'links', ...(schema === 2 ? ['requests', 'supporting'] : ['source_excerpt', 'builder_usernames']), ...(schema >= 5 ? ['evidence'] : [])].filter((key) => schema === 2 || key !== 'builder_username'));
  string(value.name, `${where}.name`, true);
  string(value.description, `${where}.description`);
  if (!value.description.trim()) fail(`${where}.description`, 'empty description');
  string(value.problem, `${where}.problem`, true);
  string(value.audience, `${where}.audience`, true);
  if (schema === 2) string(value.builder_username, `${where}.builder_username`, true);
  else {
    string(value.source_excerpt, `${where}.source_excerpt`);
    if (schema < 5 && !value.source_excerpt.trim()) fail(`${where}.source_excerpt`, 'empty source excerpt');
    strings(value.builder_usernames, `${where}.builder_usernames`);
  }
  if (schema >= 5) {
    list(value.evidence, `${where}.evidence`, evidence);
    if (!value.evidence.length) fail(where, 'missing evidence');
  }
  string(value.author, `${where}.author`, true);
  source(value, where);
  date(value.shared_at, `${where}.shared_at`);
  list(value.links, `${where}.links`, webUrl);
  if (schema === 2) {
    list(value.requests, `${where}.requests`, request);
    list(value.supporting, `${where}.supporting`, supporting);
  }
}
function discovery(value, where, schema = 2) {
  object(value, where, ['id', 'slug', 'name', 'description', 'problem', 'audience', 'builder_usernames', 'categories', 'audience_labels', 'looking_for', 'latest_shared_at', 'searchable_text', ...(schema >= 4 ? ['comment_count'] : [])].filter((key) => schema === 2 || key !== 'looking_for'));
  integer(value.id, `${where}.id`, true);
  string(value.slug, `${where}.slug`);
  if (!SLUG.test(value.slug)) fail(`${where}.slug`, 'unsafe slug');
  string(value.name, `${where}.name`, true);
  string(value.description, `${where}.description`);
  if (!value.description.trim()) fail(`${where}.description`, 'empty description');
  string(value.problem, `${where}.problem`, true);
  string(value.audience, `${where}.audience`, true);
  strings(value.builder_usernames, `${where}.builder_usernames`);
  strings(value.categories, `${where}.categories`);
  strings(value.audience_labels, `${where}.audience_labels`);
  if (schema === 2) {
    strings(value.looking_for, `${where}.looking_for`);
    for (const kind of value.looking_for) if (!REQUESTS.has(kind)) fail(`${where}.looking_for`, 'unknown request kind');
  }
  if (schema >= 4) integer(value.comment_count, `${where}.comment_count`);
  date(value.latest_shared_at, `${where}.latest_shared_at`);
  string(value.searchable_text, `${where}.searchable_text`);
}
function readJson(root, relative) {
  const file = path.join(root, relative);
  let stat;
  try { stat = fs.lstatSync(file); } catch { fail(relative, 'missing file'); }
  if (!stat.isFile() || stat.isSymbolicLink()) fail(relative, 'expected a regular file');
  try { return JSON.parse(fs.readFileSync(file, 'utf8')); }
  catch (error) { fail(relative, `invalid JSON: ${error.message}`); }
}

/** Read one completed Epic 02 export. This module never reads SQLite or invokes Rust. */
export function loadSnapshot(root) {
  const pointer = readJson(root, 'current.json');
  object(pointer, 'current.json', ['schema_version', 'version', 'snapshot']);
  if (![2, 3, 4, 5].includes(pointer.schema_version) || !VERSION.test(pointer.version)) fail('current.json', 'unsupported schema or version');
  const prefix = `snapshots/${pointer.version}/`;
  if (pointer.snapshot !== `${prefix}manifest.json`) fail('current.json', 'manifest path does not match version');
  const manifest = readJson(root, pointer.snapshot);
  object(manifest, pointer.snapshot, ['schema_version', 'version', 'discovery', 'project_details', 'project_count', 'categories', ...(pointer.schema_version >= 3 ? ['audiences'] : [])]);
  if (manifest.schema_version !== pointer.schema_version || manifest.version !== pointer.version || manifest.discovery !== 'discovery.json' || manifest.project_details !== 'projects/<slug>.json') fail(pointer.snapshot, 'unsupported or mismatched manifest');
  list(manifest.categories, `${pointer.snapshot}.categories`, (value, where) => {
    object(value, where, ['slug', 'label', 'description']);
    string(value.slug, `${where}.slug`); string(value.label, `${where}.label`); string(value.description, `${where}.description`);
    if (!SLUG.test(value.slug) || !value.label.trim() || !value.description.trim()) fail(where, 'invalid category metadata');
  });
  const categorySlugs = new Set(manifest.categories.map((category) => category.slug));
  if (categorySlugs.size !== manifest.categories.length || categorySlugs.size !== 15) fail(pointer.snapshot, 'expected 15 distinct categories');
  if (pointer.schema_version >= 3) {
    list(manifest.audiences, `${pointer.snapshot}.audiences`, (value, where) => {
      object(value, where, ['slug', 'label', 'description']);
      for (const field of ['slug', 'label', 'description']) string(value[field], `${where}.${field}`);
      if (!SLUG.test(value.slug) || !value.label.trim() || !value.description.trim()) fail(where, 'invalid audience metadata');
    });
    if (!manifest.audiences.length || new Set(manifest.audiences.map((item) => item.slug)).size !== manifest.audiences.length) fail(pointer.snapshot, 'expected distinct audiences');
  }
  integer(manifest.project_count, `${pointer.snapshot}.project_count`);
  const rows = readJson(root, `${prefix}discovery.json`);
  list(rows, `${prefix}discovery.json`, (value, where) => discovery(value, where, pointer.schema_version));
  for (const row of rows) for (const category of row.categories) if (!categorySlugs.has(category)) fail(`${prefix}discovery.json`, `unknown category ${category}`);
  if (pointer.schema_version >= 3) {
    const audienceSlugs = new Set(manifest.audiences.map((item) => item.slug));
    for (const row of rows) for (const audience of row.audience_labels) if (!audienceSlugs.has(audience)) fail(`${prefix}discovery.json`, `unknown audience ${audience}`);
  }
  if (rows.length !== manifest.project_count) fail(pointer.snapshot, 'project count mismatch');
  const details = new Map();
  const ids = new Set();
  for (const [index, row] of rows.entries()) {
    if (ids.has(row.id) || details.has(row.slug)) fail(`${prefix}discovery.json[${index}]`, 'duplicate project ID or slug');
    ids.add(row.id);
    const relative = `${prefix}projects/${row.slug}.json`;
    const detail = readJson(root, relative);
    object(detail, relative, ['id', 'slug', 'name', 'mentions']);
    if (detail.id !== row.id || detail.slug !== row.slug || detail.name !== row.name) fail(relative, 'identity differs from discovery');
    list(detail.mentions, `${relative}.mentions`, (value, where) => mention(value, where, pointer.schema_version));
    if (detail.mentions.length === 0) fail(relative, 'project has no mentions');
    if (detail.mentions[0].shared_at !== row.latest_shared_at) fail(relative, 'latest sharing date differs from discovery');
    for (let i = 1; i < detail.mentions.length; i++) {
      if (detail.mentions[i - 1].shared_at < detail.mentions[i].shared_at) fail(relative, 'mentions are out of date order');
    }
    details.set(row.slug, detail);
    if (index > 0) {
      const prev = rows[index - 1];
      if (prev.latest_shared_at < row.latest_shared_at || (prev.latest_shared_at === row.latest_shared_at && prev.id > row.id)) fail(`${prefix}discovery.json`, 'projects are out of date and ID order');
    }
  }
  return { pointer, manifest, discovery: rows, details };
}
