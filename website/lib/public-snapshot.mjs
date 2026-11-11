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
function mention(value, where) {
  object(value, where, ['name', 'description', 'problem', 'audience', 'builder_username', 'source_item_id', 'source_url', 'author', 'shared_at', 'links', 'requests', 'supporting']);
  string(value.name, `${where}.name`, true);
  string(value.description, `${where}.description`);
  if (!value.description.trim()) fail(`${where}.description`, 'empty description');
  string(value.problem, `${where}.problem`, true);
  string(value.audience, `${where}.audience`, true);
  string(value.builder_username, `${where}.builder_username`, true);
  string(value.author, `${where}.author`, true);
  source(value, where);
  date(value.shared_at, `${where}.shared_at`);
  list(value.links, `${where}.links`, webUrl);
  list(value.requests, `${where}.requests`, request);
  list(value.supporting, `${where}.supporting`, supporting);
}
function discovery(value, where) {
  object(value, where, ['id', 'slug', 'name', 'description', 'problem', 'audience', 'builder_usernames', 'categories', 'audience_labels', 'looking_for', 'latest_shared_at', 'searchable_text']);
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
  strings(value.looking_for, `${where}.looking_for`);
  for (const kind of value.looking_for) if (!REQUESTS.has(kind)) fail(`${where}.looking_for`, 'unknown request kind');
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
  if (pointer.schema_version !== 2 || !VERSION.test(pointer.version)) fail('current.json', 'unsupported schema or version');
  const prefix = `snapshots/${pointer.version}/`;
  if (pointer.snapshot !== `${prefix}manifest.json`) fail('current.json', 'manifest path does not match version');
  const manifest = readJson(root, pointer.snapshot);
  object(manifest, pointer.snapshot, ['schema_version', 'version', 'discovery', 'project_details', 'project_count', 'categories']);
  if (manifest.schema_version !== 2 || manifest.version !== pointer.version || manifest.discovery !== 'discovery.json' || manifest.project_details !== 'projects/<slug>.json') fail(pointer.snapshot, 'unsupported or mismatched manifest');
  list(manifest.categories, `${pointer.snapshot}.categories`, (value, where) => {
    object(value, where, ['slug', 'label', 'description']);
    string(value.slug, `${where}.slug`); string(value.label, `${where}.label`); string(value.description, `${where}.description`);
    if (!SLUG.test(value.slug) || !value.label.trim() || !value.description.trim()) fail(where, 'invalid category metadata');
  });
  const categorySlugs = new Set(manifest.categories.map((category) => category.slug));
  if (categorySlugs.size !== manifest.categories.length || categorySlugs.size !== 15) fail(pointer.snapshot, 'expected 15 distinct categories');
  integer(manifest.project_count, `${pointer.snapshot}.project_count`);
  const rows = readJson(root, `${prefix}discovery.json`);
  list(rows, `${prefix}discovery.json`, discovery);
  for (const row of rows) for (const category of row.categories) if (!categorySlugs.has(category)) fail(`${prefix}discovery.json`, `unknown category ${category}`);
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
    list(detail.mentions, `${relative}.mentions`, mention);
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
