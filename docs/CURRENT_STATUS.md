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
