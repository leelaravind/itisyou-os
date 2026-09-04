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
pub const SYS_GUI_CREATE: u64 = 8;
pub const SYS_GUI_FILL: u64 = 9;
pub const SYS_GUI_TEXT: u64 = 10;
pub const SYS_GUI_PRESENT: u64 = 11;
pub const SYS_DEVINFO: u64 = 12;
pub const SYS_FS_READ: u64 = 13;
pub const SYS_SPAWN_CAPS: u64 = 14;
pub const SYS_CAP_LIST: u64 = 15;
pub const SYS_CAP_CHECK: u64 = 16;

pub const ERR_NOSYS: u64 = u64::MAX;
pub const ERR_FAULT: u64 = u64::MAX - 1;
pub const ERR_BADF: u64 = u64::MAX - 2;
pub const ERR_NOENT: u64 = u64::MAX - 3;
pub const ERR_AGAIN: u64 = u64::MAX - 4;
pub const ERR_INVAL: u64 = u64::MAX - 5;
pub const ERR_2BIG: u64 = u64::MAX - 6;
pub const ERR_PERM: u64 = u64::MAX - 7;

/// Capability bits (mirror kernel_core::caps — keep in lockstep).
pub const CAP_SPAWN: u64 = 1 << 0;
pub const CAP_IPC: u64 = 1 << 1;
pub const CAP_GUI: u64 = 1 << 2;
pub const CAP_DEV: u64 = 1 << 3;
pub const CAP_FS_READ: u64 = 1 << 4;
pub const CAP_AUDIO: u64 = 1 << 5;
pub const CAP_SYS_ADMIN: u64 = 1 << 6;
pub const CAP_FS_WRITE: u64 = 1 << 7;
pub const CAP_NETWORK: u64 = 1 << 8;
pub const CAP_PROC_CONTROL: u64 = 1 << 9;
pub const CAP_SERVICE: u64 = 1 << 10;

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

/// Create a GUI window at (x,y) sized w×h with a title. Returns the window
/// id or an ERR_*.
pub fn gui_create(w: u32, h: u32, x: u32, y: u32, title: &str) -> u64 {
    let mut req = [0u8; 64];
    req[0..4].copy_from_slice(&w.to_le_bytes());
    req[4..8].copy_from_slice(&h.to_le_bytes());
    req[8..12].copy_from_slice(&x.to_le_bytes());
    req[12..16].copy_from_slice(&y.to_le_bytes());
    let t = title.as_bytes();
    let n = core::cmp::min(t.len(), req.len() - 16);
    req[16..16 + n].copy_from_slice(&t[..n]);
    raw_syscall(SYS_GUI_CREATE, req.as_ptr() as u64, (16 + n) as u64, 0)
}

/// Fill a rectangle inside a window (window-local coords).
pub fn gui_fill(win: u64, x: u32, y: u32, w: u32, h: u32, color: u32) -> u64 {
    let mut req = [0u8; 20];
    req[0..4].copy_from_slice(&x.to_le_bytes());
    req[4..8].copy_from_slice(&y.to_le_bytes());
    req[8..12].copy_from_slice(&w.to_le_bytes());
    req[12..16].copy_from_slice(&h.to_le_bytes());
    req[16..20].copy_from_slice(&color.to_le_bytes());
    raw_syscall(SYS_GUI_FILL, win, req.as_ptr() as u64, req.len() as u64)
}

/// Draw text inside a window (window-local coords).
pub fn gui_text(win: u64, x: u32, y: u32, color: u32, text: &str) -> u64 {
    let mut req = [0u8; 64];
    req[0..4].copy_from_slice(&x.to_le_bytes());
    req[4..8].copy_from_slice(&y.to_le_bytes());
    req[8..12].copy_from_slice(&color.to_le_bytes());
    let t = text.as_bytes();
    let n = core::cmp::min(t.len(), req.len() - 12);
    req[12..12 + n].copy_from_slice(&t[..n]);
    raw_syscall(SYS_GUI_TEXT, win, req.as_ptr() as u64, (12 + n) as u64)
}

/// Composite and present the scene (the caller must own `win`).
pub fn gui_present(win: u64) -> u64 {
    raw_syscall(SYS_GUI_PRESENT, win, 0, 0)
}

/// Read a VFS file into `buf` (requires CAP_FS_READ; subject to the process's
/// sandbox path prefixes). Returns bytes copied or ERR_*.
pub fn fs_read(path: &str, buf: &mut [u8]) -> u64 {
    let req: [u64; 2] = [buf.as_mut_ptr() as u64, buf.len() as u64];
    raw_syscall(
        SYS_FS_READ,
        path.as_ptr() as u64,
        path.len() as u64,
        req.as_ptr() as u64,
    )
}

/// Spawn a child with delegated capabilities: the child receives
/// `parent caps ∩ requested` — never more than the parent holds.
pub fn spawn_caps(path: &str, requested: u64) -> u64 {
    raw_syscall(
        SYS_SPAWN_CAPS,
        path.as_ptr() as u64,
        path.len() as u64,
        requested,
    )
}

/// Copy this process's opaque capability handles into `out`. Handles are
/// still checked by the kernel against owner, kind, scope, generation and
/// expiry on every use. Returns the number copied or ERR_*.
pub fn cap_list(out: &mut [u64]) -> u64 {
    raw_syscall(SYS_CAP_LIST, out.as_mut_ptr() as u64, out.len() as u64, 0)
}

/// Validate a handle for a capability kind and a resource range. Kind values
/// are the stable V0.8 ABI order documented in ADR-0013.
pub fn cap_check(handle: u64, kind: u64, scope_end: u64) -> u64 {
    raw_syscall(SYS_CAP_CHECK, handle, kind, scope_end)
}

/// Read device record `index` into `buf` (>= 16 bytes) via the kernel device
/// table. Returns 1 if a device exists at that index, 0 past the end, or an
/// ERR_*. Userspace never touches hardware — the kernel is the sole authority.
pub fn devinfo(index: u64, buf: &mut [u8]) -> u64 {
    raw_syscall(
        SYS_DEVINFO,
        index,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    )
}

/// Write a u16 as 4-digit lowercase hex (no allocator in userspace).
pub fn write_hex16(v: u16) {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 4];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = digits[((v >> (12 - i * 4)) & 0xF) as usize];
    }
    // SAFETY: buf is ASCII hex digits.
    write(unsafe { core::str::from_utf8_unchecked(&buf) });
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
