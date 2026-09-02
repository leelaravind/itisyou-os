//! Userspace processes: ELF loading, per-process address spaces (V0.3),
//! Ring 0 ↔ Ring 3 transitions, resumable contexts, and crash containment.
//!
//! Model (ADR-0004/0006): each process owns an [`AddressSpace`] and a
//! resumable [`UserContext`]. `enter_or_resume` saves a kernel abort context
//! (setjmp-style) and `sysretq`s into Ring 3 — the same path serves the
//! first entry and every later resume. A process leaves Ring 3 via the
//! `exit` syscall (→ `Exit`), the `yield` syscall (→ `Yielded`, context
//! saved for resume), or a CPL=3 fault (→ `Fault`); all long-jump back to
//! the kernel run-loop. A user crash is never a kernel panic.

use crate::memory::aspace::{activate_l4, AddressSpace, AspaceError};
use crate::memory::paging;
use crate::serial_println;
use core::sync::atomic::{AtomicU64, Ordering};
use kernel_core::elf::{self, ElfError};
use kernel_core::memmap::FRAME_SIZE;

/// Complete resumable Ring 3 register state (V0.4). All 15 GPRs plus
/// RIP/RSP/RFLAGS are tracked so a process can be resumed from ANY
/// instruction — required for timer preemption, which can interrupt user
/// code at an arbitrary point. Resume is via `iretq` (does not clobber
/// rcx/r11 the way sysretq does).
///
/// `#[repr(C)]` is REQUIRED: `enter_user_raw` reads these fields by fixed
/// byte offsets — rax=0, rbx=8, rcx=16, rdx=24, rsi=32, rdi=40, rbp=48,
/// r8=56, r9=64, r10=72, r11=80, r12=88, r13=96, r14=104, r15=112,
/// rip=120, rsp=128, rflags=136. Keep the field order in lockstep with the
/// asm.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct UserContext {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
}

impl UserContext {
    /// Fresh context: enter `entry` on `stack_top`, IF set so the timer
    /// preempts at CPL=3, all general registers zeroed (no kernel data
    /// reaches Ring 3).
    pub fn new(entry: u64, stack_top: u64) -> Self {
        UserContext {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: entry,
            rsp: stack_top,
            rflags: 0x202,
        }
    }
}

/// User virtual window: strictly inside P4 entry 0 (0..512 GiB), far below
/// the bootloader-assigned kernel/physical-memory mappings (≥ 1 TiB in the
/// low half). Enforced for every segment page and the stack.
pub const USER_MIN: u64 = 0x0010_0000; // 1 MiB
pub const USER_MAX: u64 = 0x0080_0000_0000; // 512 GiB

/// User stack: 16 pages just under the top of the window, with an unmapped
/// gap below acting as a fault-detecting hole (not a hardened guard).
pub const USER_STACK_TOP: u64 = 0x007F_FFFF_0000;
pub const USER_STACK_PAGES: u64 = 16;

/// Why a process left userspace this quantum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserExit {
    /// exit(code) syscall.
    Exit(u64),
    /// yield syscall — context saved; re-enterable immediately.
    Yielded,
    /// timer preemption — full context saved; re-enterable immediately
    /// (round-robin). Distinct from Yielded only for diagnostics/accounting.
    Preempted,
    /// wait syscall on a still-running child — context saved; re-enterable
    /// only when the run-loop wakes it (rax delivered = child status).
    Blocked,
    /// Hardware fault while CPL=3 (vector; CR2 for page faults).
    Fault { vector: u8, addr: Option<u64> },
}

/// Load failure (all are safe rejections).
#[derive(Debug)]
pub enum LoadError {
    File(crate::fs::FsError),
    Elf(ElfError),
    /// Segment or stack page outside [USER_MIN, USER_MAX).
    OutsideUserWindow {
        vaddr: u64,
    },
    /// Writable+executable segment — W^X policy (plan §11.2).
    WxSegment {
        vaddr: u64,
    },
    Space(AspaceError),
}

/// A loaded process image with its private address space (V0.3, ADR-0005)
/// and resumable context.
pub struct Process {
    pub pid: u64,
    pub entry: u64,
    pub stack_top: u64,
    pub space: AddressSpace,
    pub ctx: UserContext,
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

/// Load an ELF64 executable from the VFS into the user window.
pub fn load(path: &str) -> Result<Process, LoadError> {
    let bytes = crate::fs::read(path).map_err(LoadError::File)?;
    load_from_bytes(bytes)
}

/// Load an ELF64 executable from raw bytes into a fresh private address
/// space (also used by selftests).
pub fn load_from_bytes(bytes: &[u8]) -> Result<Process, LoadError> {
    let image = elf::parse(bytes).map_err(LoadError::Elf)?;
    let space = AddressSpace::new().map_err(LoadError::Space)?;

    let mut process = Process {
        pid: NEXT_PID.fetch_add(1, Ordering::SeqCst),
        entry: image.entry,
        stack_top: USER_STACK_TOP,
        space,
        ctx: UserContext::new(image.entry, USER_STACK_TOP),
    };

    let result = (|| {
        // Pass 1 — validate every segment and map its pages writable+NX,
        // freshly zeroed. Copying and tightening happen only after ALL
        // pages exist, so segments sharing a page can never write through
        // an already-tightened mapping.
        for segment in image.load_segments() {
            if segment.writable && segment.executable {
                return Err(LoadError::WxSegment {
                    vaddr: segment.vaddr,
                });
            }
            let start = segment.vaddr;
            let end = start
                .checked_add(segment.mem_size)
                .ok_or(LoadError::OutsideUserWindow { vaddr: start })?;
            if start < USER_MIN || end > USER_MAX || segment.mem_size == 0 {
                return Err(LoadError::OutsideUserWindow { vaddr: start });
            }
            map_range(&mut process.space, start, end)?;
        }

        // Pass 2 — copy contents through the physical alias (no CR3 switch
        // needed; works whether or not this space is active).
        for segment in image.load_segments() {
            copy_into_space(&process.space, segment.vaddr, segment.data);
        }

        // Pass 3 — tighten every segment's pages to its real protection.
        for segment in image.load_segments() {
            let mut page = segment.vaddr & !(FRAME_SIZE - 1);
            let end = segment.vaddr + segment.mem_size;
            while page < end {
                process
                    .space
                    .update_user_flags(page, segment.writable, segment.executable)
                    .map_err(LoadError::Space)?;
                page += FRAME_SIZE;
            }
        }

        // User stack (writable, never executable).
        let stack_low = USER_STACK_TOP - USER_STACK_PAGES * FRAME_SIZE;
        map_range(&mut process.space, stack_low, USER_STACK_TOP)?;
        Ok(())
    })();

    match result {
        Ok(()) => Ok(process),
        Err(err) => {
            process.space.teardown();
            Err(err)
        }
    }
}

/// Map every page intersecting [start, end) writable+NX, skipping pages this
/// space already mapped (overlapping segments).
fn map_range(space: &mut AddressSpace, start: u64, end: u64) -> Result<(), LoadError> {
    let mut page = start & !(FRAME_SIZE - 1);
    while page < end {
        if !(USER_MIN..USER_MAX).contains(&page) {
            return Err(LoadError::OutsideUserWindow { vaddr: page });
        }
        match space.map_user_page(page, true, false) {
            Ok(_) => {}
            // Already mapped in THIS fresh space == ours (shared page).
            Err(AspaceError::AlreadyMapped { .. }) => {}
            Err(e) => return Err(LoadError::Space(e)),
        }
        page += FRAME_SIZE;
    }
    Ok(())
}

/// Copy `data` to `vaddr` inside `space` via the physical-memory alias.
fn copy_into_space(space: &AddressSpace, vaddr: u64, data: &[u8]) {
    let mut off: usize = 0;
    while off < data.len() {
        let va = vaddr + off as u64;
        let page_base = va & !(FRAME_SIZE - 1);
        let page_off = (va - page_base) as usize;
        let n = core::cmp::min(FRAME_SIZE as usize - page_off, data.len() - off);
        let phys = space
            .translate(page_base)
            .expect("segment page mapped in pass 1");
        // SAFETY: destination frame belongs exclusively to this space and
        // was mapped+zeroed in pass 1; n is bounded to the page.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr().add(off),
                paging::phys_to_virt(phys).add(page_off),
                n,
            );
        }
        off += n;
    }
}

/// Run one process for one quantum in its own address space: activate its
/// CR3, enter/resume Ring 3, restore the kernel CR3 on return. Updates the
/// process's saved context on yield. Does NOT tear down — the caller (the
/// process scheduler) owns lifecycle.
pub fn run_quantum(process: &mut Process, first: bool) -> UserExit {
    if first {
        serial_println!(
            "[ITISYOU:INFO] user_enter pid={} entry={:#x} stack={:#x} ring=3 l4={:#x}",
            process.pid,
            process.entry,
            process.stack_top,
            process.space.l4_phys(),
        );
    }
    crate::syscall::CURRENT_PID.store(process.pid, Ordering::SeqCst);
    activate_l4(process.space.l4_phys());
    // Arm the preemption quantum for this slice; the timer decrements it
    // while this process runs at CPL=3 and preempts when it hits zero.
    crate::interrupts::arm_quantum();
    let exit = transition::enter_or_resume(&mut process.ctx);
    crate::interrupts::disarm_quantum();
    activate_l4(paging::boot_l4_frame());
    crate::syscall::CURRENT_PID.store(0, Ordering::SeqCst);
    exit
}

/// Run a loaded process to completion in its own address space (draining
/// cooperative yields), then tear it down. Returns its terminal outcome.
pub fn run(mut process: Process) -> UserExit {
    let mut first = true;
    let terminal = loop {
        match run_quantum(&mut process, first) {
            // A lone process yielding, preempted, or (mis)using wait with no
            // scheduler simply re-runs; tested single programs do none of
            // these indefinitely.
            UserExit::Yielded | UserExit::Preempted | UserExit::Blocked => {
                first = false;
                continue;
            }
            other => break other,
        }
    };
    match terminal {
        UserExit::Exit(code) => {
            serial_println!("[ITISYOU:INFO] user_exit pid={} code={}", process.pid, code);
        }
        UserExit::Fault { vector, addr } => {
            serial_println!(
                "[ITISYOU:INFO] user_fault pid={} vector={} addr={:#x} contained=true",
                process.pid,
                vector,
                addr.unwrap_or(0),
            );
        }
        UserExit::Yielded | UserExit::Preempted | UserExit::Blocked => unreachable!(),
    }
    process.space.teardown();
    terminal
}

/// Convenience: load + run a program from the VFS.
pub fn run_path(path: &str) -> Result<UserExit, LoadError> {
    let process = load(path)?;
    Ok(run(process))
}

/// Discard a loaded-but-never-run process, freeing its address space.
pub fn discard(process: Process) {
    process.space.teardown();
}

/// Ring transition + abort plumbing.
pub mod transition {
    use super::{UserContext, UserExit};
    use core::arch::naked_asm;
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// Saved kernel continuation: [rip, rsp, rbp, rbx, r12, r13, r14, r15].
    #[unsafe(no_mangle)]
    static mut USER_ABORT_CTX: [u64; 8] = [0; 8];

    /// True while CPL=3 execution (or a syscall on its behalf) is possible —
    /// consulted by fault handlers to decide user-fault containment.
    pub static IN_USER: AtomicBool = AtomicBool::new(false);

    /// Fault detail set by the exception handlers before aborting.
    pub static FAULT_ADDR: AtomicU64 = AtomicU64::new(0);
    pub static FAULT_HAS_ADDR: AtomicBool = AtomicBool::new(false);

    const TAG_EXIT: u64 = 1 << 56;
    const TAG_FAULT: u64 = 2 << 56;
    const TAG_YIELD: u64 = 3 << 56;
    const TAG_BLOCK: u64 = 4 << 56;
    const TAG_PREEMPT: u64 = 5 << 56;

    /// Pointer to the UserContext of the process currently in Ring 3 (or
    /// about to be), so syscalls and the timer can persist a resume point.
    static CURRENT_CTX: AtomicU64 = AtomicU64::new(0);

    /// The running process's context pointer (null if none). Used by the
    /// timer preemption path.
    pub fn current_ctx() -> *mut UserContext {
        CURRENT_CTX.load(Ordering::SeqCst) as *mut UserContext
    }

    /// Save the resumable snapshot taken at syscall entry into the current
    /// process's context, delivering `rax` to it on resume. Caller-saved
    /// registers are dead across a syscall, so only callee-saved + control
    /// registers + rax are updated.
    pub fn save_yield_context(rax: u64) {
        let ctx_ptr = CURRENT_CTX.load(Ordering::SeqCst) as *mut UserContext;
        if ctx_ptr.is_null() {
            return;
        }
        let (snap, user_rsp) = crate::syscall::snapshot();
        // SAFETY: ctx_ptr was set by enter_or_resume for the running process
        // and stays valid until it returns; single CPU serializes access.
        let ctx = unsafe { &mut *ctx_ptr };
        ctx.rip = snap[0];
        ctx.rflags = snap[1];
        ctx.rbx = snap[2];
        ctx.rbp = snap[3];
        ctx.r12 = snap[4];
        ctx.r13 = snap[5];
        ctx.r14 = snap[6];
        ctx.r15 = snap[7];
        ctx.rsp = user_rsp;
        ctx.rax = rax;
    }

    /// Return control to the run-loop as a preemption. The full context has
    /// already been written into the current process's `UserContext` by the
    /// timer handler; this only long-jumps back.
    pub fn abort_preempt() -> ! {
        // SAFETY: only called from the timer ISR while a user process runs
        // (CPL=3 verified) with the abort context armed.
        unsafe { user_abort_raw(TAG_PREEMPT) }
    }

    /// Enter (first time) or resume Ring 3 from `ctx`. Returns when the
    /// process yields (ctx updated for the next resume), exits, or faults.
    pub fn enter_or_resume(ctx: &mut UserContext) -> UserExit {
        FAULT_HAS_ADDR.store(false, Ordering::SeqCst);
        CURRENT_CTX.store(ctx as *mut UserContext as u64, Ordering::SeqCst);
        IN_USER.store(true, Ordering::SeqCst);
        // SAFETY: ctx describes a valid Ring 3 state (fresh entry or a state
        // saved by a prior yield); the abort context is armed before any
        // Ring 3 instruction runs.
        let packed = unsafe { enter_user_raw(ctx as *mut UserContext) };
        IN_USER.store(false, Ordering::SeqCst);
        CURRENT_CTX.store(0, Ordering::SeqCst);
        // Abort paths arrive with IF masked; kernel steady-state runs with
        // interrupts enabled.
        x86_64::instructions::interrupts::enable();

        match packed >> 56 {
            5 => UserExit::Preempted,
            4 => UserExit::Blocked,
            3 => UserExit::Yielded,
            2 => {
                let vector = (packed & 0xFF) as u8;
                let addr = FAULT_HAS_ADDR
                    .load(Ordering::SeqCst)
                    .then(|| FAULT_ADDR.load(Ordering::SeqCst));
                UserExit::Fault { vector, addr }
            }
            _ => UserExit::Exit(packed & 0xFFFF_FFFF),
        }
    }

    /// Abort userspace execution with an exit code (from the exit syscall).
    pub fn abort_exit(code: u64) -> ! {
        // SAFETY: only reachable while USER_ABORT_CTX is armed (IN_USER).
        unsafe { user_abort_raw(TAG_EXIT | (code & 0xFFFF_FFFF)) }
    }

    /// Abort userspace execution because of a fault (from exception handlers).
    pub fn abort_fault(vector: u8) -> ! {
        // SAFETY: as above; handlers verified CPL=3 origin first.
        unsafe { user_abort_raw(TAG_FAULT | vector as u64) }
    }

    /// Save the current process's resumable context (snapshotted at syscall
    /// entry) and return control to the run-loop as a yield.
    pub fn abort_yield() -> ! {
        // SAFETY: only called from the yield syscall while IN_USER.
        unsafe { user_abort_raw(TAG_YIELD) }
    }

    /// Return control to the run-loop as a block (the context has already
    /// been saved by the caller); the process is not re-queued until woken.
    pub fn abort_block() -> ! {
        // SAFETY: only called from the wait syscall while IN_USER.
        unsafe { user_abort_raw(TAG_BLOCK) }
    }

    /// Enter/resume Ring 3 via `iretq` (used for BOTH first entry and every
    /// resume — including timer preemption, which requires restoring ALL
    /// GPRs and does not survive sysretq's rcx/r11 clobber). Every register
    /// delivered to Ring 3 comes from `ctx` (zeroed on first entry), so no
    /// kernel data leaks. Offsets track the `UserContext` field order.
    #[unsafe(naked)]
    unsafe extern "C" fn enter_user_raw(ctx: *mut UserContext) -> u64 {
        naked_asm!(
            // Save kernel callee-saved + a return label into the abort ctx.
            "lea rax, [rip + 2f]",
            "mov [rip + USER_ABORT_CTX + 0], rax",
            "mov [rip + USER_ABORT_CTX + 8], rsp",
            "mov [rip + USER_ABORT_CTX + 16], rbp",
            "mov [rip + USER_ABORT_CTX + 24], rbx",
            "mov [rip + USER_ABORT_CTX + 32], r12",
            "mov [rip + USER_ABORT_CTX + 40], r13",
            "mov [rip + USER_ABORT_CTX + 48], r14",
            "mov [rip + USER_ABORT_CTX + 56], r15",
            // Build the iretq frame from ctx (rdi): SS, RSP, RFLAGS, CS, RIP.
            "push {uss}",
            "push qword ptr [rdi + 128]", // user RSP
            "push qword ptr [rdi + 136]", // user RFLAGS
            "push {ucs}",
            "push qword ptr [rdi + 120]", // user RIP
            // Restore all GPRs from ctx (rdi restored LAST).
            "mov rax, [rdi + 0]",
            "mov rbx, [rdi + 8]",
            "mov rcx, [rdi + 16]",
            "mov rdx, [rdi + 24]",
            "mov rsi, [rdi + 32]",
            "mov rbp, [rdi + 48]",
            "mov r8,  [rdi + 56]",
            "mov r9,  [rdi + 64]",
            "mov r10, [rdi + 72]",
            "mov r11, [rdi + 80]",
            "mov r12, [rdi + 88]",
            "mov r13, [rdi + 96]",
            "mov r14, [rdi + 104]",
            "mov r15, [rdi + 112]",
            "mov rdi, [rdi + 40]",
            "iretq",
            // Abort/yield/preempt lands here with rax = packed reason.
            "2:",
            "ret",
            uss = const super::super::gdt::USER_DATA_SELECTOR as u64,
            ucs = const super::super::gdt::USER_CODE_SELECTOR as u64,
        )
    }

    /// Long-jump back into `enter_user_raw`'s caller frame.
    #[unsafe(naked)]
    unsafe extern "C" fn user_abort_raw(packed: u64) -> ! {
        naked_asm!(
            "mov rax, rdi",
            "mov rsp, [rip + USER_ABORT_CTX + 8]",
            "mov rbp, [rip + USER_ABORT_CTX + 16]",
            "mov rbx, [rip + USER_ABORT_CTX + 24]",
            "mov r12, [rip + USER_ABORT_CTX + 32]",
            "mov r13, [rip + USER_ABORT_CTX + 40]",
            "mov r14, [rip + USER_ABORT_CTX + 48]",
            "mov r15, [rip + USER_ABORT_CTX + 56]",
            "jmp [rip + USER_ABORT_CTX + 0]",
        )
    }
}
