import * as esbuild from 'esbuild';
import { mkdir, writeFile, readFile, readdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = path.dirname(fileURLToPath(import.meta.url));
const outdir = path.join(root, 'dist');
await mkdir(outdir, { recursive: true });
const manifestPlugin = { name: 'asset-manifest', setup(build) { build.onEnd(async result => {
  if (result.errors.length) return;
  const manifest = {};
  for (const [file, output] of Object.entries(result.metafile.outputs)) {
    if (output.entryPoint?.endsWith('main.ts')) manifest.script = path.basename(file);
    if (output.entryPoint?.endsWith('global.css')) manifest.style = path.basename(file);
  }
  const icon = await readFile(path.join(root, 'src/icon.svg'));
  manifest.icon = `icon-${createHash('sha256').update(icon).digest('hex').slice(0, 12)}.svg`;
  await writeFile(path.join(outdir, manifest.icon), icon);
  // Written last: Cargo never embeds an incomplete build.
  await writeFile(path.join(outdir, 'manifest.json'), JSON.stringify(manifest));
}); }};
const context = await esbuild.context({ absWorkingDir: root, entryPoints: ['src/main.ts', 'src/global.css'], bundle: true, minify: true, format: 'esm', target: 'es2022', outdir, entryNames: '[name]-[hash]', metafile: true, plugins: [manifestPlugin] });
if (process.argv.includes('--watch')) { await context.watch(); console.log('Watching wrkgn-web assets'); }
else { await context.rebuild(); await context.dispose(); }
