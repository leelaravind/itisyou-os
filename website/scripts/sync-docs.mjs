#!/usr/bin/env node
/**
 * Docs sync — copies the canonical repository documentation into the Astro
 * content collection so /docs/[slug] renders the actual repo markdown at
 * build time (single source of truth: ../docs/*.md, never a website fork).
 */
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const docsDir = resolve(here, '..', '..', 'docs');
const targetDir = resolve(here, '..', 'src', 'content', 'docs');

/** repo file -> collection slug */
const DOCS = {
  'ARCHITECTURE.md': 'architecture',
  'BUILD_AND_RUN.md': 'build-and-run',
  'TESTING.md': 'testing',
  'SECURITY_MODEL.md': 'security-model',
  'THREAT_MODEL.md': 'threat-model',
  'KNOWN_LIMITATIONS.md': 'known-limitations',
  'ROADMAP.md': 'roadmap',
};

mkdirSync(targetDir, { recursive: true });
for (const [file, slug] of Object.entries(DOCS)) {
  const body = readFileSync(resolve(docsDir, file), 'utf8');
  writeFileSync(resolve(targetDir, `${slug}.md`), body, 'utf8');
}
console.log(`[sync-docs] copied ${Object.keys(DOCS).length} repo docs -> ${targetDir}`);
