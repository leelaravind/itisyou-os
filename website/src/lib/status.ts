/**
 * Typed access to the synced project status.
 *
 * src/data/status.json is copied from ../../status/current.json by
 * scripts/sync-status.mjs (prebuild/predev). It is the ONLY source for any
 * status, version, milestone, or verification claim rendered on the site —
 * never hardcode such claims in pages (implementation plan §14.4, §24).
 */
import raw from '../data/status.json';

/** Allowed public status vocabulary (implementation plan §14.4). */
export type ModuleStatus =
  | 'concept'
  | 'planned'
  | 'research'
  | 'in-development'
  | 'implemented'
  | 'verified'
  | 'experimental'
  | 'blocked';

export interface StatusModule {
  id: string;
  name: string;
  status: ModuleStatus;
}

export interface ProjectStatus {
  version: string;
  maturity: string;
  architecture: string;
  environment: string;
  milestone: string;
  commit: string | null;
  lastVerifiedAt: string | null;
  verification: string;
  modules: StatusModule[];
}

export const status = raw as unknown as ProjectStatus;

/** Render an optional value honestly: em dash when evidence is absent. */
export function orDash(value: string | null | undefined): string {
  return value === null || value === undefined || value === '' ? '—' : value;
}

/** Human-readable labels for the status vocabulary. */
export const STATUS_LABELS: Record<string, string> = {
  concept: 'Concept',
  planned: 'Planned',
  research: 'Research',
  'in-development': 'In Development',
  implemented: 'Implemented',
  verified: 'Verified',
  experimental: 'Experimental',
  blocked: 'Blocked',
  'not-yet-verified': 'Not Yet Verified',
  'pre-alpha': 'Pre-Alpha',
};

export function moduleById(id: string): StatusModule | undefined {
  return status.modules.find((m) => m.id === id);
}
