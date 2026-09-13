# Current status — V0.9 in progress (latest release: v0.8.1)

**Timestamp:** 2026-09-13 (Europe/London)
**Branch:** `main` · **Repository:** `E:\Project\itisyou-os`
**Latest release:** `v0.8.1` (V0.8 — Networking, Authenticity & Hardening,
release-integrity closeout), live at https://os.itisyou.app with its download.
**In development:** `0.9.0-dev` — V0.9 Transport, Interrupt Cutover & Trust.

## V0.9 (in progress)

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
host tests. The release (CI, tag, new download) is pending: GitHub Actions is
refusing to start jobs on this account for a billing reason. The website
(deployed 13:36, `b0c4d05`) shows V0.9 as in development with that evidence;
the download is still `v0.8.1`.

## V0.10 (started on a branch)

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
