# Current status — V0.11 in development (latest release v0.10.0)

**Timestamp:** 2026-09-14 (Europe/London)
**Branch:** `main` (release) · `v0.11/work` (development) · **Repository:** `E:\Project\itisyou-os`
**Latest release:** `v0.10.0` — V0.10 Userspace System. Release commit
`c936bc7` (CI run `34782271250`, all jobs green, images reproduced
bit-for-bit by the run's two clean builds); download live at
https://os.itisyou.app/download (UEFI
`f98f5cbe89dc295699f75231bc71766eb6c129a60baa41d028f5ac0f315da792`, BIOS
`d8416a58f367d835a6755662d8d9322b0ba042d7362179030f0a0ea24f44f098`). The
earlier releases `v0.9.0` and `v0.8.1` stay downloadable.
**Now:** V0.11 — AI-Native System Layer, on branch `v0.11/work` (not yet
released).

## V0.11 (in development)

Every V0.11 row in `docs/REQUIREMENTS.md` is IMPLEMENTED+VERIFIED on
`v0.11/work` with QEMU and host evidence, each with a negative control.

- **Audit trail, reviewed first.** Three defects present since v0.8.0 were
  fixed before any agent code (AUDIT11-001, AUDIT11-002, SEC11-001) and
  disclosed for v0.10.0 on the site.
- **The AI-native layer (ADR-0024):**
  - a pinned diagnostic model trained at build time from synthetic scenarios
    (MODEL11-001). Its accuracy equals a one-rule baseline, which is reported
    as such;
  - a console-only read-only system view (VIEW11-001);
  - `inferd` running the model in Ring 3 under init (INFER11-001);
  - runbooks (KB11-001);
  - an agent with no authority (AGENT11-001);
  - kernel-checked proposals (PROP11-001);
  - console-only approval with kernel execution, verification and rollback
    for `resume-scheduler` (ACT11-001) and for `retry-service` through a
    kernel→init mailbox (ACT11-002).
- **Terminal escapes.** Ring 3 output can no longer drive the operator's
  terminal (OUT11-001).
- **The adversarial review.** Six independent reviewers covered the whole
  V0.11 diff, and every finding was verified by a second reviewer. Nine
  findings were confirmed, and each is now fixed with a leg or selftest and a
  control:
  - two console-evidence defects in v0.10.0, disclosed for it: a marker split
    across output chunks (OUT11-002), and a program-chosen path printed by
    `ps` (SEC11-002);
  - a crafted audit trail that crashed recovery, and a false "the next save
    heals it" claim (AUDIT11-003);
  - a rollback kill that trusted init's pid, and a fetched-but-unacknowledged
    retry (ACT11-002);
  - proposal expiry, and bidi characters in the kernel's echo (PROP11-001,
    AUDIT11-002);
  - two tests weaker than their claims (PROP11-001).

Limitations are in `docs/KNOWN_LIMITATIONS.md`: synthetic training, three
conditions and two actions, unauthenticated IPC, and proposals that are not
persisted. **Next:** the v0.11.0 release, which needs CI (another short
public window).

## V0.9 (released as v0.9.0)

Verified so far (QEMU + host evidence in `docs/REQUIREMENTS.md` § V0.9): ACPI
discovery; the I/O APIC cutover with the 8259 PIC retired, including the PIT
mode-3 double-delivery bug the cutover exposed; ACPI S5 power-off; a DHCP
client (and a V0.8 broadcast-UDP checksum bug it exposed); audit anchoring
against an off-disk witness, shown against a whole-trail forgery; IPv6
foundations (link-local, SLAAC, NDP, ICMPv6 — client paths against QEMU's
router, responder paths against the harness's own peer); TCP streams from
Ring 3 against the host operating system's own TCP stack, including recovery
from injected loss by retransmission (and two bugs that work exposed: a kernel
stack overflow corrupting the capability table, and a V0.8 socket leak on the
console's `run` exit path); and the package-signing key hierarchy — an
offline root, certified signing keys limited by name scope and release-epoch
window, a signed revocation list (ADR-0021); Ring 3 TCP listen; and guard
pages under the kernel's static stacks (HARD09-001). Every V0.9 feature is
verified locally — last full gate `7367b89`: VERIFY OK, 36/36 QEMU legs, 366
host tests. Released as v0.9.0: release commit `f06673e`, CI run
`34760629701` green (all four jobs), tag on the stamp commit `997dbb4` (CI
`34761869462` green); production `1ba155b6` serves the v0.9.0 images and keeps
v0.8.1 downloadable.

## V0.10 (released as v0.10.0)

Released 2026-09-13: release commit `c936bc7` on `main` (fast-forwarded from
`v0.10/integration`), CI run `34782271250` green, images boot-tested byte for
byte, production `239a2140` serving them. Evidence: `docs/REQUIREMENTS.md`
§ V0.10 (REG-V010, CI-V010, WEB10-001).

Update 2026-09-13 evening: every V0.10 roadmap item is implemented and
verified on `v0.10/integration` (last full gate `11e9fb5`: VERIFY OK, 50/50
QEMU legs, 456 host tests; each step also shown against a negative control):
always-on co-scheduling at audited safe points (SCHED10-001/002), a process
tree with parent-only `wait`, `wait_nohang`, `sleep`, orphan handling, `ps`
and `kill` (PROC10-002), `svc_report` (SVC10-001), `/sbin/init` as pid 1
from `/etc/init.conf`, restarted by the kernel (INIT10-001/002/003), a Ring 3
shell with the console's input (SHELL10-001), and Ring 3 desktop apps with
click-to-focus and routed input (DESK10-001). Found and fixed on the way:
a denial of service in every release so far — the kernel wrote through user
pages the program could not write (SEC10-001, disclosed for v0.9.0 on the
site) — plus a PS/2 framing bug, an IRQ-unsafe input queue, and a
`gui_present` that checked existence instead of ownership (INPUT10-001,
GUI10-001). The release waits for its CI run (GitHub Actions needs the
billing fix or another short public window).

Update 2026-09-13 16:28: `main` green at `42399b9` (CI `34765042925`); site
production `169d50f9`; repository private. Signing keys on the owner's offline
USB drive.

Update 2026-09-13 16:05: `v0.10/integration` now carries `main` (v0.9.0) and
version `0.10.0-dev`; draft PR #2 ran GitHub CI `34764042157` — all four jobs
green on Linux. Not merged: V0.10 still lacks userspace init, the always-on
scheduler, the Ring 3 shell and desktop applications.

Two V0.10 items are implemented and verified on branch `v0.10/integration`
(kept off `main` so `main` stays the V0.9 release candidate; merge after the
`v0.9.0` tag): program arguments for Ring 3 programs (PROC10-001, syscalls 35
`args` and 36 `spawn_args`, `run … -- args`) and ITFS space reclamation
(FS10-001, gap allocator with no format change). Evidence in that branch's
`docs/REQUIREMENTS.md` § V0.10 and `docs/SESSION_CHECKPOINT.md` here.

## V0.8.1 (release)

## 2026-09-13 audit and V0.8.1

- The full gate was re-run on the `v0.8.0` commit before any document was
  trusted: `VERIFY: OK`, 24/24 QEMU legs, selftest 113/0 on BIOS and UEFI,
  292 host tests (278 in `kernel-core`), website 0 errors, secret scan clean.
- The release around it disagreed with itself: the tagged kernel reported
  `0.7.0-dev` (the shell-test legs asserted the same stale string, so the drift
  was tested in), NET08-003/004 were "NOT DONE" (not a plan state), and the live
  site and several docs still made V0.1-era claims. V0.8.1 corrects these, adds a
  build gate (`website/scripts/check-consistency.mjs`) that fails on version or
  requirement-state drift, adds real-browser verification
  (`website/scripts/browser-verify.mjs`), inventories the V0.8 `unsafe` code
  (rows 33–39) and forbids `unsafe` in `kernel-core`. No kernel behaviour
  changes beyond the version it reports.
- The roadmap after V0.8 was amended (recorded in `docs/ROADMAP.md`): V0.9
  Transport, Interrupt Cutover & Trust → V0.10 Userspace System (new) → V0.11
  AI-Native System Layer → V1.0; daily-driver hardware research moves after
  V1.0.
- **First public download.** https://os.itisyou.app/download serves the
  CI-built boot images (`itisyou-os-0.8.1-x86_64-uefi.img`,
  `…-bios.img`) with SHA-256, sizes, tested environments, QEMU instructions,
  a physical-hardware warning and known limitations. The images are
  bit-reproducible (GPT GUIDs normalized), were booted byte for byte before
  publication, and are unchanged by testing (`snapshot=on`).

### v0.8.1 release evidence

- Local gate `scripts/verify.ps1` → `VERIFY: OK` (24/24 QEMU legs, selftest
  113/0 BIOS+UEFI, 297 host tests, website + consistency gate, secret scan).
- CI run `34749240177` green for `397ab80`; `status/current.json` stamped to it.
  The stamp commit `8af917c` failed CI at the format check (a late runner edit)
  and was fixed in the next commit; the release tag sits on a green commit.
- Release candidate 1 images: uefi `eb7d566c7fcafd9a4ee672b0ede9b3c3456b9577b799698fd982d2ff72c45571`,
  bios `4a84705bec3c8a2ee0d80cceeb0b5b1562cf237936dc686035ff6f6494f410ea`. The UEFI
  file was then found not to be reproducible from a clean build (embedded
  loader link time) and is superseded by the final release images below;
  `RELEASE-BOOT-TEST: OK` on the downloaded bytes (QEMU 11.1.0, Windows 11) and
  the full matrix on the same bytes in CI (QEMU 8.2.2, ubuntu-24.04).
- Final release images (CI run `34750316003`, commit `bf32b53`, reproducible
  across two clean checkouts): uefi
  `93750593a7b56d149e1b90e023d111a8aac4fea401fa3a7ce6249b78212cbd88`, bios
  `4a84705bec3c8a2ee0d80cceeb0b5b1562cf237936dc686035ff6f6494f410ea`.
- Final deployment: staging `5ad339cd-bd0b-4752-862b-7dcc79a37c0f` → production
  `5436979f-5e4c-42ea-af02-32dbcf5b364a`: `BROWSER-VERIFY: OK` and
  `DOWNLOADS-VERIFY: OK` on both, and the files re-downloaded from production
  pass `RELEASE-BOOT-TEST: OK`. (Release-candidate deployment: staging
  `14f484fa…`, production `2d6c739d…`.)

## V0.8 (v0.8.0) summary

- Everything from V0.1–V0.7 remains green, now with SMEP/SMAP/UMIP enabled on
  every leg: **24/24 QEMU legs Success**, selftest **113** / fail **0**, 278
  host tests, `scripts/verify.ps1` → `VERIFY: OK`.
- V0.8 delivered and verified:
  - **Networking** — an e1000 driver with polled RX/TX descriptor rings; ARP,
    IPv4, ICMP echo (client *and* responder), UDP with Ring 3 sockets behind a
    capability scoped to a port, and a DNS resolver. Verified against the test
    harness's own **independent** host-side Ethernet peer, which also sends
    five hostile frames the guest must refuse and answer nothing.
  - **Package authenticity** — Ed25519 (RFC 8032 vectors pass) over a context
    string, the declared lengths and the content digest, against a compiled-in
    trust root. Unsigned, foreign and forged packages are three distinct
    refusals; a corrupt one is still refused earlier, on integrity.
  - **Userspace filesystem writes** — behind the Filesystem WRITE right *and*
    the process sandbox, with an overwrite that is one crash-atomic commit;
    proven to survive a real reboot through the Ring 3 path.
  - **Interrupt modernization** — local APIC enabled and proved to deliver; the
    I/O APIC programmed with a verified register round-trip but left masked;
    MSI-X delivered by a real NVMe block read's completion.
  - **xHCI** — a second, structurally different USB host controller: full
    enumeration and HID input, tagged `src=xhci`.
  - **Persistent audit** — records hash-chained, extended before the bounded
    ring drops anything, continued across boots, and reported `TAMPERED` when
    the stored trail is edited.
  - **Hardening** — SMEP, SMAP and UMIP, alongside the existing W^X, stack
    guard and user-pointer validation.
### Release evidence

- Local canonical gate: `scripts/verify.ps1` → `VERIFY: OK` (doctor, format,
  both clippy gates, 278 host tests, 24/24 QEMU legs, website build, secret
  scan clean over 275 files).
- Remote: CI run `33987853808` green on ubuntu-24.04 for commit `e6e3496`.
- `status/current.json` stamped to that commit; staging verified before
  production. Staging version `edaab8e1-37e3-4c3a-a13b-398494ab988c`, production version `a3544b0a-5fe1-44ab-8dfc-b2bd4a3dda0d`.
- Live: https://os.itisyou.app — 16/16 routes 200, `/nope` 404, strict CSP,
  `nosniff`, `X-Frame-Options: DENY`, TLS 200, zero client JS, rendering
  `v0.8.0 · Current Build VERIFIED · COMMIT e6e3496d129f5f9d753ddd90b875bccafbe5cefe`.
- Browser automation was unavailable in this session (the Chrome extension was
  not connected), so the site was verified over HTTP against the rendered
  markup rather than through a browser. The site ships zero client-side
  JavaScript, so the rendered HTML is the whole of what a browser would show —
  but the limitation is recorded rather than glossed.

- Deliberately **not** delivered, recorded as NOT DONE rather than reworded:
  **TCP**, IPv6, DHCP, and routing beyond a single gateway. Line-based IRQs
  still run on the legacy PIC.

### Known boundary notes

- The package trust root is a **development** key whose seed is a literal in
  `kernel/build.rs`. Published deliberately — a build-time key that looked
  secret would invite someone to trust it — but it means this build
  authenticates against a key anyone can use. No rotation, revocation or
  expiry.
- The audit chain detects a record being altered, deleted, reordered or
  inserted. It does not defend against an attacker who rewrites the whole file
  including its stored head.
- The I/O APIC's entries stay masked; this is not yet a system that could run
  without the PIC.
- xHCI handles one device, one interrupt endpoint, one slot; no hubs, no
  runtime attach/detach.
- ITFS still has no free-block reuse: `remove` and overwrite leak the old
  extent by design, trading space for crash-atomic simplicity.
