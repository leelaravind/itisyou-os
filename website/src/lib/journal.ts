/**
 * Development-story entries — content from docs/DEVELOPMENT_STORY.md
 * (continuously updated engineering log; decisions, failures, root causes,
 * fixes). Only entries that exist in the repo log appear here; nothing is
 * invented. Newest first for display.
 */
export interface JournalEntry {
  date: string;
  time: string;
  session: string;
  title: string;
  what: string;
  detail: string;
  /** Real evidence line (file paths / observed errors), or null when none. */
  evidence: string | null;
  /** Module-style status for the tag (allowed vocabulary). */
  status: 'in-development' | 'implemented' | 'blocked';
}

export const JOURNAL: JournalEntry[] = [
  {
    date: '2026-09-02',
    time: '02:30',
    session: 'Session 1 — Foundation',
    title: 'Kernel skeleton and test harness authored',
    what:
      'Workspace laid down: kernel/ (lib + interactive and selftest binaries), crates/kernel-core (host-testable stage/marker contract), tools/image-builder (pure-Rust BIOS/UEFI images + SHA-256 manifest), tools/qemu-runner (deterministic boot assertion harness with JSON evidence).',
    detail:
      'First build failed: this nightly\u2019s cargo rejects `cargo-features = ["bindeps"]` in the manifest. Root cause: artifact dependencies must now be enabled via `[unstable] bindeps = true` in .cargo/config.toml. Fixed there; `per-package-target` remains a manifest feature. Rebuild launched.',
    evidence: 'error: unknown Cargo.toml feature `bindeps` — resolved in .cargo/config.toml',
    status: 'in-development',
  },
  {
    date: '2026-09-02',
    time: '02:20',
    session: 'Session 1 — Foundation',
    title: 'Governance intake and discovery',
    what:
      'Read AGENT_OPERATING_RULES.md and the end-to-end implementation plan; both adopted as binding. Requirement traceability matrix created (docs/REQUIREMENTS.md). Stitch design export preserved unchanged in design/ and extracted as the visual source of truth.',
    detail:
      'Environment discovery and storage rules applied: Rust nightly-2026-09-01 + x86_64-unknown-none toolchain installed to E:\\toolchains, QEMU 11.1.0 extracted to E:\\tools\\qemu without elevation, scratch routed to G:\\claude-tmp. GitHub authenticated; repository to be created private.',
    evidence: 'docs/REQUIREMENTS.md · docs/adr/0001..0003 · design/stitch/ (13 screens + DESIGN.md)',
    status: 'in-development',
  },
];
