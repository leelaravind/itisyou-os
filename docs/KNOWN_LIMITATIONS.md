# Known limitations — honest current state

Updated continuously; last full revision: 2026-09-13 (V0.8.1 release-integrity
closeout). Each item says what is true of the current build, not what was true
when the subsystem first landed. Items that later milestones address name the
milestone; the sequence lives in `docs/ROADMAP.md`.

## Execution environment

- **QEMU is the only supported and tested execution environment.** Physical
  hardware boot is intentionally out of scope and untested; nothing here should
  be written to a real disk.
- QEMU machine model: `pc` (i440FX), 256 MiB RAM, CPU `qemu64,+smep,+smap,+umip`.
  Other machine models are untested.
- Single CPU only; no SMP. Synchronization assumes one core; spinlocks are
  interrupt-safe by masking, not SMP-safe.
- Host tooling scripts are Windows PowerShell (`scripts/*.ps1`); CI provides the
  Linux path.
- The toolchain is pinned to nightly-2026-08-01 because newer nightlies break
  the bootloader crate's UEFI stage (rust-osdev/bootloader#579).

## Boot

- The `itisyou-fs-persist` binary's **BIOS** disk image does not boot (a
  bootloader BIOS-stage quirk specific to that binary; its ELF is valid and its
  UEFI image boots). Its two-boot persistence test runs on UEFI. Root cause not
  isolated. Every other image boots on both BIOS and UEFI.

## Kernel, processes, scheduling

- Scheduling is **preemptive round-robin** with a fixed 20 ms quantum and no
  priorities or CPU accounting. Registers and address-space isolation are
  preserved across preemption (machine-verified).
- Processes run when the kernel console drives the scheduler: at the idle
  prompt, during `run`/`bg`, and inside the supervisor. There is **no userspace
  `init`** yet — the console is a kernel component, and a long-running
  foreground command (`desktop`, a long `run`) starves the background services
  for its duration. (V0.10: userspace init and an always-on scheduler.)
- Process model: spawn/wait/exit with a single blocking waiter per child; no
  program arguments, no fork/exec, no process groups, no signals.
- IPC: bounded kernel message channels (4 channels, ≤256 B, ≤8 queued),
  non-blocking, addressed by integer id. No shared-memory or synchronous
  rendezvous IPC.
- The kernel heap is a fixed **32 MiB** range; no growth. Physical memory above
  4 GiB is ignored by the frame allocator (`ignored_high_frames`).
- Syscalls run with interrupts masked (`SFMASK` clears IF); blocking waits inside
  a syscall are therefore forbidden and the network/storage paths are
  non-blocking there. CR3 switches do a full TLB flush (no PCID).

## Security model

- Capabilities are **resource-scoped handles** with owner binding, bounded
  delegation, revocation and expiry, enforced on the syscall path (V0.8): every
  gated call revalidates the caller's handle in the kernel table. The V0.7
  bitmask survives only as the *request* vocabulary (manifests, `spawn_caps`).
  The capability kinds are fixed at build time and there is no user-facing
  policy editor.
- The filesystem sandbox is a per-process set of path prefixes, checked on the
  normalized path for reads and, separately from the write right, for writes.
- **Package trust is a key hierarchy** (V0.9): the kernel trusts only an
  offline root whose private key is outside the source tree, and signing keys
  only through root-signed certificates with a name scope and a validity
  window in release epochs, subject to a root-signed revocation list. The
  image's fixture packages are signed by a PUBLISHED test key — deliberately,
  so anyone can reproduce the image — whose certificate covers only names
  starting `hello-`. Limits: rotation is an offline operation plus a new
  image (no over-the-network trust updates); validity is by release epoch, not
  date, because there is no trusted clock; the boot image itself is not
  authenticated, so whoever can rewrite it can replace the root with the
  kernel; and there is no persistent anti-rollback floor for revocation lists
  across boots. No real package is signed by the release key yet.
- The audit trail is hash-chained and persistent: it detects a record being
  altered, deleted, reordered or inserted. On its own it does **not** detect an
  attacker who rewrites the whole file — the chain is unkeyed, and a valid empty
  trail verifies. Since V0.9 the saved head can be **anchored** with a witness
  off the disk and checked against it, which does detect that rewrite; but
  anchoring and checking are explicit commands (`audit anchor`, `audit
  check-anchor`), the witness must be reachable, and in the harness the witness
  is a test peer, not a hardened service. Persisting is explicit (`audit
  save`), not automatic on every record.
- SMEP, SMAP and UMIP are enabled when the CPU advertises them (the harness's
  CPU model does); W^X for user segments, a guard page below the user stack, and
  user-pointer validation are always on. Since V0.9 the two static kernel
  stacks — the RSP0 stack every syscall runs on, and the double-fault IST
  stack — have unmapped guard pages, so an overflow stops the machine with a
  double fault naming the stack. That was prompted by the first TCP
  integration, which overflowed the (then unguarded) syscall stack and
  silently corrupted the capability table beside it. Kernel TASK stacks
  (32 KiB, heap-allocated) still have no guard page: an overflow there would
  still be silent corruption.
- No hardware root of trust, no Secure Boot chain, no measured boot.

## Storage

- NVMe **read + write + flush**, one I/O queue, one namespace; MSI-X delivery is
  proved on it, but the data path is still polled.
- ITFS is a minimal persistent filesystem: a fixed directory of **≤12 files**,
  no subdirectories, **contiguous** files, and **no free-block reuse** —
  `remove` and overwrite leak the old extent by design, trading space for
  crash-atomic simplicity, so a long-lived disk eventually fills. Crash
  consistency covers the superblock (double-buffered CRC commit); an overwrite
  is one crash-atomic commit; a torn *data* write of an unreferenced extent is
  harmless because nothing points at it yet. (V0.10: space reclamation.)
- No second block driver (AHCI/virtio-blk); the block trait is ready for one.
- QEMU attaches only generated disposable disks; no host disk is ever touched.

## Networking

- IPv4 over one e1000 NIC: ARP, ICMP echo (client and responder), UDP with
  capability-scoped Ring 3 sockets, and a DNS A-record resolver.
- **IPv6** (V0.9) is foundations only, brought up on demand (`ipv6`): a
  link-local address, SLAAC from a router's advertised /64, neighbour
  discovery and ICMPv6 echo in both directions. No extension headers (a packet
  carrying one is refused), no duplicate address detection, no DHCPv6, and no
  IPv6 UDP/TCP or Ring 3 IPv6 sockets.
- **TCP** (V0.9) is IPv4 only and client-side for Ring 3: programs can open a
  connection (`tcp_connect`), stream both ways and close, but there is no
  Ring 3 listen/accept yet (the state machine supports a passive open; it is not
  exposed). At most 8 connections, 4 KB send and receive buffers each. Loss
  recovery is a single retransmission timer (300 ms, doubling, 6 retries, then
  the connection times out) — no RTT estimation, no congestion control beyond a
  cap of four segments in flight, no SACK, window scaling, timestamps, fast
  retransmit, delayed ACK, zero-window probe or FIN-WAIT-2 timeout. TIME-WAIT is
  1 s (MSL 500 ms), a shortcut for the QEMU link. Initial sequence numbers mix
  the TSC with the four-tuple but are not RFC 6528's keyed hash — there is no
  per-boot secret yet. Timers run only when the stack is polled, so a finished
  connection's TIME-WAIT slot is reclaimed at the next poll, not on the clock.
- **DHCP** (V0.9) runs on
  demand (`dhcp`) and applies address, mask, router and DNS; it is not run
  automatically at boot and there is no background renewal at T1 — the lease
  is simply used until the next boot. Without it the static plan (10.0.2.15/24
  via 10.0.2.2) applies. No routing beyond one gateway, no fragmentation, and
  no ICMP error generation — an unreachable port is dropped silently.
- DNS through QEMU's user-mode forwarder failed on the Windows test host (no
  reply at all); DNS is verified against the harness's own peer.
- One NIC, polled; the receive path is drained from a scheduling slice, so a
  program that never yields also never receives.
- QEMU's 82540EM exposes no MSI capability, so the NIC has no interrupt path at
  all; MSI-X is proved on the NVMe controller instead.

## Interrupts and platform

- Line-based IRQs (timer, PS/2 keyboard and mouse) are delivered by the
  **I/O APIC** on the routes the ACPI MADT declares, and the 8259 PICs are
  retired (fully masked, LAPIC LINT0 masked) — since V0.9. Only those three ISA
  lines are routed; other devices are polled or use MSI-X. The PIT (mode 2) is
  still the scheduler tick; the local APIC timer is only used for a delivery
  proof. Single CPU: no IPIs, no AP startup, no x2APIC.
- ACPI support is table discovery (RSDP, RSDT/XSDT, MADT, FADT) plus the DSDT's
  `\_S5_` found by pattern match — **not an AML interpreter**, so methods,
  devices and power resources described in AML are invisible to the kernel.
- `poweroff` performs a real ACPI S5 soft-off; `shutdown` still uses QEMU's
  `isa-debug-exit` test device (the harness relies on its exit code). No
  suspend/resume, no thermal or battery handling.
- PCI enumeration scans bus 0 only (complete on the `pc` machine; no bridge
  recursion).

## Graphics, desktop and input

- One **bootloader-chosen framebuffer mode** (QEMU: 1280×720 BGR); no
  mode-setting, no vsync, software rendering only, **no GPU acceleration**.
- One 8×8 bitmap font, integer scaling only, no Unicode.
- Compositor: fixed wallpaper and top bar; **no window focus, drag, resize,
  z-order controls, or close buttons**; windows draw back-to-front in creation
  order. Ownership and bounds are enforced. The **live** desktop shows
  kernel-owned windows; a Ring 3 window is proved on screen in the selftest but
  a persistent Ring 3 application on the live desktop is not supported yet.
  (V0.10.)
- PS/2 keyboard is scancode set 1, US layout, printable keys plus a few
  controls; no key repeat policy, no IME. Mouse is relative 3-byte packets, no
  wheel. The first mouse packet after enabling reporting can decode as a benign
  zero-motion event.
- The console reads **polled serial**; the PS/2 keyboard drives the graphical
  desktop, not the console.

## USB and audio

- USB: UHCI (USB 1.1) and xHCI host controllers. Each enumerates **one device**
  on one root port: no hubs, no multi-device addressing, control and interrupt
  IN transfers only (no bulk or isochronous), no runtime attach/detach. HID boot
  protocol keyboard input is verified on both; USB mouse decode is host-tested
  only.
- Audio: AC97 **output only** — one PCM-out stream at 48 kHz, no capture, no
  mixing or resampling in the OS.

## Not present at all

- Wi-Fi, Bluetooth, webcam, GPU drivers, power management, suspend/resume, a
  browser, Linux binary compatibility, a production package ecosystem. These are
  research items that need physical hardware under an explicit, per-device
  authorization gate (`docs/IMPLEMENTATION_PLAN.md` §27).

## Project process

- Website deployment state is tracked in `docs/REQUIREMENTS.md` (CF-001 and the
  per-release WEB rows).
- The `v0.8.0` release reported `0.7.0-dev` from its own kernel; `v0.8.1` exists
  to correct that, and `website/scripts/check-consistency.mjs` now fails any
  build where the versions disagree.
