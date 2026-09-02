#!/usr/bin/env node
/**
 * Status truth sync (implementation plan §14.4 / §24).
 *
 * Copies ../status/current.json → src/data/status.json before every dev/build
 * so the site can never display status claims that outrun repository
 * evidence. All status-driven UI imports src/data/status.json only.
 *
 * The copy is validated minimally: it must parse as JSON and carry the fields
 * the UI depends on. Optional fields (commit, lastVerifiedAt) may be null —
 * the UI renders an honest "—" / "not yet verified" state for them.
 */
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = resolve(here, '..', '..', 'status', 'current.json');
const target = resolve(here, '..', 'src', 'data', 'status.json');

const ALLOWED_STATUSES = new Set([
  'concept',
  'planned',
  'research',
  'in-development',
  'implemented',
  'verified',
  'experimental',
  'blocked',
]);

const raw = readFileSync(source, 'utf8');
const status = JSON.parse(raw);

for (const field of ['version', 'maturity', 'architecture', 'environment', 'milestone', 'verification', 'modules']) {
  if (!(field in status)) {
    throw new Error(`status/current.json is missing required field "${field}"`);
  }
}
if (!Array.isArray(status.modules)) {
  throw new Error('status/current.json: "modules" must be an array');
}
for (const mod of status.modules) {
  if (typeof mod.id !== 'string' || typeof mod.name !== 'string') {
    throw new Error(`status/current.json: malformed module entry ${JSON.stringify(mod)}`);
  }
  if (!ALLOWED_STATUSES.has(mod.status)) {
    throw new Error(
      `status/current.json: module "${mod.id}" has status "${mod.status}" outside the allowed vocabulary (${[...ALLOWED_STATUSES].join(', ')})`,
    );
  }
}

mkdirSync(dirname(target), { recursive: true });
writeFileSync(target, JSON.stringify(status, null, 2) + '\n', 'utf8');
console.log(`[sync-status] ${source} -> ${target} (${status.version}, ${status.modules.length} modules)`);
