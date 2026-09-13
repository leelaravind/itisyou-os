# Known limitations — honest current state

Updated continuously; last full revision: 2026-09-13 (v0.9.0 release; earlier: V0.8.1 release-integrity
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
- Scheduling is **always-on but cooperative** (V0.10, ADR-0022): background
  processes get bounded slices (1 tick, 32 quanta or 20 ms) at audited safe
  points — the idle prompt, `bg` job waits, every network poll, the desktop
  and `usbwait` loops, and between quanta of a foreground `run` — never from
  the timer interrupt; the kernel itself is not preemptible. Inside these
  **non-schedulable regions** the background waits for the region to end:
  every syscall (`net_resolve` can take 1.5 s), `irq` (about 200 ms with the
  timer masked), `xhciwait` (up to 5 s), `beep`, and every command that holds
  the NVMe store (`store`, `pkg install/stage/rollback/recover`,
  `audit save/verify`). Measured with `sched last` (the whole command window,
  QEMU on the development laptop): `store put` 37 ms, `pkg install` 1547 ms,
  `irq` 932 ms, `xhciwait` with no input 5002 ms — against 49 ms for a busy
  `net poll`. A background process's effective quantum is 10–20 ms of Ring 3
  time; syscall time is not charged to it. The NIC is polled only by syscalls
  and console network commands (a slice never polls it), and `tickd` still
  spins on `yield`, so an idle system never halts the CPU.
- **Userspace init** (V0.10, ADR-0023): `/sbin/init` (pid 1) starts and
  supervises the services in `/etc/init.conf`, which lives in the initramfs —
  there is no persistent configuration and no reload. Its authority is fixed
  (spawn, IPC, `fs_read` under `/etc`, service reporting), every service
  inherits its `/etc` filesystem sandbox, and only init and its configured
  services have deterministic pids. If init dies the kernel ends its whole
  tree and restarts it at most 3 times. `svc_report` trust: the kernel
  verifies pid, parentage, caps and row ownership; a service's name and
  policy are init's claim, attributed with `supervisor=`.
- **Ring 3 shell** (V0.10): `/bin/sh` runs only when the kernel console's
  `rsh` lends it the console's input, and the kernel console waits until it
  exits; the kernel console is still where the system is administered
  (`store`, `pkg`, `net`, `audit`, … exist only there). The shell has
  builtins and `run` — no pipes, redirection, job control, quoting or
  environment. Console input is polled (the UART interrupt is not routed),
  and the kernel reads input for the shell only while the shell is waiting
  for a line.
- Process model (V0.10): spawn, wait and a non-blocking `wait_nohang`, only by
  the parent; `sleep`; orphans reaped automatically (no adopter until
  `/sbin/init` runs); the console's `ps` and `kill`. No fork/exec, no process
  groups, no signals — `kill` is the console's alone and cannot be caught.
  Program arguments (V0.10) are
  bounded and deliberately narrow: at most 16 arguments and 512 bytes, each
  argument printable ASCII without spaces (no quoting, no UTF-8, no empty
  arguments); no environment variables. From the console a line is still
  capped at 256 bytes and 24 tokens, so the 512-byte limit is reachable only
  through `spawn_args`. Since V0.10 the console's `run` runs a program as a
  process in the table, so its `wait` blocks and its `sleep` sleeps (before,
  a foreground program ran alone and both returned at once); `run` has no
  timeout — the console waits until the program ends — while `bg` gives up
  after 30 s.
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
  save`), not automatic on every record. Since V0.11 the stored trail is a
  **window of the newest 128 records** whose header names the head its first
  record extends (`base`): records before the window are gone from the disk
  and the chain vouches only for those still present (dropping them changes
  the stored count, which the anchor compares). `audit verify` is read-only
  and compares the file with the trail this boot last saved or recovered, so
  a replacement is caught while the system runs even when its chain is valid;
  across a reboot only the anchor catches a valid replacement. Trails written
  by v0.8.0–v0.10.0 after the ring had dropped a record, or by a boot other
  than the first, are reported `TAMPERED` by every version, V0.11 included:
  that is AUDIT11-001, and such a trail cannot be told apart from a tampered
  one — the next save replaces it with a verifying one. An unreadable trail is
  still overwritten by the next save.
- The persistent store is shared: programs holding `fs_read`/`fs_write` reach
  every name in it except the files the kernel owns — the audit trail and
  the package store (`<app>.<v>.pkg`/`.ok`), refused since V0.11 (SEC11-001;
  v0.10.0 and earlier let such a program replace the audit trail, or delete
  a package's commit marker to roll it back). There is no per-program
  directory beyond the path-prefix sandbox. Names with control characters
  are refused, and every kernel echo of a name or of file contents is one
  escaped line (AUDIT11-002; v0.10.0 and earlier echoed them raw, so a
  program's file name could print a line that read as a kernel marker).
- SMEP, SMAP and UMIP are enabled when the CPU advertises them (the harness's
  CPU model does); W^X for user segments, a guard page below the user stack, and
  user-pointer validation are always on — since V0.10 the validation also
  checks that the program itself may write a buffer the kernel writes (v0.9.0
  and earlier checked only that it was mapped, so any program could crash the
  kernel through `cap_list`: SEC10-001). Four kinds of kernel stack are on
  unmapped guard pages, so an overflow stops the machine with a double fault
  naming the stack instead of silently corrupting kernel data: since V0.9 the
  RSP0 stack (interrupts and exceptions arriving from Ring 3) and the
  double-fault IST stack; since V0.10 the syscall stack — a third static stack,
  32 KiB, the one the first TCP integration overflowed into the capability
  table, which v0.9.0 left unguarded although its documentation said otherwise
  (corrected after the release) — and the kernel TASK stacks (32 KiB each, in
  fixed slots of a dedicated window whose first page is never mapped). Whether
  the bootloader-provided boot/console stack has a guard page
  has not been verified.
- No hardware root of trust, no Secure Boot chain, no measured boot.

## Storage

- NVMe **read + write + flush**, one I/O queue, one namespace; MSI-X delivery is
  proved on it, but the data path is still polled.
- ITFS is a minimal persistent filesystem: a fixed directory of **≤12 files**,
  no subdirectories, and **contiguous** files. Since V0.10 freed space is
  reused (FS10-001): free space is recomputed from the gaps between the live
  extents the superblock lists — no free list is stored and the on-disk format
  is unchanged — and allocation is first-fit over those gaps. Crash
  consistency is unchanged: the superblock is a double-buffered CRC commit, an
  overwrite is one crash-atomic commit, and new data only ever goes to blocks
  that NEITHER superblock slot references (the committed one or the one a
  mount would fall back to), so a torn data write is harmless and an old
  extent is reused only after two commits have stopped referencing it.
  Remaining limits: **no compaction or defragmentation** — files are
  contiguous, so a write larger than every gap is refused with `Fragmented`
  even when enough blocks are free in total (removing or overwriting a
  neighbour opens a gap); freed space becomes allocatable one commit late
  (the fallback slot pins it, reported as `pinned` by `store df`), so an
  overwrite needs a free run outside BOTH the committed and the fallback
  copies of the file — repeatedly overwriting a file larger than about a
  third of the free space can be refused until another commit releases the
  pin. That is deliberate: pinning only the committed slot would already be
  crash-safe, but a later unreadable newest superblock would then make mount
  fall back to a directory whose data may have been overwritten, and serve
  it silently; with both slots pinned the fallback is always intact. And a
  handle whose superblock commit failed re-reads both slots before its next
  transaction on a best-effort basis — if that re-read also fails, the next
  write on the same handle trusts memory, as V0.9 did (every console and
  syscall operation mounts afresh, so no long-lived handle crosses a failure).
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
- **TCP** (V0.9) is IPv4 only. Ring 3 programs can open a connection
  (`tcp_connect`) or listen for one (`tcp_listen`), stream both ways and
  close. A listen serves exactly one connection — the listening descriptor
  becomes the stream — so there is no accept queue and no second client
  until the program listens again. At most 8 connections, 4 KB send and
  receive buffers each. Loss
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
- Compositor: fixed wallpaper and top bar; click-to-focus with raise (V0.10)
  but **no drag, resize, minimize or close buttons**. Ownership and bounds are
  enforced; a process holds at most 4 windows. One Ring 3 app can be started
  on the live desktop (`desktop <app>`); the console cannot start more while
  the desktop runs (it does not read the console), and ESC always leaves the
  desktop (it exits QEMU). Apps receive focus, key and click events; there is
  no pointer-motion or key-release event, and a full redraw happens on every
  change.
- PS/2 keyboard is scancode set 1, US layout, printable keys plus a few
  controls; no key repeat policy, no IME. Mouse is relative 3-byte packets, no
  wheel. Only the first USB HID device is enumerated.
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
