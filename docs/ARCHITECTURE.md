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
   userspace (ring 3, syscall/sysret ABI)   … since V0.2; concurrent, isolated,
                                              preempted processes; /sbin/init
                                              as pid 1 since V0.10 (verified)
   privileged services / capability engine … capability handles, services,
                                              signed packages, audit (V0.7–V0.10,
                                              verified)
   AI layer (intelligence, never authority) … V0.11, ring 3 (in development)
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
capability handles later. Eight channels since V0.11 (6 and 7 are
`/bin/inferd`'s); a message carries no sender identity, so what arrives on
a channel is a claim, not an authenticated answer.

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
syscalls refuse them — ITFS refuses names with control characters, and
every kernel echo of program- or disk-chosen text is one escaped line with
no marker prefix (V0.11: AUDIT11-001/002, SEC11-001). Pure logic —
capabilities, manifests, package format, SHA-256, service ordering/restart,
update-state resolution, sandbox path math — lives host-tested in
`kernel-core`. The V0.11 agent is a requester on this path with an approval
step added: its request is a proposal the kernel checks, only the console
approves it, and only kernel code acts (see "AI-native system layer").

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

## AI-native system layer (V0.11, in development; ADR-0024)

A diagnostic agent that can reason about the system and propose two
actions, and nothing else. Inference and diagnosis run in Ring 3; any
change a proposal leads to is made by kernel code after a console
approval, then verified, and rolled back if the check fails. None of it is
in a release yet (the latest is v0.10.0).

```
sys_view (42) → agent → inferd (IPC 6 → 7) → agent → propose (43)
  → kernel checks → proposal table → console `approve <id>`
  → kernel executes → verifies → rolls back on failure
     retry-service only: kernel → init mailbox (init_ctl, 44) → /sbin/init
```

**Components.** `kernel_core::sysview` is the approved view: a 232-byte
`ITVIEW01` record of process counts by state, up to eight service rows
(name, state, restarts, whether the live init reported it; more set a
`truncated` flag), scheduler state and slice counters, audit and network
counters and uptime — no paths, arguments, file names or payloads. Its
`features()` scales 16 features into 0..1000 and is the one function the
agent, the tests and the kernel's recomputation share. `kernel_core::model`
is three integer yes/no detectors (`service_failed`, `scheduler_paused`,
`denial_burst`; averaged perceptron, no floating point) trained on
SYNTHETIC examples that `kernel_core::scenario` generates from
`ai/scenarios.txt`. `kernel/build.rs` trains it with the same code, refuses
to build on a stale pin (`ai/diag.model.sha256`), ships the 276-byte
`/etc/ai/diag.model` and one runbook per condition (`ai/kb/` →
`/etc/ai/kb/`, a closed table) in the initramfs — which is embedded in the
kernel image — and compiles the model's SHA-256 into the kernel
(`ai::MODEL_SHA256`). At boot `ai::boot_check` compares the two and decodes
the model; the kernel keeps its own decoded copy only when they match.
`/bin/inferd` is `/sbin/init`'s third service (`ipc,fs_read` under
`/etc`): it loads the model, refuses a malformed file by name, and answers
on IPC channel 6 (a nonce and 16 features in) and 7 (the nonce, the
conditions that fired, every score and the model's SHA-256 out;
`kernel_core::infer`). `/bin/agent` is started from the console with the
view, IPC and `fs_read` under `/etc/ai` — plus `propose` for its `propose`
mode: `diagnose` reads the view, asks inferd (a 3 s bounded wait; replies
with another nonce discarded), reads the runbook of each condition that
fired and prints the diagnosis and the action the runbook names; `propose`
then files one proposal per run, a paused scheduler first. `/bin/ai-probe` (adversarial runs) and
`/bin/flakyd` (init's fourth service, which fails while the system is under
3 s old, so a retry can succeed) exist for the evidence.

**Syscalls.**

| # | Call | Gate | Effect |
|---|---|---|---|
| 42 | `sys_view(buf, len)` | `CAP_SYS_VIEW` (bit 11) → SystemAdministration READ | builds the view from the kernel's own tables and copies all 232 bytes or nothing (`ERR_2BIG` for a short buffer); only then remembers the bytes and tick for the caller, forgotten when it ends |
| 43 | `propose(record, len)` | `CAP_PROPOSE` (bit 12) → SystemAdministration USE | checks an 88-byte `kernel_core::policy` record and returns the proposal id, `ERR_INVAL` (malformed), `ERR_AGAIN` (`already_pending`, `table_full`) or `ERR_PERM` (refused on the kernel's own knowledge); every refusal is printed and audited by name |
| 44 | `init_ctl(op, buf, len)` | Service ADMIN, and the caller must be the live init | op 0 fetches the pending 24-byte command (`ERR_AGAIN` if none); op 1 acknowledges it with a 16-byte record that must name the command fetched; anyone else is refused `not_init`, audited |

Both new bits are console-only: outside `CAP_LEGACY_FULL`, dropped by
`caps::delegate` (so no `spawn` passes them on), refused in
`/etc/init.conf` (`console_only_capability`), and — rights being exact
subsets — not implied by the ADMIN right `sys_admin` carries.

**Proposal checks.** `propose` decodes the record strictly — version,
action, condition, runbook id equal to the condition, a target
`[a-z0-9-]{1,15}` or none, a zero reserved field, and the closed condition
→ action table (`service_failed` → `retry-service`, `scheduler_paused` →
`resume-scheduler`, `denial_burst` → none) — then checks, in order: the
model digest is the compiled-in one (`unknown_model`); the view digest is
the last view served to this process (`snapshot_mismatch`), at most 500
ticks (5 s) old (`snapshot_stale`); recomputing from those exact bytes with
the kernel's copy of the model fires the cited condition
(`diagnosis_mismatch`); the action applies now by facts the kernel gathers
itself — the scheduler is paused, or the named row is Failed, owned by the
live init and defined in `/etc/init.conf` (`not_applicable`); the submitter
has nothing pending (`already_pending`) and the 8-slot table has room
(`table_full`; finished entries are evicted oldest first, ids are never
reused). What passes waits as pending for at most 6000 ticks (60 s).
Nothing on this path acts.

**Approval and execution.** `proposals`, `approve <id>` and `deny <id>`
exist only in the kernel console, which reads no input while a Ring 3 shell
holds it; the listing and each preview are kernel `[ITISYOU:AI]` lines
built from validated fields. `approve` expires stale entries, refuses what
is not pending (`already_decided`) or unknown (`no_such_proposal`),
re-checks applicability from fresh facts (`precondition_changed`, and the
entry expires), then executes in kernel code, verifies, and on failure
rolls back. The lifecycle is one-way (pending → approved | denied |
expired; approved → executed → verified | rolled back). Filing, refusal,
approval, denial, expiry, execution, verification and rollback are each
printed and audited — `proposal_submitted` and `action_executed` with the
model and view digests the proposal cited, a refusal by its reason, the
rest by proposal id. `resume-scheduler` calls `sched::resume` and passes
only if other processes progress at busy points during a 100-tick window;
rollback pauses the scheduler again. `retry-service` never starts a
program behind init's back: the kernel arms a watch, then posts
`retry <name>` to a one-slot mailbox (`kernel/src/initd.rs`, records in
`kernel_core::initctl`); init, which checks it at the top of every loop,
restarts the Failed slot with its count reset and acknowledges with the
new pid. A Restart, Failed or Done report from init for that name within
the 300-tick window trips the watch inside `svc_report` and posts `stop`
at once; a pass needs no trip and the row Running with the acknowledged
pid. On `stop` init stops supervising the service, reports it Failed and
names the still-running instance, which the kernel kills (init has no
kill) — only if that pid is a live child of init running the program
`/etc/init.conf` gives the service; anything else is refused
`not_an_instance`. The kernel waits 3 s for each acknowledgement and
withdraws the command if none comes; if init had already fetched a `retry`
it did not acknowledge, the kernel rolls it back as for a failed check. The
mailbox is cleared when init dies, so a command never reaches its
successor.

**Trust boundaries.** The agent, inferd and anything else on IPC are
untrusted. inferd's answer is a claim — IPC carries no sender, and any IPC
holder can read or write channels 6 and 7 — and the agent's lines are
Ring 3 output: marker prefix rewritten and, since V0.11, control characters
shown escaped (OUT11-001), so no program can redraw a preview. The kernel
relies only on what it establishes itself: the view (from its own tables),
the view's binding to the submitting process and its age, the recomputed
condition, and the applicability facts. It evaluates the shipped model only
to check a claim, so the model's output can cause a refusal, never an
action. The console is the only approver; kernel code — for
`retry-service`, through init, which already supervises the services — is
the only executor.

## Crate boundaries

- **`kernel/`** — the only privileged code. Library + two binaries:
  `itisyou-kernel` (interactive) and `itisyou-kernel-selftest`
  (boots, runs in-kernel checks, exits QEMU with a deterministic status).
- **`crates/kernel-core`** — pure logic with zero I/O: boot-stage contract,
  serial-marker grammar, and (as subsystems land) memory-map normalization,
  path handling, parsers — including `net::{checksum,eth,arp,ipv4,icmp,udp,
  dns}` under the NIC driver, `ed25519` + `sha512` for package authenticity,
  and `audit_chain` for the tamper-evident trail, and (V0.11) `sysview`,
  `scenario`, `model`, `infer`, `policy` and `initctl` for the AI-native
  layer. Compiled unchanged into both the kernel and host
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
Marker grammar: `[ITISYOU:<TAG>] payload` — the stage and harness tags
`B###`, `PANIC`, `SELFTEST`, `TEST`, `INFO`, `MODE` are the single source
of truth in `kernel-core::marker`, shared by emitter and asserter; the
subsystems add their own tags on the same grammar (among them `AUDIT`,
`SVC`, `INIT`, `SCHED`, `PKG`, `NET`, `AI`). The prefix is the kernel's:
process output has it rewritten to `[RING3-U:` across every way the output
is split (OUT10-002, OUT11-002), and text a program or a disk chose is
echoed escaped (AUDIT11-002, SEC11-002).

## Security posture

- The kernel runs in Ring 0 and every program in Ring 3, each in its own
  address space, with only the capabilities it was granted (V0.2–V0.10). The
  boundaries and what verifies each are in `docs/SECURITY_MODEL.md`; the
  risks and residuals in `docs/THREAT_MODEL.md`.
- Panic on violated invariants; panics emit `[ITISYOU:PANIC]` and, in test
  mode, fail the QEMU run deterministically.
- The AI-authority principle (intelligence ≠ authority) shaped the kernel
  APIs so an agent cannot call a privileged operation directly: what it
  wants goes through a deterministic policy and approval path. V0.11 (in
  development) implements it as described in "AI-native system layer":
  inference and diagnosis in Ring 3, proposals the kernel checks, console
  approval, kernel execution with verification and rollback. Released
  versions up to v0.10.0 contain no AI component.

## Planned next boundaries

After V0.11 the roadmap's next milestone is V1.0, an experimental release
declared only against acceptance criteria — reliability, security,
recovery, update safety, installation/boot safety, supported environment —
written down in advance and passed (`docs/ROADMAP.md`).
