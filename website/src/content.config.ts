import { defineCollection, z } from 'astro:content';
import { glob } from 'astro/loaders';

/**
 * Repository documentation, copied verbatim from ../docs by
 * scripts/sync-docs.mjs before every dev/build. The repo markdown carries no
 * frontmatter; page titles/descriptions live in src/lib/docs.ts.
 */
const docs = defineCollection({
  loader: glob({ pattern: '*.md', base: './src/content/docs' }),
  schema: z.object({}).catchall(z.unknown()),
});

export const collections = { docs };
