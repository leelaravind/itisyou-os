//! Userspace processes (V0.2): ELF loading, Ring 0 → Ring 3 transition,
//! process lifecycle, and crash containment.
//!
//! Model (ADR-0004): a single user process at a time runs inside the current
//! kernel task's context. `enter_user` saves an abort context (setjmp-style),
//! then `iretq`s into Ring 3. The process leaves userspace only through the
//! `exit` syscall (→ `UserExit::Exit`) or a fault while CPL=3 (→
//! `UserExit::Fault`), both of which long-jump back into `enter_user`'s
//! frame. The kernel then unmaps the process and continues — a user crash is
//! never a kernel panic.

use crate::memory::{self, paging};
use crate::serial_println;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use kernel_core::elf::{self, ElfError};
use kernel_core::memmap::FRAME_SIZE;
use x86_64::structures::paging::{Page, PhysFrame};
use x86_64::VirtAddr;

/// User virtual window: strictly inside P4 entry 0 (0..512 GiB), far below
/// the bootloader-assigned kernel/physical-memory mappings (≥ 1 TiB in the
/// low half). Enforced for every segment page and the stack.
pub const USER_MIN: u64 = 0x0010_0000; // 1 MiB
pub const USER_MAX: u64 = 0x0080_0000_0000; // 512 GiB

/// User stack: 16 pages just under the top of the window, with an unmapped
/// gap below acting as a fault-detecting hole (not a hardened guard).
pub const USER_STACK_TOP: u64 = 0x007F_FFFF_0000;
pub const USER_STACK_PAGES: u64 = 16;

/// Why a process left userspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserExit {
    /// exit(code) syscall.
    Exit(u64),
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
    /// Target page already mapped (kernel/boot structures) — refuse to load.
    Overlap {
        vaddr: u64,
    },
    Memory(memory::PmmError),
    Paging(paging::PagingError),
}

/// A loaded (not yet running) process image.
pub struct Process {
    pub pid: u64,
    pub entry: u64,
    pub stack_top: u64,
    /// Every mapping owned by this process, for teardown.
    pages: Vec<Page>,
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

/// Load an ELF64 executable from the VFS into the user window.
pub fn load(path: &str) -> Result<Process, LoadError> {
    let bytes = crate::fs::read(path).map_err(LoadError::File)?;
    load_from_bytes(bytes)
}

/// Load an ELF64 executable from raw bytes (also used by selftests).
pub fn load_from_bytes(bytes: &[u8]) -> Result<Process, LoadError> {
    let image = elf::parse(bytes).map_err(LoadError::Elf)?;

    let mut process = Process {
        pid: NEXT_PID.fetch_add(1, Ordering::SeqCst),
        entry: image.entry,
        stack_top: USER_STACK_TOP,
        pages: Vec::new(),
    };

    let result = (|| {
        // Pass 1 — validate every segment and map all of its pages
        // writable+NX (freshly zeroed). Copying and permission tightening
        // happen only after ALL pages exist, so segments sharing a page can
        // never write through an already-tightened mapping.
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
            let first_page: Page = Page::containing_address(VirtAddr::new(start));
            let last_page: Page = Page::containing_address(VirtAddr::new(end - 1));
            for page in Page::range_inclusive(first_page, last_page) {
                map_user_page_tracked(&mut process, page, true, false)?;
            }
        }

        // Pass 2 — copy segment contents through the writable mappings.
        for segment in image.load_segments() {
            // SAFETY: pages mapped+zeroed in pass 1, exclusively owned by
            // this process; the range was bounds-checked above.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    segment.data.as_ptr(),
                    segment.vaddr as *mut u8,
                    segment.data.len(),
                );
            }
        }

        // Pass 3 — tighten every segment's pages to its real protection.
        for segment in image.load_segments() {
            let first_page: Page = Page::containing_address(VirtAddr::new(segment.vaddr));
            let last_page: Page =
                Page::containing_address(VirtAddr::new(segment.vaddr + segment.mem_size - 1));
            for page in Page::range_inclusive(first_page, last_page) {
                paging::update_user_flags(page, segment.writable, segment.executable)
                    .map_err(LoadError::Paging)?;
            }
        }

        // User stack (writable, never executable).
        let stack_low = USER_STACK_TOP - USER_STACK_PAGES * FRAME_SIZE;
        let first: Page = Page::containing_address(VirtAddr::new(stack_low));
        let last: Page = Page::containing_address(VirtAddr::new(USER_STACK_TOP - 1));
        for page in Page::range_inclusive(first, last) {
            map_user_page_tracked(&mut process, page, true, false)?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => Ok(process),
        Err(err) => {
            teardown(&mut process);
            Err(err)
        }
    }
}

fn map_user_page_tracked(
    process: &mut Process,
    page: Page,
    writable: bool,
    executable: bool,
) -> Result<(), LoadError> {
    let vaddr = page.start_address().as_u64();
    if !(USER_MIN..USER_MAX).contains(&vaddr) {
        return Err(LoadError::OutsideUserWindow { vaddr });
    }
    if !paging::is_unmapped(page.start_address()) {
        // Already mapped by this process (overlapping segments share pages)?
        if process.pages.contains(&page) {
            return Ok(());
        }
        // Mapped by someone else (kernel/boot structures): refuse.
        return Err(LoadError::Overlap { vaddr });
    }
    let frame = memory::alloc_frame().map_err(LoadError::Memory)?;
    match paging::map_user_page(page, frame, writable, executable) {
        Ok(()) => {
            // Fresh frames must never leak previous contents to userspace.
            // SAFETY: just mapped writable, exclusively owned.
            unsafe {
                core::ptr::write_bytes(
                    page.start_address().as_mut_ptr::<u8>(),
                    0,
                    FRAME_SIZE as usize,
                );
            }
            process.pages.push(page);
            Ok(())
        }
        Err(e) => {
            let _ = memory::free_frame(frame);
            Err(LoadError::Paging(e))
        }
    }
}

/// Unmap every page of the process and return its frames to the PMM.
fn teardown(process: &mut Process) {
    for page in process.pages.drain(..) {
        if let Ok(frame) = paging::unmap_page(page) {
            let frame: PhysFrame = frame;
            let _ = memory::free_frame(frame);
        }
    }
}

/// Run a loaded process to completion (exit or contained fault), then tear
/// it down. Returns how it ended.
pub fn run(mut process: Process) -> UserExit {
    serial_println!(
        "[ITISYOU:INFO] user_enter pid={} entry={:#x} stack={:#x} ring=3",
        process.pid,
        process.entry,
        process.stack_top,
    );
    crate::syscall::CURRENT_PID.store(process.pid, core::sync::atomic::Ordering::SeqCst);
    let exit = transition::enter_user(process.entry, process.stack_top);
    crate::syscall::CURRENT_PID.store(0, core::sync::atomic::Ordering::SeqCst);
    match exit {
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
    }
    teardown(&mut process);
    exit
}

/// Convenience: load + run a program from the VFS.
pub fn run_path(path: &str) -> Result<UserExit, LoadError> {
    let process = load(path)?;
    Ok(run(process))
}

/// Ring transition + abort plumbing.
pub mod transition {
    use super::UserExit;
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

    /// Enter Ring 3 at `entry` with `user_rsp`. Returns when the process
    /// exits or faults.
    pub fn enter_user(entry: u64, user_rsp: u64) -> UserExit {
        FAULT_HAS_ADDR.store(false, Ordering::SeqCst);
        IN_USER.store(true, Ordering::SeqCst);
        // SAFETY: entry/stack point into freshly mapped user pages; the
        // abort context is armed before any Ring 3 instruction runs.
        let packed = unsafe { enter_user_raw(entry, user_rsp) };
        IN_USER.store(false, Ordering::SeqCst);
        // Both abort paths (exit syscall, contained fault) arrive with IF
        // masked; kernel steady-state runs with interrupts enabled.
        x86_64::instructions::interrupts::enable();

        if packed & TAG_FAULT != 0 {
            let vector = (packed & 0xFF) as u8;
            let addr = FAULT_HAS_ADDR
                .load(Ordering::SeqCst)
                .then(|| FAULT_ADDR.load(Ordering::SeqCst));
            UserExit::Fault { vector, addr }
        } else {
            UserExit::Exit(packed & 0xFFFF_FFFF)
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

    /// Save callee-saved context, build an iretq frame, zero every GPR (no
    /// kernel data may leak into Ring 3), and enter userspace.
    ///
    /// RFLAGS for user: IF set (0x202) so the timer keeps running at CPL=3.
    #[unsafe(naked)]
    unsafe extern "C" fn enter_user_raw(entry: u64, user_rsp: u64) -> u64 {
        naked_asm!(
            "lea rax, [rip + 2f]",
            "mov [rip + USER_ABORT_CTX + 0], rax",
            "mov [rip + USER_ABORT_CTX + 8], rsp",
            "mov [rip + USER_ABORT_CTX + 16], rbp",
            "mov [rip + USER_ABORT_CTX + 24], rbx",
            "mov [rip + USER_ABORT_CTX + 32], r12",
            "mov [rip + USER_ABORT_CTX + 40], r13",
            "mov [rip + USER_ABORT_CTX + 48], r14",
            "mov [rip + USER_ABORT_CTX + 56], r15",
            // iretq frame: SS, RSP, RFLAGS, CS, RIP
            "push {uss}",
            "push rsi",
            "push 0x202",
            "push {ucs}",
            "push rdi",
            // No kernel register contents may reach Ring 3.
            "xor eax, eax",
            "xor ebx, ebx",
            "xor ecx, ecx",
            "xor edx, edx",
            "xor esi, esi",
            "xor edi, edi",
            "xor ebp, ebp",
            "xor r8d, r8d",
            "xor r9d, r9d",
            "xor r10d, r10d",
            "xor r11d, r11d",
            "xor r12d, r12d",
            "xor r13d, r13d",
            "xor r14d, r14d",
            "xor r15d, r15d",
            "iretq",
            // Abort lands here with rax = packed reason.
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
