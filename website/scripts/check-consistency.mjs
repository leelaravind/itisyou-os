#!/usr/bin/env node
/**
 * Release-consistency gate (implementation plan §24: "make CI fail if required
 * status claims conflict with verification metadata").
 *
 * Added after the V0.8 audit found that `v0.8.0` shipped a kernel that
 * reported itself as `0.7.0-dev` and a requirement matrix carrying a state
 * ("NOT DONE") outside the allowed vocabulary. Both slipped through because
 * nothing compared them. This compares them on every build:
 *
 *   1. `status/current.json` `version` equals the Cargo workspace version, so
 *      the website, the status metadata and the running kernel can never name
 *      different versions.
 *   2. Every requirement row's state starts with an allowed token.
 *   3. A status marked `verified` carries no PLANNED row: unstarted work
 *      cannot sit inside a verified build. Future work belongs in
 *      docs/ROADMAP.md until its milestone starts.
 *   4. With `--release` (run before a version tag is created), no row may be
 *      non-terminal at all. The publication rows (CI run, website deploy) are
 *      legitimately IN PROGRESS between the verified build and the tag — they
 *      record evidence that only exists after the build is published — so the
 *      strict form is a separate, explicit step rather than every build's rule.
 */
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const problems = [];

// --- 1. version agreement ---------------------------------------------------
const cargo = readFileSync(resolve(root, 'Cargo.toml'), 'utf8');
const section = cargo.split(/^\[workspace\.package\]\s*$/m)[1] ?? '';
const cargoVersion = (section.match(/^version\s*=\s*"([^"]+)"/m) ?? [])[1];
const status = JSON.parse(readFileSync(resolve(root, 'status', 'current.json'), 'utf8'));
if (!cargoVersion) problems.push('Cargo.toml has no [workspace.package] version');
else if (status.version !== cargoVersion)
  problems.push(`status/current.json version "${status.version}" != Cargo workspace version "${cargoVersion}"`);

// --- 2 + 3. requirement states ---------------------------------------------
const TERMINAL = ['IMPLEMENTED+VERIFIED', 'BLOCKED', 'NOT APPLICABLE'];
const WORKING = ['PLANNED', 'IN PROGRESS'];
const lines = readFileSync(resolve(root, 'docs', 'REQUIREMENTS.md'), 'utf8').split(/\r?\n/);
let rows = 0;
let nonTerminal = [];
lines.forEach((line, i) => {
  if (!line.startsWith('|')) return;
  const cells = line.split('|').slice(1, -1).map((c) => c.trim());
  if (cells.length < 4) return;
  if (cells[0] === 'ID' || /^-+$/.test(cells[0].replace(/:/g, ''))) return;
  rows++;
  const state = cells[2];
  const token = [...TERMINAL, ...WORKING].find((t) => state.startsWith(t));
  if (!token) problems.push(`docs/REQUIREMENTS.md:${i + 1} ${cells[0]} has state "${state.slice(0, 40)}" outside the vocabulary`);
  else if (WORKING.includes(token)) nonTerminal.push(`${cells[0]} (${token})`);
});
if (rows === 0) problems.push('docs/REQUIREMENTS.md: no requirement rows found');
const release = process.argv.includes('--release');
const planned = nonTerminal.filter((r) => r.endsWith('(PLANNED)'));
if (status.verification === 'verified' && planned.length)
  problems.push(`status is "verified" but requirements are unstarted: ${planned.join(', ')}`);
if (release && status.verification !== 'verified')
  problems.push(`--release: status verification is "${status.verification}", not "verified"`);
if (release && nonTerminal.length)
  problems.push(`--release: requirements not terminal: ${nonTerminal.join(', ')}`);

if (problems.length) {
  console.error('[check-consistency] FAILED');
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}
console.log(
  `[check-consistency] OK${release ? ' (release)' : ''} version=${cargoVersion} ` +
    `verification=${status.verification} requirements=${rows} non-terminal=${nonTerminal.length}`,
);
