# ITISYOU OS - End-to-End Autonomous Implementation Plan

**Document status:** Execution specification  
**Initial release target:** `0.1.0-dev` / Kernel Foundation  
**Primary architecture:** x86_64  
**Initial execution environment:** QEMU only  
**Public project website:** `https://os.itisyou.app`  
**Repository visibility:** Private by default unless the owner explicitly changes it  
**Local project root:** `E:\Project\itisyou-os` (or a collision-safe sibling under `E:\Project\`)  
**Bulk/temp path:** `G:\claude-tmp` where appropriate  
**Never intentionally use for project work:** `C:`, `D:`, `F:`

---

## 1. Purpose and authority of this document

This document is the authoritative implementation specification for the first independently bootable ITISYOU OS prototype and its public engineering website. Claude Code is expected to execute it end-to-end with the user's Agent Operating Rules applied in full.

The execution objective is not to produce a Linux distribution or a themed desktop. The first technical target is an independently bootable x86_64 kernel that can be compiled reproducibly, launched in QEMU, observed through serial output, tested automatically, and incrementally extended without Linux kernel code in the runtime OS image.

The website is a separate deliverable. It must explain truthfully what exists, what is being implemented, what is only planned, the architecture and security philosophy, development evidence, roadmap, and long-term research direction. It must be deployed to `os.itisyou.app` using the already-authorized GitHub and Cloudflare access available to Claude Code, provided those connections verify successfully.

The project must optimize for correctness, recoverability, reproducibility, evidence, and safe autonomy - not for pretending a complete daily-driver OS can be produced in one session.

---

## 2. Autonomous execution contract

Claude Code must treat the following as execution requirements:

1. Read `AGENT_OPERATING_RULES.md` before modifying the repository. Copy the user's supplied rules file into the repository unchanged if it is not already present.
2. Create `CLAUDE.md` that references the operating rules and this implementation plan as mandatory project governance.
3. Convert this plan into a requirement traceability matrix before implementation.
4. Ask no non-essential questions once execution begins. Resolve safe, reversible decisions from the repository, tooling, tests, documentation, or defaults.
5. Work continuously until a verified terminal state is reached, a genuine safety/authority blocker is hit, or the execution environment ends.
6. Preserve progress frequently in Git using logical checkpoint commits.
7. Never write project data, package caches, build output, downloads, or temp data intentionally to `C:`. Reconfigure `TEMP`, `TMP`, Cargo, Rustup, npm/pnpm stores, QEMU artifacts, and build caches to `E:` or `G:\claude-tmp` where controllable.
8. Never modify the machine's real bootloader, EFI system partition, disk partition table, Windows boot configuration, Secure Boot configuration, firmware settings, or physical disks as part of V0.1.
9. Do not boot the kernel directly on physical hardware in this phase. QEMU is the mandatory execution boundary.
10. Do not expose secrets. Use environment variables, Wrangler/GitHub secret stores, and platform-native secret mechanisms.
11. Secret-scan before every push.
12. Never claim success from code existence alone. Compile, boot, exercise, observe, and verify.
13. Never make a test green by skipping/deleting it, weakening assertions, swallowing failures, or replacing real behavior with hard-coded success.
14. If one workstream blocks, continue independent workstreams such as documentation, website, tests, CI, architecture, or tooling.
15. Before session exhaustion, preserve a resumable checkpoint containing branch, commit, completed requirements, verification evidence, current failure, exact next command, and remaining work.

### Required terminal states for every requirement

Every requirement must end in exactly one state:

- `IMPLEMENTED + VERIFIED`
- `BLOCKED` with reason, evidence, impact, and exact required input
- `NOT APPLICABLE` with reason

There is no generic `DONE` state.

---

## 3. Product vision

ITISYOU OS is a long-term research and engineering project exploring what a personal operating system could look like if designed today around:

- explicit privilege and authority boundaries;
- strong kernel/user isolation;
- capability-oriented security;
- recoverable and auditable system changes;
- deterministic privileged services;
- local-first intelligence;
- AI that can reason about the system but does not possess unrestricted kernel authority;
- reproducible configuration and builds;
- transparent resource ownership;
- minimal trusted computing base where practical;
- automatic diagnosis and verification;
- safe rollback and recovery;
- modern hardware assumptions rather than universal legacy compatibility from day one.

The project must not market itself as "better than Linux" in general. Linux is a mature, broad, production-proven kernel ecosystem. ITISYOU OS should instead document specific hypotheses it wants to test and measurable areas it may eventually improve for a constrained personal-computing target.

---

## 4. V0.1 objective

### 4.1 Hard target

At the end of the V0.1 implementation effort, the repository should contain an independently bootable x86_64 kernel image that:

- boots in QEMU through a modern boot path;
- enters 64-bit kernel code;
- initializes an early serial console;
- emits deterministic boot-stage logs;
- identifies basic CPU/platform information;
- initializes physical memory management;
- establishes controlled virtual-memory/paging abstractions;
- provides a kernel heap allocator;
- installs exception/interrupt infrastructure;
- initializes a timer source suitable for prototype scheduling;
- supports kernel tasks/threads with a primitive scheduler;
- provides keyboard input where feasible in the chosen QEMU device model;
- mounts a primitive in-memory/initramfs filesystem through a VFS abstraction;
- exposes an interactive shell or deterministic command executor;
- implements diagnostic commands;
- has a defined syscall/user-mode path as either implemented+verified or explicitly isolated as a stretch milestone;
- has automated boot and regression testing;
- fails with useful panic/serial diagnostics;
- is reproducibly buildable from documented commands;
- contains no dependency on the Linux kernel at runtime.

### 4.2 Stretch target

Attempt only after the hard target is stable:

- Ring 3 transition;
- minimal userspace process;
- syscall entry/exit;
- minimal ELF64 loader;
- user program printing through a syscall;
- PCI enumeration;
- ACPI table discovery/read-only parsing;
- basic framebuffer output beyond text diagnostics.

Stretch goals must never destabilize or falsify the hard target. If a stretch feature cannot be verified, keep it out of the release-critical path and mark it accurately.

### 4.3 Explicit V0.1 non-goals

Do not attempt to claim complete support for:

- Wi-Fi;
- Bluetooth;
- accelerated Intel Iris Xe graphics;
- full USB stack;
- webcam;
- audio;
- laptop suspend/hibernate;
- production-grade NVMe/AHCI storage;
- a complete TCP/IP stack;
- browser;
- desktop compositor;
- GUI application ecosystem;
- Linux binary compatibility;
- production package manager;
- physical laptop installation;
- production-grade Secure Boot chain.

These belong to later milestones.

---

## 5. Engineering principles

### 5.1 Safety by isolation

V0.1 must execute in QEMU. Disk images used in tests must be project-generated disposable images, never pass-through host disks.

### 5.2 Intelligence is not authority

Future AI agents must not execute privileged kernel operations directly. The intended architecture is:

`Agent intent -> Policy engine -> Risk/permission evaluation -> User approval where required -> Deterministic privileged service -> Kernel operation -> Verification -> Audit -> Rollback/recovery`

The kernel API and future privileged-service API should be designed so this separation remains possible.

### 5.3 Small trusted core

Keep kernel responsibilities explicit. Prefer clear subsystem boundaries and narrow interfaces over hidden global state.

### 5.4 Deterministic observability

Every boot stage must be observable through serial output even if the framebuffer or keyboard path fails.

### 5.5 Failure is data

Kernel panic output, QEMU exit code, timeout, exception vector, failing test assertion, and serial tail must be captured as structured evidence wherever possible.

### 5.6 Reproducibility

A fresh authorized developer/agent should be able to reconstruct the environment using versioned documentation and scripts, subject only to downloading dependencies and obtaining service credentials.

---

## 6. Recommended technical baseline

Claude Code must verify actual compatibility in the environment before locking versions. The default design is:

- **Kernel language:** Rust `no_std`, with minimal x86_64 assembly only when architecture entry/exit requires it.
- **Target:** `x86_64-unknown-none` or a project-specific bare-metal target JSON if required by the chosen boot approach.
- **Boot:** Prefer a maintained boot protocol/toolchain that supports x86_64 and memory-map/framebuffer handoff. Limine is a suitable default if verified compatible; otherwise use a documented equivalent without weakening requirements.
- **Emulator:** QEMU x86_64.
- **Firmware:** UEFI via OVMF where practical; retain a documented fallback only if required.
- **Build orchestration:** Cargo plus project scripts/Makefile/justfile chosen to remain straightforward on Windows.
- **Testing:** Rust unit tests for pure logic; host-side tests for parsers/algorithms; QEMU serial integration tests; deterministic timeout and exit signaling.
- **Website:** modern static/edge-friendly web stack suitable for Cloudflare. Prefer the existing ITISYOU ecosystem conventions if a nearby project provides a proven baseline; otherwise use a minimal TypeScript framework compatible with Cloudflare deployment.
- **CI:** GitHub Actions for formatting, lint/type checks, unit tests, kernel build, QEMU boot smoke test when runner virtualization/tooling permits, website build, accessibility/static checks, secret scan, and deployment gating.

Do not add dependencies simply because they are convenient. Record why each kernel dependency exists, its license, maintenance status, and trusted-code implications.

---

## 7. Repository strategy

### 7.1 Repository creation

If no existing repository represents this project:

1. Create `E:\Project\itisyou-os`.
2. Initialize Git.
3. Create a private GitHub repository, preferred slug `itisyou-os`, under the authenticated owner unless an existing naming convention dictates a safer collision-free name.
4. Add origin.
5. Create the initial protected/recoverable branch strategy supported by the account.
6. Push the foundation commit after secret scan.

If a repository already exists, inspect and reuse it rather than creating a duplicate.

### 7.2 Proposed monorepo structure

```text
itisyou-os/
├─ AGENT_OPERATING_RULES.md
├─ CLAUDE.md
├─ README.md
├─ LICENSE                 # only after license decision is supported/authorized
├─ SECURITY.md
├─ CONTRIBUTING.md
├─ .gitignore
├─ .gitattributes
├─ rust-toolchain.toml
├─ Cargo.toml
├─ Cargo.lock
├─ kernel/
│  ├─ Cargo.toml
│  └─ src/
│     ├─ main.rs
│     ├─ arch/x86_64/
│     ├─ boot/
│     ├─ cpu/
│     ├─ memory/
│     ├─ interrupts/
│     ├─ time/
│     ├─ task/
│     ├─ syscall/
│     ├─ fs/
│     ├─ device/
│     ├─ console/
│     ├─ security/
│     ├─ diagnostics/
│     └─ panic.rs
├─ crates/
│  ├─ abi/
│  ├─ boot-protocol/
│  ├─ kernel-types/
│  └─ test-protocol/
├─ user/
│  ├─ init/
│  └─ programs/            # stretch phase
├─ initramfs/
│  ├─ manifest/
│  └─ root/
├─ tools/
│  ├─ image-builder/
│  ├─ qemu-runner/
│  ├─ serial-test-runner/
│  └─ status-exporter/
├─ scripts/
│  ├─ bootstrap.ps1
│  ├─ doctor.ps1
│  ├─ build.ps1
│  ├─ run-qemu.ps1
│  ├─ test.ps1
│  ├─ verify.ps1
│  ├─ secret-scan.ps1
│  ├─ package-image.ps1
│  └─ export-status.ps1
├─ tests/
│  ├─ boot/
│  ├─ kernel/
│  ├─ integration/
│  └─ fixtures/
├─ website/
│  ├─ app-or-src/
│  ├─ public/
│  ├─ content/
│  ├─ tests/
│  └─ deployment-config/
├─ status/
│  ├─ current.json
│  ├─ milestones.json
│  └─ verification.json
├─ docs/
│  ├─ ARCHITECTURE.md
│  ├─ IMPLEMENTATION_PLAN.md
│  ├─ REQUIREMENTS.md
│  ├─ THREAT_MODEL.md
│  ├─ SECURITY_MODEL.md
│  ├─ TESTING.md
│  ├─ BUILD_AND_RUN.md
│  ├─ DEPLOYMENT.md
│  ├─ RECOVERY.md
│  ├─ KNOWN_LIMITATIONS.md
│  ├─ ROADMAP.md
│  ├─ DEVELOPMENT_STORY.md
│  ├─ SESSION_CHECKPOINT.md
│  └─ adr/
└─ .github/
   └─ workflows/
```

Use the exact structure only where it fits the selected build tooling; preserve the conceptual boundaries.

---

## 8. Environment bootstrap

Create `scripts/doctor.ps1` and `scripts/bootstrap.ps1`. The doctor must inspect before installing.

### 8.1 Verify

- PowerShell version;
- Git and authenticated GitHub CLI;
- Rustup and Rust toolchain;
- Cargo;
- LLVM/binutils needs if any;
- QEMU installation and executable path;
- OVMF firmware availability if used;
- bootloader/image tooling;
- Node.js and package manager for website;
- Wrangler and authenticated Cloudflare account;
- browser automation tooling for website verification;
- free disk space on `E:` and `G:`;
- `TEMP`/`TMP` and major tool caches are not unintentionally targeting `C:` for large project operations.

### 8.2 Bootstrap behavior

- Install only missing required dependencies using safe, standard mechanisms.
- Never make broad system upgrades.
- Never delete user data to make space.
- Redirect caches and temporary build locations to `E:` or `G:\claude-tmp` where supported.
- Record exact tool versions into a generated, non-secret diagnostics file.
- Run smoke checks after each installed dependency.
- If a dependency requires a user-only privileged action that cannot be automated safely, mark only that subtask blocked and continue all independent work.

---

## 9. Boot architecture

### 9.1 Boot contract

The bootloader must hand the kernel enough information to establish a controlled early environment, including where supported:

- memory map;
- framebuffer information;
- kernel image/base data;
- ACPI RSDP pointer;
- command line/configuration;
- initramfs/module pointers.

Define an internal `BootInfo` abstraction so the rest of the kernel is not tightly coupled to a third-party bootloader's raw structures.

### 9.2 Boot stages

Assign stable boot stage identifiers:

- `B000` firmware/loader handoff observed
- `B010` kernel entry
- `B020` early serial ready
- `B030` CPU baseline established
- `B040` boot memory map validated
- `B050` physical memory manager ready
- `B060` virtual memory abstraction ready
- `B070` heap ready
- `B080` descriptor/exception layer ready
- `B090` interrupt controller/timer baseline ready
- `B100` scheduler initialized
- `B110` VFS/initramfs initialized
- `B120` input path initialized
- `B130` shell/init task running
- `B140` userspace transition ready (stretch)
- `B150` V0.1 boot acceptance reached

Each stage must emit a serial event with a stable machine-parseable prefix plus human-readable message.

### 9.3 QEMU exit signaling

Implement a test-only mechanism for deterministic pass/fail exit from QEMU, gated so production-like interactive runs are not forced to exit. The serial test harness must distinguish:

- success exit;
- explicit test failure;
- kernel panic;
- triple fault/reboot;
- QEMU launch failure;
- timeout/hang.

---

## 10. Kernel subsystem plan

### 10.1 Console and logging

Implement serial first. Framebuffer/text console is secondary.

Requirements:

- log levels appropriate for early kernel use;
- no heap dependency for earliest logging;
- structured stage markers;
- panic-safe output path;
- ring buffer only after memory is stable;
- avoid logging secrets or arbitrary user memory contents.

### 10.2 CPU and descriptor setup

Implement/document:

- CPUID feature collection;
- GDT/TSS as required;
- IDT;
- exception handlers with vector/error-code reporting;
- double-fault handling on a dedicated stack if practical;
- page-fault diagnostics including fault address and access type without unsafe recursive failure.

Do not enable CPU features without checking support.

### 10.3 Physical memory manager

Start with a simple allocator suitable for correctness and introspection, e.g. bitmap or frame-list based.

Requirements:

- parse boot memory map;
- reserve kernel image, boot structures, initramfs, framebuffer, and firmware-reserved ranges;
- reject overlapping/invalid regions;
- frame allocation/freeing;
- allocation accounting;
- double-free protection where reasonably testable;
- invariants and debug diagnostics.

### 10.4 Virtual memory

Requirements:

- explicit page-table abstractions;
- map/unmap operations;
- flags for present/writable/user/NX where available;
- avoid accidental writable+executable mappings where architecture/tooling permits;
- TLB invalidation rules documented;
- guard pages for sensitive stacks where feasible;
- tests for alignment, range overflow, duplicate mappings, and invalid unmap.

### 10.5 Kernel heap

Start with a proven simple allocator design appropriate for V0.1; correctness over sophistication.

Requirements:

- initialize only after paging/frame allocator is ready;
- alignment guarantees;
- allocation failure handling;
- no silent null-style corruption;
- heap statistics;
- stress tests for small/large/aligned allocations where host-side or QEMU testing is feasible.

### 10.6 Interrupts and timer

Requirements:

- exception handling first;
- select PIC/APIC strategy appropriate to initial QEMU target and document the choice;
- timer interrupt;
- stable tick accounting;
- interrupt-safe shared-state design;
- no blocking allocator use in unsafe interrupt contexts unless explicitly designed for it.

### 10.7 Scheduler and tasks

V0.1 can use a simple round-robin cooperative/preemptive prototype, but its invariants must be explicit.

Requirements:

- task IDs;
- task states;
- task stacks with guard strategy where possible;
- context switch implementation;
- idle task;
- yield path;
- timer-driven preemption if stable enough;
- task list diagnostics;
- no task can silently vanish without state transition/accounting;
- scheduler tests for state transitions and runnable queue behavior.

### 10.8 Synchronization

Implement only primitives needed by V0.1:

- interrupt masking guards where appropriate;
- spin mutex or equivalent with documented constraints;
- avoid pretending SMP safety before SMP is actually supported;
- lock-order rules if multiple locks arise;
- debug assertions for misuse when practical.

### 10.9 VFS and initramfs

Create a small VFS abstraction separating path lookup and file operations from the initial in-memory backend.

Minimum behavior:

- root mount;
- directory listing;
- file read;
- metadata/type query;
- normalized absolute paths;
- reject path traversal semantics that escape root abstraction;
- deterministic initramfs build;
- seed files such as `/etc/version`, `/docs/welcome.txt`, or equivalent generated from project metadata.

Persistent disk writes are not required in V0.1.

### 10.10 Input

For QEMU, implement the simplest reliable keyboard/input path compatible with the selected virtual machine model. If interrupt-driven keyboard support becomes a disproportionate blocker, a serial-command input mode may serve as the verified automation path while keyboard support is marked separately.

### 10.11 Shell

Minimum commands:

- `help`
- `version`
- `system`
- `cpu`
- `memory`
- `tasks`
- `uptime`
- `ls [path]`
- `cat <path>`
- `clear`
- `echo`
- `panic-test` only in explicit development/test mode
- `reboot` where QEMU platform support exists
- `shutdown` or deterministic QEMU exit where supported

Shell requirements:

- bounded command input;
- robust whitespace parsing;
- useful errors for unknown commands/missing arguments;
- no unchecked buffer operations;
- automated command-output tests through serial input where feasible.

### 10.12 Userspace and syscall stretch phase

Only after the kernel baseline is stable:

- define a versioned ABI crate/interface;
- Ring 3 code/data selectors or equivalent architecture setup;
- user stack;
- syscall entry mechanism suitable for x86_64;
- validate all user pointers before kernel dereference;
- no direct user mapping of kernel-only pages;
- initial syscalls such as `write`, `exit`, `yield`, possibly `get_time`;
- minimal ELF64 loader with strict validation;
- malformed ELF tests;
- user program prints a deterministic line, exits, and cannot directly access kernel memory.

---

## 11. Security architecture from day one

### 11.1 Trust boundaries

Document at minimum:

1. firmware/bootloader boundary;
2. kernel trusted boundary;
3. userspace boundary;
4. privileged system-service boundary (future);
5. untrusted application boundary (future);
6. AI/agent boundary (future);
7. external network boundary (future).

### 11.2 V0.1 security requirements

- Rust `unsafe` usage must be minimized, localized, documented with invariants, and reviewable.
- Create an `UNSAFE_INVENTORY.md` or generated equivalent listing each important unsafe module/block and its safety contract.
- Enable NX/no-execute and page permission separation where the boot/architecture path permits it.
- Validate boot-provided pointers/ranges before broad use where feasible.
- Do not trust initramfs metadata blindly; validate format and bounds.
- Panic on violated kernel invariants rather than continuing with known corruption.
- No network services in V0.1.
- No automatic host filesystem sharing into the guest.
- QEMU test launch must not expose host disks or privileged devices.
- Dependency and secret scans before release/push.
- Threat model must be updated as new privileged features appear.

### 11.3 Future capability model

Reserve architecture/documentation for:

`subject -> capability -> object -> permitted operation -> constraints -> provenance`

Capabilities should eventually be explicit, inspectable, revocable, and narrower than blanket administrator privileges.

---

## 12. Automated verification system

### 12.1 One-command verification

`scripts/verify.ps1` must become the canonical local gate. It should run all applicable checks and return non-zero on failure.

Target sequence:

1. environment doctor subset;
2. formatting;
3. lints/static checks;
4. host/unit tests;
5. kernel build;
6. image/initramfs build;
7. QEMU boot smoke test;
8. QEMU kernel integration tests;
9. shell command tests where implemented;
10. website type/lint tests;
11. website unit/component tests;
12. website production build;
13. browser E2E/accessibility tests where environment permits;
14. secret scan;
15. requirement/status export consistency checks.

### 12.2 QEMU test harness

Create a host runner that:

- launches QEMU with explicit arguments;
- captures stdout/stderr/serial to artifact files outside `C:`;
- applies a bounded timeout;
- searches for stable machine-readable boot/test events;
- records last N serial lines on failure;
- captures QEMU exit reason/code;
- writes a machine-readable test result JSON;
- never interprets absence of output as success.

### 12.3 Kernel tests

At minimum create tests for:

- memory-map normalization;
- reserved-range exclusion;
- frame allocator exhaustion and reuse;
- alignment/overflow helpers;
- mapping/unmapping validation;
- heap allocation invariants;
- scheduler state transitions;
- VFS path normalization;
- initramfs malformed input;
- shell parser boundaries;
- boot smoke success marker;
- intentional panic detection;
- exception diagnostics where safely inducible;
- userspace/ELF validation if stretch milestone lands.

### 12.4 Regression discipline

Every bug fixed during the session should receive a regression test whenever reasonably testable. Tests must prove the previous failure mode rather than simply exercising nearby code.

### 12.5 Evidence retention

Store small, useful verification evidence under a controlled `artifacts/` or CI artifact system, not permanent massive logs in Git. Commit summarized evidence/status JSON and documentation, not bulky transient QEMU disks/logs unless explicitly justified.

---

## 13. Build reproducibility and release artifacts

The build pipeline should be capable of producing:

- kernel binary/ELF;
- bootable disk/ISO image appropriate to selected boot path;
- initramfs archive;
- build manifest;
- Git commit SHA;
- compiler/toolchain versions;
- checksums;
- test status summary;
- release notes.

Development artifacts must be clearly marked `EXPERIMENTAL` / `PRE-ALPHA`.

No release page or README should instruct users to install V0.1 on an important physical machine.

---

## 14. Website implementation - os.itisyou.app

### 14.1 Purpose

The website is the public technical record and project interface. It must communicate:

- what ITISYOU OS is;
- what it is not;
- current milestone and maturity;
- current build status based on real repository metadata where feasible;
- architecture;
- boot sequence;
- security/AI-authority model;
- roadmap;
- engineering process;
- verification evidence;
- development story;
- releases/changelog;
- known limitations;
- future research direction;
- GitHub/source link subject to repository visibility/access policy.

### 14.2 Stitch design ingestion

Use the Stitch design created from the approved master Stitch prompt as the canonical visual reference.

Claude Code must:

1. inspect the exported Stitch assets/code/screens available through the configured design integration;
2. preserve the visual system rather than recreating a generic approximation;
3. extract reusable tokens: typography, spacing, radii, borders, surfaces, status semantics, motion rules;
4. implement responsive behavior for 1440, 1280, 768, and ~390 widths;
5. preserve accessibility and reduced-motion behavior;
6. optimize heavy effects; do not ship huge unnecessary assets merely because Stitch generated them;
7. record any deliberate design deviations and why.

If Stitch access is unavailable, do not block the OS work. Implement the information architecture and a restrained baseline UI, mark the exact design-integration portion blocked, and keep the components ready for visual replacement.

### 14.3 Required routes

At minimum:

- `/`
- `/architecture`
- `/build`
- `/roadmap`
- `/security`
- `/engineering`
- `/docs`
- `/docs/[slug]` or equivalent
- `/releases`
- `/changelog`
- `/philosophy`
- `/faq`
- custom `404`

Routes may be combined only if the design deliberately uses sections and deep links while preserving all required information.

### 14.4 Status truth model

Use machine-readable files under `status/` as the source of truth where practical:

`status/current.json` should contain fields such as:

```json
{
  "version": "0.1.0-dev",
  "maturity": "pre-alpha",
  "architecture": "x86_64",
  "environment": "QEMU",
  "milestone": "Kernel Foundation",
  "commit": "<generated>",
  "lastVerifiedAt": "<generated-or-null>",
  "verification": "not-yet-verified",
  "modules": []
}
```

A status must be derived from evidence, not marketing intent.

Allowed public statuses:

- Concept
- Planned
- Research
- In Development
- Implemented
- Verified
- Experimental
- Blocked

`Implemented` and `Verified` are not interchangeable.

### 14.5 Required homepage content

- ITISYOU OS identity;
- "Building an operating system from the kernel upward" positioning;
- explicit `Experimental / Pre-alpha` marker;
- current milestone;
- live/current build card backed by status metadata;
- layered architecture visual;
- current V0.1 modules;
- boot-sequence visualization;
- rationale for building a new OS without disparaging Linux;
- security principle: "AI has intelligence, not authority";
- development verification loop;
- roadmap;
- latest development-story entries;
- long-term research direction;
- physical-hardware warning;
- GitHub/docs CTA;
- footer with project state.

### 14.6 Website accessibility

Target WCAG 2.2 AA for implemented UI:

- semantic landmarks/headings;
- keyboard navigation;
- visible focus;
- contrast;
- meaningful link text;
- diagram text alternatives or accessible descriptions;
- minimum touch targets;
- reduced-motion support;
- no status encoded by color alone;
- responsive overflow handling;
- accessible code blocks and tables.

### 14.7 Website performance/security

- avoid unnecessary client JavaScript;
- optimize fonts/assets;
- no giant video backgrounds;
- set a strong CSP appropriate to actual asset sources;
- security headers;
- no unnecessary third-party analytics during initial build;
- no collection of personal data unless later explicitly designed and documented;
- avoid exposing build secrets or internal CI metadata beyond intentionally public fields.

---

## 15. Cloudflare deployment

### 15.1 Discovery first

Claude Code must verify:

- authenticated Cloudflare account;
- existing `itisyou.app` zone;
- whether `os.itisyou.app` DNS/route already exists;
- whether an appropriate Pages/Workers project already exists;
- current ITISYOU deployment conventions that can be safely reused.

Do not overwrite unrelated routes/projects.

### 15.2 Deployment model

Prefer the simplest Cloudflare-native static/edge deployment supported by the chosen website framework and current account configuration.

Infrastructure/configuration must be committed where the platform permits it. Avoid undocumented dashboard-only state.

### 15.3 Staging and production

If a staging convention exists, use:

`local -> staging -> production`

If no staging hostname exists and creating one is safe/free, a temporary preview deployment can serve as staging. Do not create billable infrastructure without authorization.

### 15.4 Domain

Final production target:

`https://os.itisyou.app`

Verify:

- DNS resolution;
- TLS;
- expected HTTP status;
- canonical redirect behavior;
- core routes;
- security headers;
- no console/network errors on primary pages;
- desktop/mobile rendering;
- robots/sitemap/meta tags as appropriate for public indexing;
- social metadata.

Never report the site as live merely because a deployment command returned success; actually request and browser-test the production hostname.

---

## 16. GitHub and CI/CD

### 16.1 Git safety

Before each meaningful work segment:

- inspect branch;
- inspect working tree;
- preserve unrelated changes;
- use checkpoint commits;
- review staged diff;
- secret scan;
- push only task-related changes.

No force push, destructive reset, history rewrite, or silent deletion of existing user work.

### 16.2 Suggested workflows

`ci.yml`

- format check;
- Rust lint/static checks where compatible with bare-metal code;
- host/unit tests;
- kernel build;
- QEMU smoke/integration test if runner setup supports it;
- website checks/build;
- secret scan;
- generated status schema validation.

`website-deploy.yml` or Cloudflare-native Git deployment

- only after CI gate;
- deploy preview/staging;
- smoke verify;
- production deployment based on repository strategy and available Cloudflare integration.

`release.yml` later

- build reproducible artifacts;
- checksums;
- test evidence;
- release metadata;
- never label experimental images stable.

### 16.3 Branch protection

Enable sensible protections if account/repository capabilities allow without requiring a paid upgrade. Do not purchase/enable paid plans.

---

## 17. Documentation set

Maintain throughout development, not only at the end.

### Required documents

**README.md**  
Project purpose, current reality, quick start, safety warning, links.

**docs/ARCHITECTURE.md**  
Kernel boundaries, boot path, subsystem diagrams, future userspace/services/AI model.

**docs/IMPLEMENTATION_PLAN.md**  
Repository copy of this plan, updated only by explicit amendments while preserving original requirements.

**docs/REQUIREMENTS.md**  
Stable requirement IDs, status, implementation location, verification evidence.

**docs/THREAT_MODEL.md**  
Assets, trust boundaries, attacker assumptions, V0.1 risks, future risk expansion.

**docs/SECURITY_MODEL.md**  
Privilege model, unsafe-code policy, memory permissions, future capability model.

**docs/TESTING.md**  
Test pyramid, QEMU harness, commands, failure classification.

**docs/BUILD_AND_RUN.md**  
Exact reproducible environment/build/QEMU commands.

**docs/DEPLOYMENT.md**  
Website Cloudflare topology, domain, config, rollback.

**docs/RECOVERY.md**  
Git recovery, failed deployment rollback, corrupted build artifacts, CI diagnosis.

**docs/KNOWN_LIMITATIONS.md**  
Truthful current limitations.

**docs/ROADMAP.md**  
Milestones with no fabricated dates.

**docs/DEVELOPMENT_STORY.md**  
Continuously record design decisions, failures, root causes, fixes, test evidence, milestones, and lessons learned.

**docs/SESSION_CHECKPOINT.md**  
Machine-resumable current state, updated before session limit or any forced stop.

**docs/adr/**  
Architecture Decision Records for material choices.

---

## 18. Development story requirements

For each material milestone or failure, append:

- timestamp/date;
- branch/commit;
- objective;
- change made;
- why;
- alternatives considered where meaningful;
- failure/error encountered;
- root cause;
- fix;
- verification command;
- verification result;
- residual risk;
- next objective.

Do not reconstruct this only at the end; update it as implementation proceeds.

---

## 19. Requirement traceability baseline

Create stable IDs at minimum as follows.

| ID | Requirement | Acceptance evidence |
|---|---|---|
| GOV-001 | Agent rules loaded and referenced | Files present; CLAUDE.md reference |
| GOV-002 | No intentional project writes to C:/D:/F: | Doctor/config evidence |
| GIT-001 | Recoverable private repo created/reused | Remote/visibility verified |
| GIT-002 | Secret scan before pushes | Scan logs/CI |
| ENV-001 | Toolchain verified | doctor report |
| KRN-001 | x86_64 independent kernel builds | build command + artifact |
| BOOT-001 | QEMU reaches kernel entry | serial marker B010 |
| BOOT-002 | Serial logging works | serial output B020 |
| MEM-001 | Physical memory manager works | unit/QEMU tests |
| MEM-002 | Virtual memory abstraction works | mapping tests |
| MEM-003 | Kernel heap works | allocation tests |
| INT-001 | Exceptions/IDT installed | controlled exception test |
| TIM-001 | Timer/ticks work | monotonic tick evidence |
| TASK-001 | Scheduler runs multiple tasks | deterministic task output/tests |
| FS-001 | VFS/initramfs mounts | ls/cat integration test |
| SH-001 | Shell commands execute robustly | serial interaction tests |
| DIAG-001 | Panic produces useful serial evidence | intentional panic test |
| TEST-001 | One-command verification gate exists | verify.ps1 result |
| TEST-002 | QEMU timeout/failure classification works | negative tests |
| SEC-001 | Unsafe inventory exists | document/generated report |
| SEC-002 | No host disk passthrough | QEMU config review/test |
| DOC-001 | Required docs maintained | file review |
| WEB-001 | Website implements approved design system | browser/visual review |
| WEB-002 | Status is evidence-backed | generated status validation |
| WEB-003 | Responsive/accessibility gate | automated/browser evidence |
| CF-001 | os.itisyou.app deployed | production browser verification |
| CI-001 | CI checks repo on push/PR | GitHub Actions result |
| REL-001 | Build manifest/checksums produced | artifact inspection |
| USR-001 | Ring 3/userspace hello works | stretch; QEMU proof |
| ABI-001 | Syscall ABI defined/tested | stretch; tests/docs |

Add granular child requirements as implementation discovers necessary components. Never remove an original requirement merely because it is difficult.

---

## 20. Execution phases

### Phase 0 - Intake, governance, and discovery

1. Read operating rules.
2. Inspect `E:\Project` for collision/existing repo.
3. Inspect GitHub for existing repo.
4. Inspect Cloudflare for existing `os.itisyou.app` resources.
5. Verify Stitch/design access if configured.
6. Verify disk space and cache/temp routing.
7. Create requirement matrix.
8. Create initial architecture/ADR skeleton.
9. Establish Git checkpoint.

**Gate:** safe workspace + verified access + explicit requirement matrix.

### Phase 1 - Repository and build skeleton

1. Initialize monorepo.
2. Configure Rust toolchain/target.
3. Establish kernel crate and boot image flow.
4. Establish scripts for doctor/build/run/test.
5. Boot a minimal kernel to serial in QEMU.
6. Add CI skeleton.
7. Commit/push.

**Gate:** reproducible QEMU boot marker from clean build.

### Phase 2 - Kernel foundation

Implement in dependency order:

1. boot abstraction;
2. serial/logging;
3. CPU/GDT/TSS/IDT;
4. memory map normalization;
5. physical memory allocator;
6. paging abstraction;
7. kernel heap;
8. exception diagnostics;
9. interrupt/timer baseline;
10. task scheduler;
11. VFS/initramfs;
12. input/shell;
13. diagnostics/status export.

After every meaningful subsystem: add tests, run targeted test, run applicable regression, checkpoint commit.

**Gate:** all V0.1 hard-target boot stages reach B150 automatically.

### Phase 3 - Negative/adversarial verification

Test intentionally:

- malformed memory map fixture;
- allocator exhaustion;
- duplicate mapping;
- invalid VFS paths;
- unknown shell command;
- overlong shell input;
- intentional panic;
- boot timeout;
- corrupted initramfs fixture;
- repeated scheduler yields/context switches;
- missing optional framebuffer;
- controlled exception path.

Fix root causes and add regression coverage.

**Gate:** no unresolved release-critical failure.

### Phase 4 - Website implementation

Can run in parallel with kernel work once architecture/status schema is stable.

1. ingest Stitch design;
2. establish design tokens/components;
3. implement routes;
4. connect status JSON/content;
5. create architecture/boot/security diagrams;
6. add documentation content;
7. add development story/changelog/release surfaces;
8. test desktop/mobile/accessibility/reduced-motion;
9. optimize performance and security headers;
10. production build.

**Gate:** local production build and E2E pass.

### Phase 5 - Cloudflare deployment

1. inspect existing resources;
2. provision/reuse only safe required resources;
3. deploy preview/staging;
4. browser test;
5. deploy production;
6. verify `https://os.itisyou.app` and routes;
7. verify TLS/security headers/meta;
8. record deployment topology and rollback.

**Gate:** production hostname verified, not merely deployment command success.

### Phase 6 - Userspace stretch

Only if hard target remains green:

1. ABI crate;
2. Ring 3 transition;
3. user stack/process model;
4. syscall mechanism;
5. pointer validation;
6. minimal ELF loader;
7. userspace hello/test program;
8. fault isolation tests.

**Gate:** deterministic userspace output and protection tests.

### Phase 7 - Final integration and evidence

1. run full `verify.ps1` from clean/reproducible state;
2. review requirement matrix against original request;
3. run secret scan;
4. inspect staged/outgoing diff;
5. update all docs;
6. export status metadata from actual verification;
7. ensure website reflects evidence;
8. push final checkpoint;
9. verify CI;
10. verify production site;
11. write final report and session checkpoint.

---

## 21. Autonomous orchestration strategy

Claude Code should parallelize independent work only when the environment supports safe file ownership separation.

Suggested ownership:

- **Main agent:** architecture, requirements, integration, kernel-critical code, final verification.
- **Kernel subagent:** one isolated subsystem at a time with explicit files and tests.
- **Test subagent:** QEMU runner, fixtures, negative cases; no overlapping kernel edits unless coordinated.
- **Website subagent:** `website/` only after status schema contract is frozen.
- **Docs/review subagent:** documentation consistency and requirement evidence, read-mostly unless assigned specific docs.

The main agent must inspect diffs and rerun important tests; subagent claims are not verification.

---

## 22. Failure handling loop

For every material failure:

1. capture the actual error with secrets redacted;
2. classify: environment, compiler, linker, boot, exception, hang, logic, test harness, website, Cloudflare, GitHub;
3. reproduce with the smallest deterministic command;
4. identify root cause from code/config/logs/runtime state;
5. make the smallest correct fix;
6. add/update a regression test where feasible;
7. rerun targeted test;
8. rerun affected regression set;
9. rerun full gate when the subsystem becomes stable;
10. record the incident in development story if material.

Do not endlessly repeat the same failing command without changing the diagnostic hypothesis.

---

## 23. Session-limit resilience

The project must be resumable without conversation memory.

Update `docs/SESSION_CHECKPOINT.md` after major phases and before a known session/usage limit with:

- timestamp;
- repository path;
- remote;
- branch;
- HEAD commit;
- clean/dirty working-tree status;
- current milestone;
- completed requirement IDs;
- blocked requirement IDs;
- latest successful verification command/result;
- exact current failing command and error summary if any;
- changed-but-uncommitted files if unavoidable;
- next three actions in exact order;
- relevant artifact/log paths;
- deployed URLs;
- infrastructure state;
- no secrets.

Commit/push this checkpoint whenever safe before interruption.

---

## 24. Website content truth policy

The public website must never outrun the engineering evidence.

Examples:

- A source file existing for a scheduler -> `Implemented` only if it compiles/integrates.
- Scheduler integration test passing -> may become `Verified` for the tested scope.
- A roadmap idea with no code -> `Planned` or `Concept`.
- A partially functioning Ring 3 path -> `Experimental` or `In Development`, not `Verified`.
- A failed module -> expose `Blocked` only if public disclosure is useful and safe; otherwise keep accurate internal status and use a neutral public description.

Generate status from repository evidence where practical, and make CI fail if required status claims conflict with verification metadata.

---

## 25. Definition of Done - V0.1

V0.1 is complete only when all applicable items below are satisfied:

### Kernel

- clean reproducible build;
- QEMU boot reaches defined V0.1 acceptance marker;
- serial diagnostics available throughout boot;
- physical memory allocator verified;
- paging/virtual memory abstraction verified;
- heap verified;
- exceptions/interrupt baseline verified;
- timer verified;
- multiple kernel tasks verified;
- VFS/initramfs verified;
- shell/command path verified;
- panic/failure path verified;
- no host physical disk passthrough;
- runtime does not rely on Linux kernel code.

### Testing

- canonical one-command verification gate;
- no skipped/disabled tests used to gain green status;
- negative boot/hang/panic cases tested;
- regression tests for material bugs;
- CI runs applicable gates;
- secret scan clean.

### Documentation

- architecture current;
- build/run instructions tested;
- threat/security model current;
- known limitations explicit;
- roadmap current;
- development story current;
- unsafe inventory current;
- requirement traceability reconciled.

### Website

- Stitch-derived design implemented or exact design access blocker documented;
- all required information present;
- current status evidence-backed;
- desktop/mobile verified;
- accessibility checks applied;
- reduced motion verified;
- production build clean;
- no fake metrics/results;
- physical hardware warning visible.

### GitHub/Cloudflare

- repository created/reused and pushed;
- recoverable commit history;
- CI result verified;
- `os.itisyou.app` live if Cloudflare access permits;
- TLS/routes/browser journey verified;
- deployment config documented;
- no unauthorized paid resource enabled.

### Final report

Must state:

- exact repository path;
- GitHub repository URL;
- branch and final commit;
- production website URL;
- what was implemented;
- exact tests/checks run and results;
- kernel boot evidence summary;
- website deployment evidence;
- requirement states;
- skipped/stretch items;
- blockers;
- recurring costs/resources, if any;
- exact next milestone.

---

## 26. Post-V0.1 roadmap

### V0.2 - Userspace Foundation

- stable process abstraction;
- Ring 3 isolation;
- ELF loading;
- syscall ABI;
- IPC foundations;
- user programs;
- stronger memory protection.

### V0.3 - Storage

- PCI enumeration maturity;
- block device abstraction;
- AHCI/NVMe research/driver work;
- persistent filesystem strategy;
- crash-consistency and recovery experiments.

### V0.4 - Networking

- NIC driver target chosen from actual hardware/VM needs;
- Ethernet;
- ARP/NDP as appropriate;
- IPv4/IPv6 foundations;
- UDP/TCP;
- DNS;
- strict network permission model.

### V0.5 - Graphics/Desktop Foundations

- framebuffer/display abstraction;
- input subsystem;
- compositor;
- windowing primitives;
- text rendering;
- basic GUI toolkit;
- accessibility architecture.

### V0.6 - Hardware Expansion

- USB;
- audio;
- hardware discovery/device manager;
- ACPI/power foundations;
- laptop hardware research;
- targeted driver strategy.

### V0.7 - System Platform

- service manager;
- package/runtime model;
- signed/atomic updates;
- sandboxing;
- capability/permission engine;
- recovery and rollback.

### V0.8 - AI-Native System Layer

- local inference service outside kernel;
- system knowledge/RAG over approved local state;
- diagnostic agent;
- policy-controlled system actions;
- preview/approval;
- provenance/audit;
- post-action verification and rollback.

### V0.9 - Daily-Driver Research

- Wi-Fi;
- Bluetooth;
- accelerated graphics strategy;
- power management;
- suspend/resume;
- application ecosystem/compatibility;
- real hardware qualification on explicitly approved sacrificial/test hardware only.

### V1.0 - Experimental Personal OS

Only declare when reliability, security, recovery, hardware support, installation safety, update safety, and daily-driver acceptance criteria are explicitly defined and passed. Version number alone must not imply maturity.

---

## 27. Rules for physical-hardware progression

Moving from QEMU to real hardware is a separate future approval gate.

Before any real-hardware boot attempt, require:

- explicit user authorization for the exact machine/device;
- tested recovery media;
- complete backup verification;
- no installation/write operation initially - removable-media read-only/ephemeral boot preferred;
- hardware inventory;
- known-safe storage driver behavior;
- watchdog/recovery strategy;
- no automatic bootloader replacement;
- explicit rollback path;
- QEMU regression suite green;
- dedicated real-hardware checklist.

V0.1 does not cross this gate.

---

## 28. First autonomous command sequence - intent, not blind copy

Claude Code should translate the following into commands appropriate to the verified local environment:

```text
1. Read AGENT_OPERATING_RULES.md.
2. Inspect E:\Project and identify/create itisyou-os safely.
3. Check free space on E: and G:.
4. Redirect project caches/temp away from C: where configurable.
5. Inspect git/gh/cloudflare/stitch/toolchain connectivity.
6. Create/reuse private GitHub repo.
7. Create governance + requirement docs.
8. Bootstrap the minimal Rust/QEMU toolchain.
9. Produce the smallest serial-printing bootable kernel.
10. Add deterministic QEMU boot test.
11. Commit + secret scan + push checkpoint.
12. Grow kernel subsystem-by-subsystem, testing each.
13. In parallel, implement website from Stitch once design assets are accessible.
14. Deploy website to Cloudflare preview/staging, verify, then production.
15. Run full verification, reconcile requirements, update public status, push final checkpoint.
```

Never substitute "command completed" for verification of its resulting state.

---

## 29. Final execution directive for Claude Code

Execute this project autonomously to the maximum safe, verifiable extent available in the current session. Do not stop after planning, scaffolding, or a minimal hello-world if additional requirements can be implemented and verified. Preserve the V0.1 hard target over speculative stretch work. Keep the website truthful and deploy it independently even if a kernel stretch feature blocks. Use QEMU as the safety boundary. Do not modify the physical machine's boot chain or disks. Maintain requirement traceability, development story, tests, Git checkpoints, secret hygiene, and session checkpoint continuously. Finish with evidence, not claims.
