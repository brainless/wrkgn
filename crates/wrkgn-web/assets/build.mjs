import * as esbuild from 'esbuild';
import { mkdir, writeFile, readFile, readdir, access } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = path.dirname(fileURLToPath(import.meta.url));
const outdir = path.join(root, 'dist');
await mkdir(outdir, { recursive: true });
// Self-hosted variable fonts (Latin and latin-ext subsets, normal style, weight axis only) from @fontsource-variable.
// Licenses (OFL) are kept in fonts/LICENSE-*.txt and are neither copied to dist nor served.
const fonts = [
  { key: 'font_dm_sans', family: 'DM Sans', pkg: '@fontsource-variable/dm-sans', slug: 'dm-sans', weights: '400 700' },
  { key: 'font_source_serif_4', family: 'Source Serif 4', pkg: '@fontsource-variable/source-serif-4', slug: 'source-serif-4', weights: '400 700' },
];
const fontFiles = {};
let fontFaceCss = '';
for (const font of fonts) {
  const pkgDir = path.join(root, 'node_modules', font.pkg);
  const packageCss = await readFile(path.join(pkgDir, 'wght.css'), 'utf8');
  // latin-ext first so latin wins where ranges overlap; browsers fetch each file only when its range is needed.
  for (const subset of ['latin-ext', 'latin']) {
    const label = `${font.slug}-${subset}-wght-normal`;
    // unicode-range is read from the package's own CSS, never hand-written.
    const range = new RegExp(`/\\* ${label} \\*/[^}]*?unicode-range:\\s*([^;]+);`).exec(packageCss)?.[1];
    if (!range) throw new Error(`unicode-range for ${label} not found in ${font.pkg}/wght.css`);
    const bytes = await readFile(path.join(pkgDir, 'files', `${label}.woff2`));
    const key = subset === 'latin' ? font.key : `${font.key}_latin_ext`;
    const name = `${key.replaceAll('_', '-')}-${createHash('sha256').update(bytes).digest('hex').slice(0, 12)}.woff2`;
    await writeFile(path.join(outdir, name), bytes);
    fontFiles[key] = name;
    fontFaceCss += `@font-face{font-family:'${font.family}';font-style:normal;font-weight:${font.weights};font-display:swap;src:url(/assets/${name}) format('woff2');unicode-range:${range}}\n`;
  }
}
// Prepends the hashed @font-face rules so the hashed CSS bundle changes whenever a font does.
const fontFacePlugin = { name: 'font-face', setup(build) { build.onLoad({ filter: /src[\\/]global\.css$/ }, async args => ({ contents: fontFaceCss + await readFile(args.path, 'utf8'), loader: 'css' })); } };
const manifestPlugin = { name: 'asset-manifest', setup(build) { build.onEnd(async result => {
  if (result.errors.length) return;
  const manifest = { ...fontFiles };
  for (const [file, output] of Object.entries(result.metafile.outputs)) {
    if (output.entryPoint?.endsWith('main.ts')) manifest.script = path.basename(file);
    if (output.entryPoint?.endsWith('global.css')) manifest.style = path.basename(file);
  }
  const icon = await readFile(path.join(root, 'src/icon.svg'));
  manifest.icon = `icon-${createHash('sha256').update(icon).digest('hex').slice(0, 12)}.svg`;
  await writeFile(path.join(outdir, manifest.icon), icon);
  // Optional social preview image; emitted only when the source file exists.
  const ogPath = path.join(root, 'src/og-image.png');
  if (await access(ogPath).then(() => true, () => false)) {
    const og = await readFile(ogPath);
    manifest.og_image = `og-image-${createHash('sha256').update(og).digest('hex').slice(0, 12)}.png`;
    await writeFile(path.join(outdir, manifest.og_image), og);
  }
  // Written last: Cargo never embeds an incomplete build.
  await writeFile(path.join(outdir, 'manifest.json'), JSON.stringify(manifest));
}); }};
const context = await esbuild.context({ absWorkingDir: root, entryPoints: ['src/main.ts', 'src/global.css'], bundle: true, minify: true, format: 'esm', target: 'es2022', outdir, entryNames: '[name]-[hash]', metafile: true, external: ['/assets/*'], plugins: [fontFacePlugin, manifestPlugin] });
if (process.argv.includes('--watch')) { await context.watch(); console.log('Watching wrkgn-web assets'); }
else { await context.rebuild(); await context.dispose(); }
