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
pub const SYS_CAP_REVOKE: u64 = 17;
pub const SYS_CAP_RESTRICT: u64 = 18;
pub const SYS_UPTIME: u64 = 19;
pub const SYS_NET_INFO: u64 = 20;
pub const SYS_UDP_BIND: u64 = 21;
pub const SYS_UDP_SEND: u64 = 22;
pub const SYS_UDP_RECV: u64 = 23;
pub const SYS_UDP_CLOSE: u64 = 24;
pub const SYS_NET_RESOLVE: u64 = 25;
pub const SYS_FS_WRITE: u64 = 26;
pub const SYS_FS_DELETE: u64 = 27;
pub const SYS_FS_LIST: u64 = 28;

/// Everything under this prefix lives on the persistent store; anything else
/// is the read-only initramfs.
pub const STORE_PREFIX: &str = "/data/";

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
/// V0.11: console-granted only, never delegable (`kernel_core::caps`).
pub const CAP_SYS_VIEW: u64 = 1 << 11;
pub const CAP_PROPOSE: u64 = 1 << 12;

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

/// Monotonic ticks since boot. Lets a long-running service pace itself in
/// real time instead of by scheduling passes, whose rate depends entirely on
/// how busy the rest of the system is.
pub fn uptime_ticks() -> u64 {
    raw_syscall(SYS_UPTIME, 0, 0, 0)
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

// --- Process model (V0.10) ------------------------------------------------------

pub const SYS_WAIT_NOHANG: u64 = 37;
pub const SYS_SLEEP: u64 = 38;
/// The status `wait` reports for a process the console killed.
pub const STATUS_KILLED: u64 = 2 << 32;

/// Collect an ended child WITHOUT blocking: `pid`, or any child for 0 (the
/// lowest pid first). Returns the collected child's pid and stores its status
/// in `status`; `ERR_AGAIN` while the matching children are all running;
/// `ERR_NOENT` if there is no such child — including a process that exists
/// but is not this process's child. Needs the Process capability.
pub fn wait_nohang(pid: u64, status: &mut u64) -> u64 {
    raw_syscall(SYS_WAIT_NOHANG, pid, status as *mut u64 as u64, 0)
}

/// Sleep for `ticks` timer ticks (100 Hz; at most 6000, else `ERR_INVAL`);
/// 0 is a yield. Returns 0.
pub fn sleep_ticks(ticks: u64) -> u64 {
    raw_syscall(SYS_SLEEP, ticks, 0, 0)
}

pub const SYS_SVC_REPORT: u64 = 39;
pub const SYS_CONSOLE_READ: u64 = 40;
pub const SYS_GUI_EVENT: u64 = 41;
/// V0.11 (ADR-0024): the approved system view (`CAP_SYS_VIEW`).
pub const SYS_SYS_VIEW: u64 = 42;

/// Copy the approved system view (a `kernel_core::sysview` record) into
/// `buf`: returns its length, `ERR_2BIG` if `buf` is too small, `ERR_PERM`
/// without the view capability.
pub fn sys_view(buf: &mut [u8]) -> u64 {
    raw_syscall(SYS_SYS_VIEW, buf.as_mut_ptr() as u64, buf.len() as u64, 0)
}

/// Event kinds in a `gui_event` record (`kernel_core::wm`): byte 0.
pub const GUI_EV_FOCUS_IN: u8 = 1;
pub const GUI_EV_FOCUS_OUT: u8 = 2;
/// Byte 1 is the key, as ASCII.
pub const GUI_EV_KEY: u8 = 3;
/// Bytes 2-3 and 4-5 are x and y (u16 LE) in the window's content.
pub const GUI_EV_CLICK: u8 = 4;

/// Wait for the next event on a window this process owns (Gui capability):
/// fills the 8-byte record and returns 8, or an error (`ERR_PERM` for a
/// window it does not own).
pub fn gui_event(win: u64, record: &mut [u8; 8]) -> u64 {
    loop {
        let r = raw_syscall(SYS_GUI_EVENT, win, record.as_mut_ptr() as u64, 8);
        // Parked until an event arrived: ask again.
        if r != ERR_AGAIN {
            return r;
        }
    }
}

/// Read one line from the console (no newline). Only the process the kernel
/// handed the console's input to (`rsh`) may; others get `ERR_PERM`. Waits
/// until a line is complete; `ERR_2BIG` if `buf` is too small for it (the
/// line is kept for the next call).
pub fn console_read(buf: &mut [u8]) -> u64 {
    loop {
        let r = raw_syscall(
            SYS_CONSOLE_READ,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
        );
        // The kernel parked us until a line arrived: ask again.
        if r != ERR_AGAIN {
            return r;
        }
    }
}

/// Report one service lifecycle event to the kernel's service table: the
/// 32-byte record of `kernel_core::svcreport`. Needs the Service capability
/// with ADMIN (`CAP_SERVICE`). Returns 0 or an error.
pub fn svc_report(req: &[u8; 32]) -> u64 {
    raw_syscall(SYS_SVC_REPORT, req.as_ptr() as u64, 32, 0)
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

/// Capability kinds — the stable V0.8 ABI indices a process uses to address
/// its own handles (same order as the kernel's `CapabilityKind::index`).
pub const KIND_FILESYSTEM: u64 = 0;
pub const KIND_DEVICE: u64 = 1;
pub const KIND_GUI: u64 = 2;
pub const KIND_NETWORK: u64 = 3;
pub const KIND_AUDIO: u64 = 4;
pub const KIND_PROCESS: u64 = 5;
pub const KIND_SERVICE: u64 = 6;
pub const KIND_SYSADMIN: u64 = 7;

/// Rights carried inside a handle (V0.8). A filesystem handle holding only
/// `RIGHT_READ` can never satisfy a write.
pub const RIGHT_USE: u64 = 1 << 0;
pub const RIGHT_READ: u64 = 1 << 1;
pub const RIGHT_WRITE: u64 = 1 << 2;
pub const RIGHT_CONTROL: u64 = 1 << 3;
pub const RIGHT_ADMIN: u64 = 1 << 4;

/// Permanently give up this process's authority over a resource kind.
///
/// Least privilege a program can apply to itself: once a task is done with an
/// authority it drops it, so a later compromise cannot use it. Enforcement
/// reads the capability table on every syscall, so this bites immediately.
pub fn cap_revoke(kind: u64) -> u64 {
    raw_syscall(SYS_CAP_REVOKE, kind, 0, 0)
}

/// Narrow this process's own handle for `kind`: keep only `rights_mask`, and
/// (when `expires_in_ticks` is non-zero) make it expire that many timer ticks
/// from now. Requests are intersected with what is already held, so this can
/// only ever remove authority.
pub fn cap_restrict(kind: u64, rights_mask: u64, expires_in_ticks: u64) -> u64 {
    raw_syscall(SYS_CAP_RESTRICT, kind, rights_mask, expires_in_ticks)
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
/// Largest UDP payload `udp_send` will assemble on the stack. Bounded by the
/// 64 KiB user stack, not by the protocol.
pub const UDP_SEND_MAX: usize = 1024;

/// Copy the 20-byte interface record: `[mac:6, ip:4, mask:4, gateway:4,
/// dns_low:1, link_up:1]`.
pub fn net_info(out: &mut [u8; 20]) -> u64 {
    raw_syscall(SYS_NET_INFO, out.as_mut_ptr() as u64, out.len() as u64, 0)
}

/// Bind a UDP port. Returns a socket descriptor or an ERR_*.
///
/// This is where the network capability is checked, scoped to the port: a
/// program whose handle does not cover `port` gets `ERR_PERM` here rather than
/// silently binding something it may not use.
pub fn udp_bind(port: u16) -> u64 {
    raw_syscall(SYS_UDP_BIND, port as u64, 0, 0)
}

/// Send `payload` to `dst:dst_port` from a bound socket.
pub fn udp_send(sock: u64, dst: [u8; 4], dst_port: u16, payload: &[u8]) -> u64 {
    if payload.len() > UDP_SEND_MAX {
        return ERR_INVAL;
    }
    let mut req = [0u8; 6 + UDP_SEND_MAX];
    req[0..4].copy_from_slice(&dst);
    req[4..6].copy_from_slice(&dst_port.to_le_bytes());
    req[6..6 + payload.len()].copy_from_slice(payload);
    raw_syscall(
        SYS_UDP_SEND,
        sock,
        req.as_ptr() as u64,
        (6 + payload.len()) as u64,
    )
}

/// Take the oldest queued datagram: `[src_ip:4, src_port:2, payload...]`.
/// Returns the byte count, or `ERR_AGAIN` when nothing has arrived.
pub fn udp_recv(sock: u64, buf: &mut [u8]) -> u64 {
    raw_syscall(
        SYS_UDP_RECV,
        sock,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    )
}

pub fn udp_close(sock: u64) -> u64 {
    raw_syscall(SYS_UDP_CLOSE, sock, 0, 0)
}

/// Resolve a host name to an IPv4 address. Writes four bytes on success.
pub fn net_resolve(name: &str, out: &mut [u8; 4]) -> u64 {
    raw_syscall(
        SYS_NET_RESOLVE,
        name.as_ptr() as u64,
        name.len() as u64,
        out.as_mut_ptr() as u64,
    )
}

/// Print a dotted-quad address.
pub fn write_ipv4(addr: [u8; 4]) {
    for (i, octet) in addr.iter().enumerate() {
        if i > 0 {
            write(".");
        }
        write_u64(*octet as u64);
    }
}

/// Create or overwrite a file in the persistent store.
///
/// Returns the number of bytes written, or an ERR_*. The write is one
/// crash-atomic commit: an interrupted overwrite leaves the previous contents,
/// never a truncated file.
pub fn fs_write(path: &str, data: &[u8]) -> u64 {
    let mut req = [0u8; 16];
    req[0..8].copy_from_slice(&(data.as_ptr() as u64).to_le_bytes());
    req[8..16].copy_from_slice(&(data.len() as u64).to_le_bytes());
    raw_syscall(
        SYS_FS_WRITE,
        path.as_ptr() as u64,
        path.len() as u64,
        req.as_ptr() as u64,
    )
}

/// Delete a file from the persistent store.
pub fn fs_delete(path: &str) -> u64 {
    raw_syscall(SYS_FS_DELETE, path.as_ptr() as u64, path.len() as u64, 0)
}

/// Newline-separated names in the persistent store.
pub fn fs_list(buf: &mut [u8]) -> u64 {
    raw_syscall(SYS_FS_LIST, buf.as_mut_ptr() as u64, buf.len() as u64, 0)
}

/// Print one byte as two lowercase hex digits (MAC octets, status bytes).
pub fn write_hex8(v: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let buf = [HEX[(v >> 4) as usize], HEX[(v & 0x0F) as usize]];
    write_raw(buf.as_ptr() as u64, 2);
}

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

// --- TCP (V0.9) ----------------------------------------------------------------

pub const SYS_TCP_CONNECT: u64 = 29;
pub const SYS_TCP_SEND: u64 = 30;
pub const SYS_TCP_RECV: u64 = 31;
pub const SYS_TCP_CLOSE: u64 = 32;
pub const SYS_TCP_STATE: u64 = 33;
pub const SYS_TCP_LISTEN: u64 = 34;

/// `tcp_state` results.
pub const TCP_CONNECTING: u64 = 0;
pub const TCP_ESTABLISHED: u64 = 1;
pub const TCP_CLOSING: u64 = 2;
pub const TCP_CLOSED: u64 = 3;
pub const TCP_RESET: u64 = 4;
pub const TCP_TIMED_OUT: u64 = 5;

/// Open a connection to `ip:port` (non-blocking; poll [`tcp_state`]).
pub fn tcp_connect(ip: [u8; 4], port: u16) -> u64 {
    let mut req = [0u8; 6];
    req[..4].copy_from_slice(&ip);
    req[4..].copy_from_slice(&port.to_le_bytes());
    raw_syscall(SYS_TCP_CONNECT, req.as_ptr() as u64, req.len() as u64, 0)
}

/// Passive open on `port`: a descriptor in LISTEN that becomes the stream
/// when a peer connects (poll [`tcp_state`] for `TCP_ESTABLISHED`).
pub fn tcp_listen(port: u16) -> u64 {
    raw_syscall(SYS_TCP_LISTEN, port as u64, 0, 0)
}

/// Queue bytes on a connection; returns how many were accepted.
pub fn tcp_send(sock: u64, data: &[u8]) -> u64 {
    raw_syscall(SYS_TCP_SEND, sock, data.as_ptr() as u64, data.len() as u64)
}

/// Read received bytes: the count, 0 at end of stream, `ERR_AGAIN` if none yet.
pub fn tcp_recv(sock: u64, buf: &mut [u8]) -> u64 {
    raw_syscall(
        SYS_TCP_RECV,
        sock,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    )
}

/// Orderly close.
pub fn tcp_close(sock: u64) -> u64 {
    raw_syscall(SYS_TCP_CLOSE, sock, 0, 0)
}

/// Connection state (`TCP_CONNECTING` … `TCP_TIMED_OUT`).
pub fn tcp_state(sock: u64) -> u64 {
    raw_syscall(SYS_TCP_STATE, sock, 0, 0)
}

// --- Program arguments (V0.10) ------------------------------------------------

pub const SYS_ARGS: u64 = 35;
pub const SYS_SPAWN_ARGS: u64 = 36;

/// Most arguments a process can carry (mirrors `kernel_core::progargs`).
pub const ARGS_MAX: usize = 16;
/// Largest argument block in bytes, terminators included — a buffer this
/// size always holds the whole block.
pub const ARGS_BLOCK_MAX: usize = 512;

/// Copy this process's argument block into `buf`: the arguments in order,
/// each followed by one NUL byte. Returns the block length (0 when launched
/// without arguments), `ERR_2BIG` — with NOTHING written — when `buf` is
/// shorter than the block, or `ERR_FAULT` for a bad buffer. Needs no
/// capability. Split the result with [`split_args`].
pub fn args(buf: &mut [u8]) -> u64 {
    raw_syscall(SYS_ARGS, buf.as_mut_ptr() as u64, buf.len() as u64, 0)
}

/// Iterate over the arguments in a block returned by [`args`].
pub fn split_args(block: &[u8]) -> ArgIter<'_> {
    ArgIter { rest: block }
}

/// Iterator returned by [`split_args`] (same splitting rule as
/// `kernel_core::progargs::split` — keep in lockstep).
pub struct ArgIter<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for ArgIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        match self.rest.iter().position(|&b| b == 0) {
            Some(i) => {
                let arg = &self.rest[..i];
                self.rest = &self.rest[i + 1..];
                Some(arg)
            }
            None => {
                let arg = self.rest;
                self.rest = &[];
                Some(arg)
            }
        }
    }
}

/// Spawn a child with delegated capabilities (as [`spawn_caps`]) and an
/// argument block in the [`args`] encoding. The kernel validates the block
/// with the same rules as the console: more than [`ARGS_MAX`] arguments or
/// [`ARGS_BLOCK_MAX`] bytes is `ERR_2BIG`; an empty argument, a space or
/// non-printable byte, or a missing final terminator is `ERR_INVAL`. Nothing
/// is loaded for a refused block. Requires the Process capability.
pub fn spawn_args(path: &str, requested: u64, block: &[u8]) -> u64 {
    let req: [u64; 3] = [requested, block.as_ptr() as u64, block.len() as u64];
    raw_syscall(
        SYS_SPAWN_ARGS,
        path.as_ptr() as u64,
        path.len() as u64,
        req.as_ptr() as u64,
    )
}
