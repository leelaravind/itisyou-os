# Session checkpoint — resumable state

**Timestamp:** 2026-09-05 Europe/London
**Repository:** `E:\Project\itisyou-os` · branch `main`
**Tags:** `v0.1.0` … `v0.7.0` (V0.8 is unreleased; no V0.8 tag yet)
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.8 in progress. The released V0.7 System Platform is immutable at `v0.7.0` and was locally and remotely verified (selftest pass=106 fail=0; platform-bios + two-boot update-recovery green; CI run 33810268090 green; staging and production website verified).

## V0.7 verified additions

- **Capabilities** (kernel-core/caps + syscall gating): default-deny bits,
  per-quantum publication, spawn inherits / spawn_caps intersects (no
  amplification). New syscalls: SYS_FS_READ=13 (sandboxed), SYS_SPAWN_CAPS=14.
- **FS sandbox**: per-process path prefixes checked on the normalized path
  (kernel-core path::is_within — traversal-proof).
- **Audit** (kernel/audit.rs): ring + [ITISYOU:AUDIT] markers for every
  privileged action + denial; `audit` command.
- **Services** (kernel/services.rs + kernel-core/service): registry with deps
  + exact caps; deterministic cycle-checked order; bounded restart (3);
  [ITISYOU:SVC] markers; `svc` command. proc gains reap() +
  run_until_pid_idle_bounded().
- **Packages/updates** (kernel/platform.rs + kernel-core/{manifest,pkg,
  sha256,update}; ITFS atomic remove): ITPKG format; store `<app>.<v>.pkg`
  + `.ok`; install/rollback/recover each ONE atomic superblock commit;
  launch re-verifies digest + grants manifest∩launcher caps under the app
  sandbox (/apps/<name> + /etc); `pkg` command. Fixtures packed by build.rs
  (incl. corrupted + hostile-manifest) under /pkgs.
- New user programs: sandbox-probe, cap-parent/child, fs-probe, echo-svc,
  crashy-svc, svc-client, hello-app.
- Harness: NVMe test disks 16 MiB (774 KB debug ELFs need the room).

## Gates / how to resume

- `scripts/test.ps1` = full matrix incl. platform-bios + update-interrupt/
  update-recovery (two boots, one persistent disk). `scripts/verify.ps1` =
  full gate.
- Shell: `run <path> [caps|-] [prefix]`, `svc`, `pkg …`, `audit`.
- Next in **V0.8**: the e1000 driver + RX/TX data path (the NIC is already
  enumerated at 00:03.0 and the parsers in `kernel_core::net` are ready and
  host-tested, so what is missing is polled RX/TX rings plus ARP/ICMP/UDP
  on top of them), then signed packages, userspace FS writes, and a
  userspace init.

## V0.8 work checkpoint

The immutable V0.7 baseline is `v0.7.0` at `00b5eea`. V0.8 is complete: every
requirement in `docs/REQUIREMENTS.md` is in a terminal state, and the two that
are NOT DONE (TCP; DHCP/IPv6/routing) say so rather than being reworded.

Delivered and verified:

1. **Capability handles are the enforcement path** — forged, expired and
   revoked handles each refused on the real syscall path with the reason
   audited (`platform-bios`).
2. **Long-running Ring 3 services** (ADR-0014) — `services-bg-bios`.
3. **Networking** (ADR-0015) — e1000 with polled RX/TX rings, ARP, IPv4, ICMP
   echo client and responder, UDP with port-scoped Ring 3 sockets, DNS.
   Verified against the runner's own INDEPENDENT host-side Ethernet peer
   (`tools/qemu-runner/src/wire.rs`) over a `dgram` netdev: `net-bios`.
4. **Signed packages** (ADR-0016) — Ed25519 in `kernel_core::ed25519`,
   RFC 8032 vectors pass; ITPKG002; three distinct authenticity refusals.
5. **Userspace filesystem writes** — `fs-write-bios` + `fs-write-persist`.
6. **APIC / I/O APIC / MSI-X** (ADR-0017) — `irq-bios`.
7. **xHCI enumeration + HID** (ADR-0018) — `xhci-hid-bios`.
8. **Persistent hash-chained audit** — `audit-persist-{write,verify,tamper}`.
9. **SMEP/SMAP/UMIP + stack guard + W^X** — `harden-bios`, and every leg now
   runs on a CPU advertising the three protections.

Gate: 24/24 QEMU legs Success, selftest pass=113 fail=0, 278 host tests,
`scripts/verify.ps1` -> `VERIFY: OK`.

## Things a resuming session will want to know

- `scripts/test.ps1` is the canonical matrix; `scripts/verify.ps1` wraps it
  with fmt, both clippy gates, the website build and the secret scan. The full
  gate takes roughly 12 minutes.
- CI (`.github/workflows/ci.yml`) duplicates the QEMU legs INLINE rather than
  calling `test.ps1`, so a new leg must be added in BOTH places.
- A `--require` string containing a double quote does not survive native
  argument quoting on Windows. Assert the quoted part separately.
- The runner's `-cpu qemu64,+smep,+smap,+umip` is load-bearing: without it the
  guest's hardening is enabled into a void.
- Editing repo files from Python: always `io.open(..., encoding='utf-8',
  newline='')`. Most files are CRLF in the working copy and LF in git, and the
  sources are full of em dashes that the default locale encoding mangles.

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu;
TEMP → G:\claude-tmp\tmp; never mix kernel + host packages in one cargo
invocation; user programs build static no-pie; kernel build.rs has
kernel-core as a build-dependency (ITPKG packing); CURRENT_CAPS/sandbox
published per quantum in run_quantum; PIC-lock and compositor-lock ISR rules
unchanged.

## Blockers

None.

## Prior release evidence (V0.7)

Release evidence points to commit `1f08bf5`, CI run `33810268090`,
staging Worker `8ab2626a-88e2-4721-9956-632339d146c2`, and production Worker
`1c065f8f-00be-46d7-ada2-7eb2238a8ebb`; the release tag is applied after the
closing evidence commit and clean-tree check.
