import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadSnapshot } from '../lib/public-snapshot.mjs';

const source = process.argv[2];
if (!source || process.argv.length !== 3) {
  console.error('Usage: npm run stage -- <completed-export-directory>');
  process.exit(2);
}

const data = fileURLToPath(new URL('../data/', import.meta.url));
const destination = path.join(data, 'public');
const temporary = path.join(data, `.public-staging-${process.pid}`);
const backup = path.join(data, `.public-previous-${process.pid}`);

function write(relative, value) {
  const file = path.join(temporary, relative);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

let movedOld = false;
try {
  const selected = loadSnapshot(path.resolve(source));
  fs.mkdirSync(data, { recursive: true });
  fs.mkdirSync(temporary);
  const prefix = `snapshots/${selected.pointer.version}/`;
  // Re-serialize only the validated public contract, never a whole source directory.
  write('current.json', selected.pointer);
  write(`${prefix}manifest.json`, selected.manifest);
  write(`${prefix}discovery.json`, selected.discovery);
  for (const [slug, detail] of selected.details) write(`${prefix}projects/${slug}.json`, detail);
  loadSnapshot(temporary);
  if (fs.existsSync(destination)) {
    fs.renameSync(destination, backup);
    movedOld = true;
  }
  fs.renameSync(temporary, destination);
  if (movedOld) fs.rmSync(backup, { recursive: true, force: true });
  console.log(`Staged public snapshot ${selected.pointer.version} (${selected.discovery.length} projects).`);
} catch (error) {
  if (movedOld && !fs.existsSync(destination)) fs.renameSync(backup, destination);
  if (fs.existsSync(temporary)) fs.rmSync(temporary, { recursive: true, force: true });
  console.error(error.message);
  process.exitCode = 1;
}
