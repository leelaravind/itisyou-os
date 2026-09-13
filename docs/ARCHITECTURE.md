# Architecture — ITISYOU OS

This document describes what exists now and the boundaries later milestones
build inside. Anything marked *(planned)* is design intent, not implemented
functionality.

## System shape

```
┌───────────────────────────────────────────────────────────┐
│ firmware (SeaBIOS / OVMF-UEFI, inside QEMU)               │
└──────────────┬────────────────────────────────────────────┘
               │ rust-osdev bootloader (BIOS & UEFI stages)
               ▼
┌───────────────────────────────────────────────────────────┐
│ kernel (Rust no_std, x86_64, ring 0)                      │
│                                                           │
│  boot handoff (BootInfo) → early serial → CPU baseline    │
│  → memory map → PMM → paging → heap → GDT/TSS/IDT         │
│  → PIC/PIT timer → scheduler → VFS/initramfs → shell      │
│    (stages B010 … B150, serial-observable)                │
└───────────────────────────────────────────────────────────┘
   userspace (ring 3, syscall/sysret ABI)   … V0.2, single process (verified)
   privileged services / capability engine … future (planned)
   AI layer (intelligence, never authority) … future (planned)
```

## Userspace & processes (V0.2/V0.3, ADR-0004/0005/0006)

Concurrent Ring 3 processes, each with a **private address space** (per-
process L4; kernel entries shared supervisor-only, user window in L4 entry 0
— cross-process isolation is structural and MMU-verified). Entry/resume via
`sysretq` from a `#[repr(C)]` resumable `UserContext`; syscalls via
`syscall`/`sysret` on a masked dedicated kernel stack; faults at CPL=3 (#PF/
#GP/#UD) terminate only the process via a saved abort context. A preemptive round-robin
run-loop (`proc.rs`) rotates processes: the timer preempts a non-yielding
Ring 3 process after a fixed quantum (V0.4), saving its full trap frame; a
naked timer ISR saves/restores all GPRs and long-jumps to the run-loop on
preemption. `yield`/`wait` also save context and return to it. The strict ELF64 loader (`kernel-core::elf` + `user.rs`)
accepts only static ET_EXEC images. Syscalls: write, exit, yield, getpid,
uptime, args, spawn, wait, msg_send, msg_recv (and later additions listed in
`kernel/src/syscall.rs`) — user buffers validated against the active CR3
before any access. The first six convey no authority and are ungated;
everything else is default-deny behind a capability handle. Program
arguments (V0.10) are an immutable per-process block validated by
`kernel_core::progargs` at creation and read back with `args`; the console
passes them after `--` and a parent through `spawn_args`. `user/ulib` is the Ring 3 ABI side; evidence programs
`/bin/{init,gp-test,pf-test,child,parent}` are baked into the initramfs.

## Always-on scheduling, the process tree and init (V0.10, ADR-0022/0023)

The kernel is still not preemptible and the timer interrupt still does not
schedule. Instead, background processes get **bounded slices** (1 tick, 32
quanta or 20 ms) at a few audited **safe points** in kernel code: the idle
prompt, the console's job waits (`run`, `bg`, `pkg launch`, `rsh`), every
network poll, the desktop and `usbwait` loops. A host-tested gate
(`kernel_core::cosched`) refuses a slice with interrupts masked, inside a
run-loop, during a Ring 3 quantum, with a counted kernel lock held, or with
the NVMe store open; `sched` measures what the background got and whether a
command starved it. Every process has a parent; only the parent may collect
a child (`wait`, `wait_nohang`); `sleep` parks a process; one `finish` path
ends every process (exit, fault, `kill`) and hands its children to init or
reaps them. The kernel starts **`/sbin/init` as pid 1** with fixed authority;
init reads `/etc/init.conf` (host-tested grammar), starts and supervises the
services, and reports them to the kernel's service table through a strict,
kernel-checked record (`svc_report`); the kernel restarts a dead init, ending
its tree first, at most three times. A Ring 3 shell (`/bin/sh`) can borrow
the console's input (`console_read`); Ring 3 desktop apps receive focus, key
and click events for their windows (`gui_event`).

## Devices & storage (V0.3, ADR-0008)

PCI enumeration (`device::pci`, decoding in host-tested `kernel-core::pci`)
discovers controllers; a `BlockDevice` trait (`device::block`,
read+write+flush) is backed by a `RamDisk` and by a polled **NVMe** driver
(`device::nvme`) that maps BAR0 as uncacheable MMIO, sets up admin + one I/O
queue pair, and does Identify / Read / Write / Flush. On top sits **ITFS**
(`kernel-core::itfs` format logic + `kernel/src/fs_disk.rs` block I/O): a
minimal persistent filesystem — a fixed directory of contiguous append-only
files, committed through double-buffered CRC-protected superblocks for atomic
metadata updates against a crash. Verified to survive a full QEMU reboot. All
storage testing uses generated disposable QEMU disks — never a host disk.

## IPC (V0.3, ADR-0007)

Bounded kernel-owned message channels (copy-through-kernel send/recv) — the
smallest primitive for future system-service RPC, shaped to become
capability handles later.

## Graphics, input & desktop (V0.5, ADR-0010)

`gfx` wraps the bootloader-provided linear framebuffer (firmware mode; QEMU:
1280×720 BGR) with an in-heap `u32` back buffer and `fill_rect`/`draw_char`/
`draw_string`/`blit` primitives plus a CRC `hash_region`; one `present()`
converts the back buffer to the framebuffer's real pixel format. `gfx::
compositor` owns windows (each with an `owner_pid` and its own RGB backing
store), a wallpaper, top-bar chrome, and a cursor, and renders the scene
back-to-front. Every window op enforces ownership + bounds, so a process can no
more draw outside its window than write outside its address space; a process's
windows are dropped on exit. Ring 3 reaches the screen **only** through
validated GUI syscalls (`gui_create/fill/text/present`) — never the framebuffer
directly. `input` initializes the i8042 controller and decodes PS/2 keyboard
(IRQ1) and mouse (IRQ12) events via host-tested `kernel-core` decoders; the
mouse IRQ accumulates deltas lock-free (it must never take the compositor/FB
lock a `composite()` may hold). The `desktop` shell command enters an
interactive loop that applies input and re-composites. Pure logic (8×8 font,
scancode + mouse decoders) lives in `kernel-core` and is host-tested.

## Device model, USB & audio (V0.6, ADR-0011)

A generic device model sits over PCI: every function is probed into a `Device`
(identity + sized BARs + capability list, walked with a bounded loop-guarded
walker that detects MSI/MSI-X/PCIe/PM), and registered `Driver`s (`probe` →
`attach`) bind deterministically. The device table drives serial diagnostics
(B190), the `lsdev` shell command, and a `devinfo` syscall. Two real class
drivers bind through it: **AC97** audio, which plays a synthesized PCM tone
through a bus-master BDL and is proven end-to-end by capturing the samples to a
WAV; and **UHCI** USB, which enumerates a device over polled control transfers
(descriptors parsed by host-tested `kernel_core::usb`) and reads **HID keyboard
input** off the interrupt endpoint. USB HID and PS/2 decode into one unified
`InputEvent` stream the desktop consumes. Every V0.6 driver is **polled**, so
the verified PIC timer/PS-2 IRQ path is untouched; MSI/MSI-X is detected but
APIC/MSI routing is deferred. Userspace reaches devices only through `devinfo`
— no direct config/MMIO/port access.

## System platform (V0.7, ADR-0012)

Authority is explicit: each process carries **capability bits** (spawn/ipc/
gui/dev/fs_read), enforced default-deny at the single syscall dispatch
boundary; delegation only narrows (`spawn` inherits, `spawn_caps` intersects).
A per-process **FS sandbox** restricts `fs_read` to normalized-path prefixes
(traversal-proof, host-tested). **Services** are ordinary Ring 3 processes in
a static registry (binary + deps + exact caps); the supervisor starts them in
a deterministic cycle-checked order, contains crashes, applies a bounded
restart policy, and emits `[ITISYOU:SVC]` diagnostics. Since V0.8 a second
set of services is **persistent** (ADR-0014): started at boot, given CPU by
the shell's idle path rather than run to completion, and supervised for the
life of the system, where a daemon's clean exit counts as a failure. `bg`
co-schedules a client with them so an IPC round trip with a live service is
possible. **Applications** ship
as ITPKG packages (strict manifest + ELF + SHA-256, verified at install and
re-verified at launch) into a persistent version-numbered store where every
install/update/rollback/recovery transition is one crash-atomic ITFS
superblock commit — an interrupted update can never activate. Every
privileged action and every denial lands in the **audit trail**
(`[ITISYOU:AUDIT]`, bounded ring, no payload contents), hash-chained across
boots; `audit save` stores the newest 128 records with the head the first
of them extends (`itisyou-audit v2 … base= head=`, V0.11), boot recovery
adopts that trail and keeps its records for the next save, and `audit
verify` checks the file read-only against what the running kernel saved or
recovered. The trail and the package store are kernel-owned — the `fs_*`
syscalls refuse them — and every kernel echo of program- or disk-chosen
text is one escaped line with no marker prefix (V0.11: AUDIT11-001/002,
SEC11-001). Pure logic —
capabilities, manifests, package format, SHA-256, service ordering/restart,
update-state resolution, sandbox path math — lives host-tested in
`kernel-core`. A future AI agent is just another requester on this same
request → capability check → service → action → audit path; no AI exists in
the kernel.

## Networking (V0.8, ADR-0015)

An **e1000** driver with polled RX/TX descriptor rings, above it an interface
holding one IPv4 address and a bounded ARP cache, and above that a bounded UDP
socket table. Everything that *parses* lives host-tested in
`kernel_core::net::{checksum,eth,arp,ipv4,icmp,udp,dns}`: allocation-free,
borrowing from the DMA buffer, and refusing what it does not fully understand
(VLAN tags, fragments, ICMP types other than echo, DNS compression bombs)
rather than guessing. Authority is a **capability scoped to a port**: `udp_bind`
checks the Network handle against the port, and every later datagram call
re-checks the socket's own port, so a handle narrowed after the bind stops
working at the next use. Datagram syscalls never block — inside a syscall
`SFMASK` has cleared IF, so a wait there would freeze the machine — and return
`ERR_AGAIN` for the caller to retry on its own slice.

## Interrupt controllers (V0.8 ADR-0017, V0.9 ADR-0019)

Since V0.9 the **I/O APIC** delivers the line IRQs — PIT timer, PS/2 keyboard
and mouse — on the routes the ACPI MADT declares (on QEMU's `pc` machine the
PIT's IRQ0 arrives on GSI 2), with each route verified by read-back before the
cutover. The legacy 8259 **PIC** is retired: both chips fully masked and the
local APIC's LINT0 (the PIC's virtual-wire input) masked. The **local APIC**
takes the EOI and is proved to deliver (one-shot APIC timer on vector 0x41).
**MSI-X** is programmed on the NVMe controller and delivered by a real block
read's completion. The PIT runs in mode 2 (mode 3 double-delivered through
QEMU's edge-triggered I/O APIC path — found by the cutover).

## USB (V0.6 UHCI, V0.8 xHCI, ADR-0018)

Two structurally different host controllers sharing only descriptor parsing.
UHCI walks a frame list; xHCI is ring-based and command-driven — command ring,
per-endpoint transfer rings, and an event ring the controller owns. Input is
tagged with the controller that delivered it (`src=usb` / `src=xhci`).

## Hardening (V0.8)

**SMEP**, **SMAP** and **UMIP** are enabled from CPUID and reported from CR4.
SMAP inverts the default for user memory: the kernel is forbidden to touch it
except in three declared windows (the two user-copy helpers and `write`), each
bracketed by a guard whose `Drop` closes the window on every path. W^X is
enforced at load (a segment both writable and executable is refused), the user
stack is bounded by unmapped memory, and every user pointer is validated
against the ACTIVE address space — since V0.10 (SEC10-001) for being
user-accessible at every level of the walk, and writable at every level when
the kernel writes, not merely mapped. RFLAGS.DF and AC are cleared on every
entry from Ring 3, and the RSP0, double-fault, syscall and kernel-task stacks
all sit on unmapped guard pages (V0.9–V0.10).

## Crate boundaries

- **`kernel/`** — the only privileged code. Library + two binaries:
  `itisyou-kernel` (interactive) and `itisyou-kernel-selftest`
  (boots, runs in-kernel checks, exits QEMU with a deterministic status).
- **`crates/kernel-core`** — pure logic with zero I/O: boot-stage contract,
  serial-marker grammar, and (as subsystems land) memory-map normalization,
  path handling, parsers — including `net::{checksum,eth,arp,ipv4,icmp,udp,
  dns}` under the NIC driver, `ed25519` + `sha512` for package authenticity,
  and `audit_chain` for the tamper-evident trail. Compiled unchanged into both the kernel and host
  tools, unit-tested on the host.
- **`tools/image-builder`** — host tool; turns kernel ELFs into bootable
  BIOS/UEFI disk images (pure Rust) + SHA-256 manifest.
- **`tools/qemu-runner`** — host test harness; launches QEMU, asserts
  boot-stage markers, classifies panic/timeout/selftest outcomes, writes
  JSON evidence. Two guest channels: a serial line (markers, shell stdin) and
  an optional HMP **monitor** for injecting PS/2 input (`sendkey`/`mouse_move`/
  `mouse_button`) and capturing framebuffer screendumps. Absence of output is
  never success.

## Boot contract

The bootloader hands the kernel a typed `BootInfo` (memory regions, physical
memory mapping offset, framebuffer, RSDP). Kernel code consumes it through
`itisyou_kernel::early_init` so subsystems never depend on third-party boot
structures directly. Boot stages B000–B190 (defined in
`kernel-core::stage`, through B170 graphics, B180 desktop, B190 device model)
each emit one machine-parseable serial marker; the harness asserts them in order.

## Observability

Serial (COM1) is the primary channel and works before any allocator exists.
Marker grammar: `[ITISYOU:<TAG>] payload` with tags `B###`, `PANIC`,
`SELFTEST`, `TEST`, `INFO`, `MODE` — single source of truth in
`kernel-core::marker`, shared by emitter and asserter.

## Security posture

- The kernel runs in Ring 0 and every program in Ring 3, each in its own
  address space, with only the capabilities it was granted (V0.2–V0.10). The
  boundaries and what verifies each are in `docs/SECURITY_MODEL.md`; the
  risks and residuals in `docs/THREAT_MODEL.md`.
- Panic on violated invariants; panics emit `[ITISYOU:PANIC]` and, in test
  mode, fail the QEMU run deterministically.
- The AI-authority principle (intelligence ≠ authority) constrains future
  interfaces: kernel APIs are designed so privileged operations can later sit
  behind a deterministic policy/service layer rather than being callable by
  an agent directly. No AI component exists in the V0.1 runtime.

## Planned next boundaries

V0.11 adds the AI boundary as ordinary Ring 3 services under the existing
capability, audit and approval paths (`docs/ROADMAP.md`): an agent gets no
entry point a program does not have.
