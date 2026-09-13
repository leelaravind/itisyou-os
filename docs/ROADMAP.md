# Roadmap — no fabricated dates

Milestones are sequenced by dependency, not calendar. A milestone is reached
when its verification gates pass, never before.

## V0.1 — Kernel Foundation *(complete — tag v0.1.0)*

Independently bootable x86_64 kernel in QEMU: serial diagnostics, physical +
virtual memory, heap, exceptions/interrupts/timer, kernel tasks + scheduler,
VFS/initramfs, interactive shell, automated boot/regression testing,
reproducible builds.

## V0.2 — Userspace Foundation *(complete — tag v0.2.0)*

Ring 3 execution of real ELF64 programs, strict loader (W^X, window, overlap
policy), syscall ABI (write/exit/yield/getpid), kernel/user MMU isolation,
crash containment, clean process teardown.

## V0.3 — Process Isolation + Storage Foundation *(complete — tag v0.3.0)*

Per-process page tables with verified cross-process isolation; concurrent
Ring 3 processes (cooperative), spawn/wait/exit; strict user-buffer
validation; a minimal IPC channel; PCI enumeration; a block-device
abstraction; a read-only NVMe driver.

## V0.4 — Preemptive Multitasking + Persistent Storage *(complete — tag v0.4.0)*

Timer-driven preemptive scheduling (non-yielding processes cannot monopolize;
registers + address spaces preserved across preemption); NVMe write + flush;
ITFS, a minimal persistent filesystem with double-buffered CRC-committed
superblocks and verified reboot persistence.

## V0.5 — Graphics + Input + Basic Desktop/Compositor *(complete — tag v0.5.0)*

Achieved: a real graphical environment produced by the OS inside QEMU — a
bootloader-provided linear framebuffer wrapped with a `u32` back buffer and an
8×8 bitmap font; a window compositor with per-window backing stores and
ownership/bounds isolation (a process cannot draw outside its own window);
GUI syscalls so Ring 3 renders a window with no direct framebuffer access; PS/2
keyboard (IRQ1) and mouse (IRQ12) input; and an interactive desktop that reacts
to real keyboard + mouse events. Verified by an in-kernel selftest with pixel
read-backs (including a Ring 3 window's colour read off the composited screen)
and by a QEMU-monitor input-injection test that also screendumps the OS-rendered
desktop. Remaining for a richer desktop (V0.6+): mode-setting, window
focus/drag/z-order, compositing a persistent Ring 3 window on the live desktop,
a broader GUI toolkit, and accessibility architecture.

## V0.6 — Hardware Expansion *(complete — tag v0.6.0)*

Achieved: a generic device/driver model (probe/bind, queryable device table)
over an enriched PCI foundation (BAR sizing, capability-list walking with
MSI/MSI-X/PCIe/PM detection, adversarial malformed handling); a **USB UHCI**
host controller with full device enumeration over control transfers and
**verified HID keyboard input** off the interrupt endpoint; an **AC97** audio
driver whose generated PCM samples are proven to traverse the driver → codec →
output path (captured to a WAV); a **unified input subsystem** (PS/2 + USB HID
behind one event stream, delivered to the GUI); hardware discovery via `lsdev`
(kernel + a Ring 3 tool); and a `devinfo` syscall giving userspace device
access without hardware authority. Drivers are polled, keeping the verified PIC
timer/PS-2 interrupt path intact. Remaining for a fuller platform: **USB xHCI**,
multi-device USB enumeration, bulk/isochronous transfers, audio capture,
APIC/IOAPIC/MSI interrupt routing, and ACPI/power foundations.

## V0.7 — System Platform *(complete — tag v0.7.0)*

Achieved: an explicit **capability model** (default deny at the syscall
boundary; delegation only narrows — a child can never hold what its parent
lacked); a **filesystem sandbox** (normalized-path prefix checks that defeat
traversal); a **service supervisor** over ordinary Ring 3 processes
(deterministic cycle-checked startup order, states, crash containment,
bounded restart policy, structured diagnostics — no hidden privileged
daemons); a managed **application model** (strict manifests requesting
capabilities; launches granted `manifest ∩ launcher` under an app sandbox);
the **ITPKG package** format (SHA-256-verified at install AND launch;
malformed/corrupt/hostile-manifest packages refused); **atomic updates with
rollback and recovery** on the persistent store (staged → one crash-atomic
commit; an interrupted update can never activate — proven across a real
reboot); and an **audit/provenance trail** for every privileged action and
denial. Future AI agents can only request through this same
capability/policy/service path (intelligence ≠ authority). Remaining for a
fuller platform: capability handles + revocation, signed packages (key
provisioning), userspace FS writes, long-running background services,
kernel-image updates.

## V0.8 — Networking, Authenticity & Hardening *(complete — tags v0.8.0, v0.8.1)*

`v0.8.1` is a release-integrity closeout, not new function: the `v0.8.0`
kernel reported `0.7.0-dev`, two requirement rows used a state outside the
plan's vocabulary, and the public site still made V0.1-era claims. It fixes
those, adds a build gate that fails on version or requirement-state drift, and
adds real-browser verification of the deployed site.

Delivered and verified: an e1000 driver with polled descriptor rings; ARP,
IPv4, ICMP echo (client and responder), UDP and DNS; capability-scoped Ring 3
sockets; Ed25519 package signatures with a compiled-in trust root;
capability-scoped userspace filesystem writes with atomic overwrite; the local
APIC, I/O APIC and MSI-X with real interrupt-delivery evidence; xHCI
enumeration and HID input; a hash-chained audit trail that survives reboots and
detects tampering; and SMEP/SMAP/UMIP alongside the existing W^X and stack
guard.

Deliberately NOT delivered, and recorded as such rather than reworded: **TCP**,
IPv6, DHCP, and routing beyond a single gateway. The I/O APIC is programmed but
its entries stay masked — line IRQs remain on the verified PIC path.

## Amendment — 2026-09-13 (Phase 1 audit)

The sequence below V0.8 was re-derived from dependencies rather than kept as
written, and the change is recorded here rather than made silently:

1. **A userspace-system milestone is inserted before the AI layer (new V0.10).**
   The AI layer's own premise is an agent that runs as an ordinary, supervised,
   least-privilege Ring 3 service and reaches the system only through the
   capability/policy/service path. Today processes run only while the kernel
   console drives the scheduler, services starve during a foreground command,
   there is no userspace `init`, and a Ring 3 program cannot keep a window on
   the live desktop. An agent built on that would be a kernel-console feature
   in disguise. The foundations come first — the plan's own rule is not to
   delay OS correctness for AI branding.
2. **The AI-native layer moves from V0.10 to V0.11**, unchanged in scope.
3. **Daily-driver research moves after V1.0.** Wi-Fi, Bluetooth, GPU
   acceleration, power management and suspend/resume need physical hardware;
   QEMU offers no Wi-Fi or Bluetooth device to verify against, and physical
   hardware requires an explicit per-device authorization gate
   (`docs/IMPLEMENTATION_PLAN.md` §27). A requirement that cannot be
   *verified* cannot gate V1.0, so V1.0 is defined as a VM-verified
   experimental release and says so.
4. **"Signing the audit chain head" becomes "anchoring it"** in V0.9: a key
   stored beside the log on the same disk protects nothing against the
   whole-disk attacker the item exists for, and there is no hardware root of
   trust to seal one. Moving the head out of that attacker's reach is the
   honest form of the same goal.

## V0.9 — Transport, Interrupt Cutover & Trust *(released as v0.9.0, 2026-09-13)*

Verified so far: ACPI discovery, the I/O APIC cutover with the 8259 PIC
retired (and the PIT-mode bug it exposed), ACPI S5 power-off, the DHCP client,
a fix for broadcast UDP checksums, and audit anchoring against an off-disk
witness, IPv6 foundations (client and responder paths), and TCP streams from
Ring 3 verified against the host's own TCP stack with loss injected, and the
package-signing key hierarchy (offline root, scoped certificates, signed
revocation). Released as v0.9.0 on 2026-09-13: release commit `f06673e`, CI run
`34760629701`; per-row evidence in `docs/REQUIREMENTS.md`.

- **TCP** with a connection state machine (active and passive open, orderly
  close, reset), a retransmission timer with backoff, and a receive window;
  Ring 3 stream sockets behind the port-scoped network capability; verified
  against a *real* host TCP stack, with loss injected to force retransmission.
- **DHCP** client and **IPv6 foundations** (NDP, SLAAC, ICMPv6 echo), verified
  against QEMU's user-mode network as an independent implementation.
- **ACPI** table discovery (RSDP → XSDT/RSDT → MADT, FADT) and the **I/O APIC
  cutover**: timer, keyboard and mouse delivered through the I/O APIC using the
  MADT's overrides, the 8259 PIC masked and retired; ACPI S5 power-off.
- **Package-signing key hierarchy**: an offline root, signing-key certificates
  with validity windows, a signed revocation list, rotation — and release
  private keys that never enter the source tree.
- **Audit head anchoring** outside the audited disk.

## V0.10 — Userspace System

*(Released as `v0.10.0`: release commit `c936bc7`, CI run `34782271250`
green; every item below verified, each step with a negative control.)*

A userspace `init` as the first process, starting services from configuration;
an always-on scheduler so services keep running whatever the console is doing;
program arguments; a Ring 3 shell; filesystem space reclamation so a long-lived
disk does not fill; and persistent Ring 3 applications on the live desktop with
focus and input routed to the focused window.

## V0.11 — AI-Native System Layer

A local inference service **outside** the kernel (a small model running in
Ring 3, trained reproducibly from the repository), system knowledge over an
approved, read-only view of local state, a diagnostic agent, policy-controlled
system actions with preview and explicit approval, provenance and audit, and
post-action verification with rollback. The invariant is unchanged: the agent
has intelligence, not authority.

## V1.0 — Experimental Personal OS

Declared only when its acceptance criteria — reliability, security, recovery,
update safety, installation/boot safety and supported environment — are
written down in advance and passed. It ships as a reproducible, checksummed,
downloadable boot image for virtual machines, with the tested environments
stated exactly and physical hardware explicitly unsupported.

## After V1.0 — Daily-Driver Research

Wi-Fi, Bluetooth, accelerated graphics strategy, power management,
suspend/resume, SMP, application ecosystem — real hardware only on explicitly
approved sacrificial devices, one device at a time.
