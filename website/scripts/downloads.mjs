#!/usr/bin/env node
/**
 * Release downloads: stage the exact release files into the built site, and
 * verify what a deployed site actually serves.
 *
 *   node scripts/downloads.mjs stage  <dir-with-release-files>
 *   node scripts/downloads.mjs verify <base-url>
 *
 * The manifest is src/data/downloads.json (committed). `stage` copies each file
 * into dist/downloads/<tag>/ ONLY if its size and SHA-256 match the manifest,
 * and writes SHA256SUMS.txt next to them — so a deploy can never publish bytes
 * other than the ones the release gate booted. `verify` downloads every file
 * from the deployed site and checks size, SHA-256 and headers, because a
 * deploy command succeeding says nothing about what visitors receive.
 */
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(readFileSync(resolve(here, '..', 'src', 'data', 'downloads.json'), 'utf8'));
const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');
const [mode, arg] = process.argv.slice(2);

if (!manifest.files?.length) {
  console.log('[downloads] manifest lists no files; nothing to do');
  process.exit(0);
}

if (mode === 'stage') {
  if (!arg) throw new Error('usage: downloads.mjs stage <dir>');
  const out = resolve(here, '..', 'dist', 'downloads', manifest.tag);
  if (!existsSync(resolve(here, '..', 'dist', 'index.html'))) throw new Error('dist/ is not built; run npm run verify first');
  mkdirSync(out, { recursive: true });
  const sums = [];
  for (const f of manifest.files) {
    const src = join(arg, f.file);
    const data = readFileSync(src);
    const digest = sha256(data);
    if (data.length !== f.bytes || digest !== f.sha256)
      throw new Error(`${f.file}: ${data.length} bytes sha256=${digest} does not match the manifest (${f.bytes}, ${f.sha256})`);
    copyFileSync(src, join(out, f.file));
    sums.push(`${f.sha256}  ${f.file}`);
  }
  writeFileSync(join(out, 'SHA256SUMS.txt'), sums.join('\n') + '\n');
  console.log(`[downloads] staged ${manifest.files.length} files + SHA256SUMS.txt -> ${out}`);
} else if (mode === 'verify') {
  const base = (arg ?? '').replace(/\/+$/, '');
  if (!base) throw new Error('usage: downloads.mjs verify <base-url>');
  const problems = [];
  for (const f of manifest.files) {
    const url = `${base}/downloads/${manifest.tag}/${f.file}`;
    const res = await fetch(url);
    const buf = Buffer.from(await res.arrayBuffer());
    const digest = sha256(buf);
    const type = res.headers.get('content-type');
    const disp = res.headers.get('content-disposition');
    console.log(`[downloads] ${res.status} ${url} ${buf.length} bytes sha256=${digest} type=${type} disposition=${disp}`);
    if (res.status !== 200) problems.push(`${url}: HTTP ${res.status}`);
    if (buf.length !== f.bytes) problems.push(`${url}: ${buf.length} bytes, manifest says ${f.bytes}`);
    if (digest !== f.sha256) problems.push(`${url}: sha256 ${digest}, manifest says ${f.sha256}`);
    if (!/octet-stream/.test(type ?? '')) problems.push(`${url}: content-type ${type}`);
    if (!/attachment/.test(disp ?? '')) problems.push(`${url}: not served as an attachment`);
    if (arg && process.argv[4]) {
      mkdirSync(process.argv[4], { recursive: true });
      writeFileSync(join(process.argv[4], f.file), buf);
    }
  }
  const sumsUrl = `${base}/downloads/${manifest.tag}/SHA256SUMS.txt`;
  const sums = await fetch(sumsUrl);
  const text = await sums.text();
  for (const f of manifest.files) if (!text.includes(`${f.sha256}  ${f.file}`)) problems.push(`SHA256SUMS.txt lacks ${f.file}`);
  if (problems.length) {
    console.log(`DOWNLOADS-VERIFY: FAILED`);
    for (const p of problems) console.log(`  - ${p}`);
    process.exit(1);
  }
  console.log(`DOWNLOADS-VERIFY: OK ${manifest.files.length} files + SHA256SUMS.txt served byte-exact from ${base}`);
} else {
  console.error('usage: downloads.mjs stage <dir> | verify <base-url> [save-dir]');
  process.exit(2);
}
