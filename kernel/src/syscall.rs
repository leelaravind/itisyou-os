//! Syscall ABI + `syscall`/`sysret` fast path (V0.2, ADR-0004).
//!
//! ABI (registers): rax = number, rdi/rsi/rdx = args, rax = return value.
//! rcx and r11 are clobbered by the hardware (return RIP / RFLAGS).
//!
//! | nr | name   | args                  | returns                       |
//! |----|--------|-----------------------|-------------------------------|
//! | 0  | write  | fd(=1), ptr, len      | bytes written, or ERR_*       |
//! | 1  | exit   | code                  | does not return               |
//! | 2  | yield  | —                     | 0                             |
//! | 3  | getpid | —                     | pid                           |
//!
//! Unknown numbers return [`ERR_NOSYS`]; invalid user pointers return
//! [`ERR_FAULT`] after validation — the kernel never dereferences unvalidated
//! user addresses. Syscalls execute with interrupts masked (SFMASK) on a
//! dedicated kernel stack; single CPU, no nesting (yield re-enables IF only
//! while parked in the kernel scheduler).

use crate::user::transition;
use crate::user::{USER_MAX, USER_MIN};
use core::arch::naked_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

pub const SYS_WRITE: u64 = 0;
pub const SYS_EXIT: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_GETPID: u64 = 3;

pub const ERR_NOSYS: u64 = u64::MAX;
pub const ERR_FAULT: u64 = u64::MAX - 1;
pub const ERR_BADF: u64 = u64::MAX - 2;

/// Cap for a single write so a hostile length cannot stall the kernel.
const WRITE_MAX: u64 = 64 * 1024;

const KSTACK_SIZE: usize = 32 * 1024;

// Referenced by name from the naked entry stub.
#[unsafe(no_mangle)]
static mut SYSCALL_USER_RSP: u64 = 0;
#[unsafe(no_mangle)]
static mut SYSCALL_KSTACK_TOP: u64 = 0;

static mut SYSCALL_STACK: [u8; KSTACK_SIZE] = [0; KSTACK_SIZE];

/// PID of the currently running user process (0 = none).
pub static CURRENT_PID: AtomicU64 = AtomicU64::new(0);

/// Count of rejected unknown-syscall attempts (diagnostic evidence).
pub static NOSYS_COUNT: AtomicU64 = AtomicU64::new(0);

/// Program the syscall MSRs (B140). Requires gdt::init() done.
pub fn init() {
    let selectors = crate::gdt::selectors();
    // SAFETY: standard syscall MSR setup; selector layout is asserted by
    // gdt::init; the entry stub upholds the syscall ABI contract.
    unsafe {
        Efer::update(|flags| flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
        Star::write(
            selectors.user_code,
            selectors.user_data,
            selectors.kernel_code,
            selectors.kernel_data,
        )
        .expect("GDT layout incompatible with sysret");
        let entry: unsafe extern "C" fn() = syscall_entry;
        LStar::write(VirtAddr::new(entry as usize as u64));
        // Mask IF (and TF/DF/AC) while in the kernel syscall path.
        SFMask::write(
            RFlags::INTERRUPT_FLAG
                | RFlags::TRAP_FLAG
                | RFlags::DIRECTION_FLAG
                | RFlags::ALIGNMENT_CHECK,
        );
        let base = (&raw const SYSCALL_STACK) as u64;
        let top = (base + KSTACK_SIZE as u64) & !0xF;
        core::ptr::write(&raw mut SYSCALL_KSTACK_TOP, top);
    }
}

/// Hardware entry point. On entry: rcx = user RIP, r11 = user RFLAGS,
/// rsp = USER stack (untrusted!), IF masked via SFMASK.
///
/// Switches to the kernel syscall stack, preserves the user return context,
/// calls the Rust dispatcher, restores, `sysretq`s.
#[unsafe(naked)]
unsafe extern "C" fn syscall_entry() {
    naked_asm!(
        "mov [rip + SYSCALL_USER_RSP], rsp",
        "mov rsp, [rip + SYSCALL_KSTACK_TOP]",
        "push rcx", // user RIP
        "push r11", // user RFLAGS
        "push qword ptr [rip + SYSCALL_USER_RSP]",
        "sub rsp, 8", // 16-byte alignment for the call
        // dispatcher(a1=rdi, a2=rsi, a3=rdx, nr=rcx)
        "mov rcx, rax",
        "call {dispatch}",
        "add rsp, 8",
        "pop qword ptr [rip + SYSCALL_USER_RSP]",
        "pop r11",
        "pop rcx",
        "mov rsp, [rip + SYSCALL_USER_RSP]",
        "sysretq",
        dispatch = sym syscall_dispatch,
    )
}

extern "C" fn syscall_dispatch(a1: u64, a2: u64, a3: u64, nr: u64) -> u64 {
    match nr {
        SYS_WRITE => sys_write(a1, a2, a3),
        SYS_EXIT => transition::abort_exit(a1),
        SYS_YIELD => {
            // Park in the kernel scheduler with interrupts enabled, then
            // return to the masked syscall path.
            x86_64::instructions::interrupts::enable();
            crate::task::yield_now();
            x86_64::instructions::interrupts::disable();
            0
        }
        SYS_GETPID => CURRENT_PID.load(Ordering::SeqCst),
        _ => {
            NOSYS_COUNT.fetch_add(1, Ordering::SeqCst);
            ERR_NOSYS
        }
    }
}

/// write(fd, ptr, len) — fd 1 (serial stdout) only. The [ptr, ptr+len)
/// range must lie entirely inside the user window and be mapped; validated
/// page-by-page before any byte is read.
fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if fd != 1 {
        return ERR_BADF;
    }
    if len == 0 {
        return 0;
    }
    if len > WRITE_MAX {
        return ERR_FAULT;
    }
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return ERR_FAULT,
    };
    if ptr < USER_MIN || end > USER_MAX {
        return ERR_FAULT;
    }
    // Every touched page must be mapped (user pages are USER_ACCESSIBLE by
    // construction; kernel pages can never appear in the user window).
    let mut page_addr = ptr & !0xFFF;
    while page_addr < end {
        if crate::memory::paging::translate(VirtAddr::new(page_addr)).is_none() {
            return ERR_FAULT;
        }
        page_addr += 4096;
    }
    // SAFETY: range validated above; single CPU and the owning process is
    // suspended in this very syscall, so the mapping cannot change.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    for chunk in bytes.utf8_chunks() {
        crate::serial_print!("{}", chunk.valid());
        if !chunk.invalid().is_empty() {
            crate::serial_print!("\u{FFFD}");
        }
    }
    len
}
