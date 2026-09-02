//! Userspace runtime for ITISYOU OS programs (Ring 3 side of the syscall
//! ABI defined in `kernel/src/syscall.rs`).

#![no_std]

pub const SYS_WRITE: u64 = 0;
pub const SYS_EXIT: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_GETPID: u64 = 3;

pub const ERR_NOSYS: u64 = u64::MAX;
pub const ERR_FAULT: u64 = u64::MAX - 1;
pub const ERR_BADF: u64 = u64::MAX - 2;

/// Raw syscall: rax=nr, rdi/rsi/rdx=args → rax. rcx/r11 are clobbered by
/// the hardware; the kernel may clobber any caller-saved register.
#[inline]
pub fn raw_syscall(nr: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") nr => ret,
            inlateout("rdi") a1 => _,
            inlateout("rsi") a2 => _,
            inlateout("rdx") a3 => _,
            out("rcx") _,
            out("r11") _,
            out("r8") _,
            out("r9") _,
            out("r10") _,
            options(nostack),
        );
    }
    ret
}

/// Write a string to stdout (fd 1). Returns bytes written or an ERR_*.
pub fn write(s: &str) -> u64 {
    raw_syscall(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64)
}

/// Write from a raw pointer/len — used by tests probing kernel validation.
pub fn write_raw(ptr: u64, len: u64) -> u64 {
    raw_syscall(SYS_WRITE, 1, ptr, len)
}

pub fn getpid() -> u64 {
    raw_syscall(SYS_GETPID, 0, 0, 0)
}

pub fn yield_now() -> u64 {
    raw_syscall(SYS_YIELD, 0, 0, 0)
}

pub fn exit(code: u64) -> ! {
    raw_syscall(SYS_EXIT, code, 0, 0);
    // The kernel never returns from exit; loop defensively regardless.
    #[allow(clippy::empty_loop)]
    loop {}
}

/// Userspace panic: report and exit with a distinctive code.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    write("RING3-PANIC\n");
    exit(101)
}
