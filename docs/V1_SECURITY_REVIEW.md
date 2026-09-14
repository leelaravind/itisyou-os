# V1.0 whole-attack-surface security review (V1-SEC-002)

**Criterion (`docs/V1_ACCEPTANCE.md` V1-SEC-002):** a multi-agent review of
the whole kernel attack surface — every syscall, the ELF loader, the VFS and
ITFS, packages and the trust hierarchy, the network stack, input and device
parsing, ACPI, the AI layer, the audit trail — with every finding verified by
an independent refuter. Zero confirmed high-severity findings open at release;
every confirmed medium fixed, or recorded in `docs/KNOWN_LIMITATIONS.md` with
the reason it is accepted.

**How it was run.** Twelve area reviewers read the code in parallel (read-only:
no builds, no edits), each against the threat model in `docs/THREAT_MODEL.md`
and a shared severity rubric, reporting only defects with a concrete trigger
traced from an attacker entry point to a defective line. Every reviewer's
findings were then handed to an **independent refuter** told to refute each one
by reading the code itself (default to refuted when uncertain). A thirteenth
agent, the completeness critic, listed every kernel source file and every
syscall number and reviewed what no area reviewer had covered — it found the
cross-cutting CPU-state gaps (unhandled exception vectors, FPU state, syscall
register cleanup) that no per-subsystem review owned.

**Result.** 37 distinct findings survived refutation: **3 high, 15 medium, 19
low** (the critic's `cpu-1` and `cpu-3` are the same defects as `proc-1` and
`sc-1`). Every high is **fixed**. Of the mediums, 8 are fixed and 7 are
accepted and recorded in `docs/KNOWN_LIMITATIONS.md` (each an availability
limit a capability holder or a hostile disk can impose on a single-user VM —
none crosses a privilege boundary or corrupts kernel memory). 18 findings in
all are fixed, each with a QEMU leg and a mutation control (the fix disabled,
the leg shown failing, the fix restored): see `scripts/test.ps1` legs
`fault-contain-{bios,uefi}`, `pkg-store-hostile-bios`, `gui-quota-bios`, `cap-exhaust-bios`, the
`sysfuzz-*` legs, and the net/fs/platform legs, and
`E:\claude-tmp\itisyou-audit\v1\sec002\` for the mutation controls. The one refuted finding (`pkg-4`) is kept in the table
with the refuter's reasoning.

Fix IDs used in code and commits: SEC1-001 (CPU exception containment),
SEC1-002 (user FPU off), SEC1-003 (syscall register scrub), SEC1-004 (package
name binding at launch), SEC1-005 (strict store names + checked next version),
NET1-001 (non-recursive ICMP responder), NET1-002 (broadcast/multicast source
and echo drop), NET1-003 (in-place TCB reset), NET1-004 (bounded NIC drain),
GUI1-001 (per-owner window pixel quota), FS1-001 (read-only syscalls never
format), USB1-001/002/003 (xHCI and MMIO
bounds), CAP1-001 (loud refusal on capability-table exhaustion).

## Findings

Severity is the refuter's where it differs from the reviewer's. Disposition
`fixed` = corrected in V1.0 with a leg and control; `accepted` = recorded in
`docs/KNOWN_LIMITATIONS.md` with its reason; `low — noted` = recorded here and
left for a hardening pass.

| ID | Area | Severity | Verdict | Disposition | Site | Finding |
|---|---|---|---|---|---|---|
| cpu-1 | critic | high | confirmed | fixed (= proc-1) | `kernel/src/interrupts.rs:127` | Ring 3 #DE/#DB/#SS find no IDT gate, escalate to #DF, and panic the kernel |
| net-1 | net-ipv4 | high | confirmed | fixed | `kernel/src/net/mod.rs:583` | IPv4 ICMP echo responder re-enters poll() through blocking ARP resolve — remote peer drives unbounded kernel-stack recursion to a guard-page double fault |
| pkg-1 | packages-trust | high | confirmed | fixed | `kernel/src/platform.rs:378` | pkg launch does not check the stored package's manifest name against the app name it launches, so the published test key's hello-* scope does not apply at launch |
| proc-1 | process | high | confirmed | fixed | `kernel/src/interrupts.rs:127` | Ring 3 divide-by-zero (unhandled #DE) escalates to a double fault and panics the kernel |
| audit-1 | audit-console | medium | confirmed | accepted | `kernel/src/audit.rs:91` | Any process, with no capabilities, can flush every earlier record out of the stored 128-record trail window and the 64-record ring |
| out-1 | audit-console | medium | confirmed | accepted | `kernel/src/serial.rs:30` | Ring 3 partial output leaves the console 'mid-line' and stops every busy-point slice, which defeats approve's retry-service verification |
| audit-2 | audit-console | medium | confirmed | accepted | `kernel/src/syscall.rs:1146` | A program holding fs_write can take every ITFS directory slot (or the free space) so the kernel-owned audit trail can never be saved |
| cap-1 | caps-ipc | medium | confirmed | fixed | `kernel/src/capability.rs:99` | Global 256-entry capability table has no per-process/per-tree quota and no process limit; a spawn holder fills it and every later process silently gets no authority |
| cap-2 | caps-ipc | medium | confirmed | accepted | `kernel/src/syscall.rs:983` | Any zero-capability program can flush the audit window at will with ungated cap_check denials, evicting evidence of earlier actions before `audit save` |
| fs-1 | fs | medium | confirmed | fixed | `kernel/src/lib.rs:321` | Read-only fs_read/fs_list can format the persistent store (READ right causes a destructive write); the 'never format a corrupt disk' rule is also ineffective |
| fs-2 | fs | medium | confirmed | accepted | `kernel/src/syscall.rs:1146` | One program can take all 12 ITFS directory slots with names the console cannot type: package install/update and (if the trail is absent) audit persistence blocked across reboots with no in-OS recovery |
| fs-3 | fs | medium | confirmed | fixed | `kernel/src/platform.rs:257` | Hostile disk: a package-store name with version u32::MAX panics the kernel on the next pkg install/stage (u32 overflow in next-version computation) |
| gui-1 | gui | medium | confirmed | fixed | `kernel/src/gfx/compositor.rs:110` | One Gui program can take the whole compositor pixel budget (or every window slot), so every other process is refused a window and the kernel's own `desktop` exits the VM |
| usb-1 | input-devices | medium | confirmed | fixed | `kernel/src/device/xhci.rs:401` | xHCI scratchpad pointer array written past its single 4 KiB DMA frame when the controller asks for more than 512 buffers |
| audit-flood-1 | input-devices | medium | confirmed | accepted | `kernel/src/console.rs:123` | Any program with no capabilities can flood audited denials (console_read not_owner) and push other processes' records out of the audit ring and the saved-trail window |
| tcp-1 | net-tcp-ipv6 | medium | confirmed | accepted | `kernel/src/net/tcp.rs:373` | Half-closed and orphaned TCP connections are never reclaimed: a remote peer pins connection slots and service ports until reboot |
| tcp-2 | net-tcp-ipv6 | medium | confirmed | fixed | `crates/kernel-core/src/net/tcp.rs:963` | Remote SYN+RST to a listener builds an 8 KB Tcb by value on the 32 KB syscall stack (the V0.9 overflow pattern ADR-0020 forbids) |
| tcp-3 | net-tcp-ipv6 | medium | confirmed | fixed | `kernel/src/net/mod.rs:467` | Unbounded NIC drain loop with interrupts masked: a sustained frame flood freezes the whole machine and starves the TCP timers |
| pkg-2 | packages-trust | medium | confirmed | fixed | `kernel/src/platform.rs:257` | A store version of 4294967295 overflows stage()'s next-version computation and panics the kernel during install |
| ai-1 | ai | low | confirmed | low — noted | `kernel/src/ai.rs:281` | proposal_expired audit records are stamped with the pid of whichever unrelated process happened to call propose |
| audit-3 | audit-console | low | confirmed | low — noted | `kernel/src/proc.rs:814` | The spawn audit record stores the caller's raw path string, not the canonical path of the program actually loaded |
| out-2 | audit-console | low | confirmed | low — noted | `kernel/src/console_out.rs:87` | A per-process output slot stays held for the whole life of any process that ever wrote, so a few idle children exhaust all 16 and break line atomicity for everyone |
| cap-3 | caps-ipc | low | confirmed | low — noted | `crates/kernel-core/src/capability.rs:202` | A Service handle with ADMIN (CAP_SERVICE) is unscoped, so any `service,ipc:<x>` holder reaches inferd's reserved channels 6-7, contrary to KNOWN_LIMITATIONS and V1-SEC-006 |
| cap-4 | caps-ipc | low | confirmed | low — noted | `kernel/src/syscall.rs:1043` | cap_restrict can never narrow a channel-scoped IPC handle (always scope_denied), so IPC authority cannot be time-limited or reduced since V1.0 |
| cap-5 | caps-ipc | low | confirmed | low — noted | `kernel/src/ipc.rs:80` | An oversized IPC message is left at the head of the queue on ERR_2BIG, so any default-`ipc` program permanently wedges tickd's request channel |
| cpu-2 | critic | low | confirmed | fixed | `kernel/src/user.rs:32` | x87/SSE register state is shared across processes: never saved or restored on a switch |
| cpu-3 | critic | low | confirmed | fixed (= sc-1) | `kernel/src/syscall.rs:416` | sysretq returns kernel scratch registers (rdi, rsi, rdx, r8-r10) to Ring 3 |
| fs-4 | fs | low | confirmed | low — noted | `kernel/src/device/nvme.rs:275` | NVMe Identify LBA data size is not validated: LBADS >= 64 panics (shift overflow) at boot and on every store access; LBADS > 12 makes each 1-block transfer spill past the single PRP page into PRP2 = 0 |
| fs-5 | fs | low | confirmed | low — noted | `kernel/src/device/nvme.rs:209` | NVMe CAP.DSTRD is not bounded against the 8 KiB BAR mapping: doorbell writes for DSTRD >= 9 land outside the mapped window |
| gui-2 | gui | low | confirmed | low — noted | `kernel/src/gfx/compositor.rs:119` | Window id counter `next_id += 1` (u32) panics on overflow in the dev profile |
| usb-2 | input-devices | low | confirmed | fixed | `kernel/src/device/xhci.rs:369` | xHCI DBOFF/RTSOFF used without bounding them to the mapped BAR window: out-of-window MMIO writes, kernel page-fault panic at boot |
| usb-3 | input-devices | low | confirmed | fixed | `kernel/src/device/xhci.rs:355` | xHCI maps an unvalidated BAR0 size; map_mmio's unchecked arithmetic overflows (panic) or maps an enormous window |
| usb-4 | input-devices | low | confirmed | low — noted | `kernel/src/device/uhci.rs:573` | UHCI and AC97 compute I/O port numbers with unchecked u16 addition: a high I/O BAR panics the kernel at boot |
| usb-5 | input-devices | low | confirmed | low — noted | `kernel/src/device/xhci.rs:291` | xHCI await_event drains the event ring without checking its deadline: a device that keeps producing events hangs the kernel |
| net-2 | net-ipv4 | low | confirmed | fixed | `kernel/src/net/mod.rs:545` | ICMP Echo Requests to the broadcast address are answered, giving a wire peer a spoofed-source reflection primitive |
| tcp-4 | net-tcp-ipv6 | low | confirmed | fixed | `kernel/src/net/mod.rs:553` | TCP answers segments from a broadcast source address with RSTs/SYN-ACKs sent to the link broadcast, contrary to ADR-0020 and THREAT_MODEL |
| tcp-5 | net-tcp-ipv6 | low | confirmed | low — noted | `kernel/src/net/tcp.rs:202` | No per-owner limit on the 8 TCP slots: one program with a network capability for a single port can monopolise TCP for everyone |
| pkg-3 | packages-trust | low | confirmed | fixed | `crates/kernel-core/src/update.rs:40` | Store versions have several spellings (leading zeros, '+'): rollback reports and audits success while the version it demoted stays active |
| sc-1 | syscall-core | low | confirmed | fixed | `kernel/src/syscall.rs:416` | Syscall fast-path (sysretq) leaks kernel register contents to Ring 3 |
| pkg-4 | packages-trust | none | refuted | low — noted | `kernel/src/platform.rs:253` | No package downgrade protection: an older genuinely signed version is launched as the active one, and install accepts it as an update; the manifest version is never compared |
