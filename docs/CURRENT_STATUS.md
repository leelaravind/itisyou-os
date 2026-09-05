# Current status — V0.8 Networking, Authenticity & Hardening

**Timestamp:** 2026-09-05 (Europe/London)
**Branch:** `main` · **Repository:** `E:\Project\itisyou-os`
**Milestone:** `V0.8 — Networking, Authenticity & Hardening`

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
