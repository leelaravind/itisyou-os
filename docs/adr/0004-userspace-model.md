# ADR-0004: V0.2 userspace model — syscall/sysret, shared address space, abort-context containment

**Status:** Accepted · 2026-09-02

## Context

V0.2 must run a real ELF64 program in Ring 3 with working syscalls, strict
kernel/user isolation, and crash containment — without destabilizing the
verified V0.1 kernel and while remaining fully provable through the QEMU
serial harness.

## Decisions

1. **Privilege transition**: initial kernel→user entry via `iretq` (frame:
   user SS/RSP/RFLAGS(IF=1)/CS/RIP, all GPRs zeroed so no kernel data leaks);
   user→kernel via the `syscall` instruction (EFER.SCE + STAR/LSTAR/SFMASK).
   GDT layout is fixed by the sysret contract (`user_code = user_data + 8`)
   and asserted at boot. TSS RSP0 provides the kernel stack for interrupts
   and exceptions arriving at CPL=3.
2. **Address space**: V0.2 keeps ONE address space. User pages live in a
   dedicated window `[1 MiB, 512 GiB)` — entirely inside P4 entry 0, far
   from the bootloader-assigned kernel mappings (≥ 1 TiB). Isolation is
   enforced by the U/S bit: user pages carry USER_ACCESSIBLE (and their
   table path does), kernel pages never do. W^X is enforced for user
   mappings exactly as for kernel ones. Per-process page tables are V0.2+
   follow-up work (documented limitation), required before *concurrent*
   user processes.
3. **Process lifecycle**: one user process at a time, hosted by the calling
   kernel task. `enter_user` saves a setjmp-style abort context; the process
   leaves userspace only via the `exit` syscall or a contained CPL=3 fault
   (#PF/#GP/#UD), both of which long-jump back. Teardown unmaps every page
   and returns the frames. A user crash is a `UserExit::Fault` value, not a
   kernel panic.
4. **Syscall ABI** (versioned in `kernel/src/syscall.rs`, mirrored by
   `user/ulib`): rax=nr, rdi/rsi/rdx=args, rax=result; rcx/r11 hardware-
   clobbered. Syscalls run with IF masked on a dedicated kernel stack.
   `write` validates the whole user buffer (window bounds + per-page
   mapping) before any dereference; unknown numbers return ERR_NOSYS.
5. **Loader policy**: only static ET_EXEC x86_64 images (PIE deliberately
   rejected); structural validation in host-tested `kernel-core::elf`;
   policy validation (window, W^X, overlap-with-existing-mappings) in the
   kernel loader; segments mapped in three phases (map+zero → copy →
   tighten) so shared pages can never be written through tightened
   mappings.

## Alternatives

- **int 0x80-style gate** instead of syscall/sysret: slower, more moving
  parts; syscall/sysret is the x86_64-native path.
- **Per-process page tables now**: correct end-state but not required to
  prove Ring 3 + isolation for a single process; deferred with an explicit
  limitation note rather than half-implemented.
- **Running user code at CPL=0 ("fake userspace")**: prohibited by the goal;
  the design proves CPL=3 via hardware behavior (#GP on privileged
  instructions, #PF with USER_MODE error bit on kernel-half touches).

## Consequences

- Kernel data never reaches Ring 3 registers; user pointers are never
  dereferenced unvalidated; SMEP/SMAP are not enabled (QEMU model lacks
  them) — documented.
- The abort path abandons an in-flight exception frame (no iretq) by
  design; IF is re-enabled on the kernel side after every abort.
- Sibling user processes, fork/exec, IPC, and preemptive user scheduling
  build on this without replacing it (V0.2+ roadmap).
