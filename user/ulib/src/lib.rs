//! Userspace runtime for ITISYOU OS programs (Ring 3 side of the syscall
//! ABI defined in `kernel/src/syscall.rs`).

#![no_std]

pub const SYS_WRITE: u64 = 0;
pub const SYS_EXIT: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_GETPID: u64 = 3;
pub const SYS_SPAWN: u64 = 4;
pub const SYS_WAIT: u64 = 5;
pub const SYS_MSG_SEND: u64 = 6;
pub const SYS_MSG_RECV: u64 = 7;

pub const ERR_NOSYS: u64 = u64::MAX;
pub const ERR_FAULT: u64 = u64::MAX - 1;
pub const ERR_BADF: u64 = u64::MAX - 2;
pub const ERR_NOENT: u64 = u64::MAX - 3;
pub const ERR_AGAIN: u64 = u64::MAX - 4;
pub const ERR_INVAL: u64 = u64::MAX - 5;
pub const ERR_2BIG: u64 = u64::MAX - 6;

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

/// Spawn a child from an initramfs path. Returns child pid or ERR_*.
pub fn spawn(path: &str) -> u64 {
    raw_syscall(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0)
}

/// Wait for a child; returns its encoded status (exit code, or 0x1_0000_0000
/// | vector for a fault) or ERR_*.
pub fn wait(pid: u64) -> u64 {
    raw_syscall(SYS_WAIT, pid, 0, 0)
}

/// Send a message on a channel. Returns bytes queued or ERR_*.
pub fn msg_send(channel: u64, data: &[u8]) -> u64 {
    raw_syscall(
        SYS_MSG_SEND,
        channel,
        data.as_ptr() as u64,
        data.len() as u64,
    )
}

/// Receive a message into `buf`. Returns bytes written or ERR_*.
pub fn msg_recv(channel: u64, buf: &mut [u8]) -> u64 {
    raw_syscall(
        SYS_MSG_RECV,
        channel,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    )
}

pub fn exit(code: u64) -> ! {
    raw_syscall(SYS_EXIT, code, 0, 0);
    // The kernel never returns from exit; loop defensively regardless.
    #[allow(clippy::empty_loop)]
    loop {}
}

/// Write a small unsigned integer as decimal (no allocator in userspace).
pub fn write_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if v == 0 {
        write("0");
        return;
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    // SAFETY: buf[i..] is ASCII digits.
    write(unsafe { core::str::from_utf8_unchecked(&buf[i..]) });
}

/// Userspace panic: report and exit with a distinctive code.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    write("RING3-PANIC\n");
    exit(101)
}
