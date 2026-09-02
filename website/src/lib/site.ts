/** Site-wide navigation + docs metadata. */

export const SITE_NAME = 'ITISYOU OS';
export const SITE_URL = 'https://os.itisyou.app';

export interface NavItem {
  href: string;
  label: string;
}

/** Primary header navigation (per Stitch header). */
export const NAV_PRIMARY: NavItem[] = [
  { href: '/architecture', label: 'Architecture' },
  { href: '/build', label: 'Build' },
  { href: '/roadmap', label: 'Roadmap' },
  { href: '/security', label: 'Security' },
  { href: '/docs', label: 'Docs' },
];

/** Extra routes grouped under the "Engineering" disclosure. */
export const NAV_ENGINEERING: NavItem[] = [
  { href: '/engineering', label: 'Engineering' },
  { href: '/releases', label: 'Releases' },
  { href: '/changelog', label: 'Changelog' },
  { href: '/philosophy', label: 'Philosophy' },
  { href: '/faq', label: 'FAQ' },
  { href: '/source', label: 'Source' },
];

export const NAV_ALL: NavItem[] = [...NAV_PRIMARY, ...NAV_ENGINEERING];

/** Docs collection metadata (slugs created by scripts/sync-docs.mjs). */
export interface DocMeta {
  slug: string;
  title: string;
  crumb: string;
  description: string;
  sourcePath: string;
}

export const DOCS_META: DocMeta[] = [
  {
    slug: 'architecture',
    title: 'Architecture',
    crumb: 'ARCHITECTURE',
    description:
      'What exists now — system shape, crate boundaries, boot contract, observability — and the boundaries later milestones build inside.',
    sourcePath: 'docs/ARCHITECTURE.md',
  },
  {
    slug: 'build-and-run',
    title: 'Build and Run',
    crumb: 'BUILD_AND_RUN',
    description:
      'Reproducible environment, build, and QEMU instructions for a Windows host. Every command is the exact command used in verification.',
    sourcePath: 'docs/BUILD_AND_RUN.md',
  },
  {
    slug: 'testing',
    title: 'Testing',
    crumb: 'TESTING',
    description:
      'The test pyramid: host unit tests, QEMU boot smoke, in-kernel selftests, negative cases — and the failure classification contract.',
    sourcePath: 'docs/TESTING.md',
  },
  {
    slug: 'security-model',
    title: 'Security Model',
    crumb: 'SECURITY_MODEL',
    description:
      'The central principle — AI has intelligence, not authority — trust boundaries, V0.1 concrete requirements, and the future capability model.',
    sourcePath: 'docs/SECURITY_MODEL.md',
  },
  {
    slug: 'threat-model',
    title: 'Threat Model',
    crumb: 'THREAT_MODEL',
    description:
      'Assets, adversary/failure assumptions scoped to V0.1 development reality, mitigations, and explicit non-threats.',
    sourcePath: 'docs/THREAT_MODEL.md',
  },
  {
    slug: 'known-limitations',
    title: 'Known Limitations',
    crumb: 'KNOWN_LIMITATIONS',
    description: 'The honest current state: what the kernel does not do yet, updated continuously.',
    sourcePath: 'docs/KNOWN_LIMITATIONS.md',
  },
  {
    slug: 'roadmap',
    title: 'Roadmap',
    crumb: 'ROADMAP',
    description:
      'Milestones V0.1 through V1.0, sequenced by dependency, not calendar. No fabricated dates.',
    sourcePath: 'docs/ROADMAP.md',
  },
];

export function docMeta(slug: string): DocMeta | undefined {
  return DOCS_META.find((d) => d.slug === slug);
}
