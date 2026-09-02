# ADR-0009: Preemptive scheduling + persistent storage (NVMe writes, ITFS)

**Status:** Accepted · 2026-09-02 (V0.4)

## Context

V0.3 left two gaps before a real multitasking OS: (1) scheduling was
cooperative, so a non-yielding process could monopolize the CPU; (2) storage
was read-only, so nothing persisted. V0.4 closes both.

## Decision — preemptive scheduling

- **Full trap frame + iretq resume.** `UserContext` was expanded from the
  syscall-only callee-saved set to ALL 15 GPRs + RIP/RSP/RFLAGS, and Ring 3
  entry/resume switched from `sysretq` to `iretq`. Preemption can interrupt
  user code at any instruction, so every register must be saved and restored;
  `sysretq` clobbers rcx/r11 and cannot resume such a context, `iretq` can.
- **Naked timer ISR.** The timer interrupt is a naked handler that pushes all
  GPRs, calls a Rust body, and (if not preempting) restores + `iretq`s. When
  a running process's quantum (2 ticks = 20 ms) expires *and* the interrupt
  came from CPL=3, it copies the full trap frame into the process's
  `UserContext`, EOIs, and long-jumps to the scheduler (`UserExit::Preempted`)
  — the same setjmp-style abort path used by yield/fault, so no new stack
  discipline. Kernel-context ticks just account + iretq. Syscalls run with IF
  masked (SFMASK), so the kernel is never preempted mid-syscall.
- **Round-robin run-loop.** `Preempted` re-queues like `Yielded`;
  `run_until_pid_exits(target, max_ticks)` bounds a run so a co-scheduled
  infinite spinner cannot hang the harness.

## Decision — persistent storage

- **NVMe writes.** The read-only driver gained `write_block` (NVMe Write
  0x01) and `flush` (NVMe Flush 0x00) on the same polled I/O queue, with the
  same bounds validation as reads. The `BlockDevice` trait gained
  `write_block`/`flush` (default read-only) so higher layers are
  device-agnostic.
- **ITFS** (ADR name: ITISYOU Trivial File System): a deliberately minimal,
  correctness-first filesystem — a fixed on-disk directory (≤12 files) of
  **contiguous, append-only** files. Chosen over a B-tree/extent design
  because V0.4 needs *correctness and recoverability*, not features.
  - **Crash consistency:** a **double-buffered, CRC-protected superblock**
    (slots at block 0 and 1). A commit writes the new superblock
    (generation+1) to the slot NOT holding the committed state, after the
    file body is flushed. A torn commit leaves the other slot's
    older-but-consistent superblock intact; `mount` picks the valid slot with
    the highest generation. This gives atomic metadata commits without a
    journal.
  - **Strict validation:** magic, block size, CRC-32, and cross-checks
    (used-count vs file_count, every entry inside the data area and below
    `next_free_block`) — malformed/corrupt images are rejected, never
    trusted. Pure format logic lives in host-tested `kernel_core::itfs`.

## Alternatives

- **Preemption via a second kernel stack / green threads:** heavier; the
  abort-context long-jump already existed and generalizes cleanly.
- **FAT/ext2:** far more code and edge cases than a foundation needs; ITFS
  can be replaced behind the same block/file interface later.
- **Journal for crash consistency:** more general but heavier; double-
  buffered superblocks give atomic commits for a fixed-directory FS with a
  fraction of the complexity.

## Consequences

- A non-yielding Ring 3 process is preempted within one quantum; fairness and
  register/address-space preservation are machine-verified.
- Files written through the real block layer survive a full QEMU reboot
  (proven by a two-boot test on the same disposable disk).
- Limitations (V0.5+): no per-file update/delete/rename (append-only), a
  fixed small directory, single I/O queue / polled NVMe, cooperative-quantum
  granularity, no fsync-per-file API. The `itisyou-fs-persist` **BIOS** disk
  image does not boot (a bootloader BIOS-stage quirk specific to that
  binary); its UEFI image boots and is used for the persistence test — UEFI
  is a fully verified firmware path.
