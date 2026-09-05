//! UDP sockets: a fixed table of bound ports with bounded receive queues.
//!
//! There is no allocation anywhere in the receive path. A socket owns a small
//! ring of pre-sized slots; when it is full the *oldest* datagram is dropped
//! and counted. That choice matters for a kernel: the alternative — growing a
//! queue — hands a remote sender a lever on kernel memory, and dropping the
//! *newest* instead would let one burst mask everything that follows it.
//!
//! Ownership is by pid. A socket belongs to the process that bound it, so a
//! process cannot read another's datagrams even if it guesses the descriptor
//! number, and every socket a process holds is closed when it dies.

use kernel_core::net::ipv4::Ipv4Addr;
use spin::Mutex;

/// Concurrently bound ports. Small on purpose: this is a system stack, not a
/// server, and an unbounded table would be another remote-influenced
/// allocation.
pub const MAX_SOCKETS: usize = 8;
/// Datagrams a socket may hold before the oldest is dropped.
pub const QUEUE_LEN: usize = 4;
/// Largest datagram payload a socket will store.
pub const MAX_DATAGRAM: usize = 1024;

/// Owner value for sockets the kernel binds for its own use (DNS lookups).
/// Distinct from any real pid, including 0.
pub const KERNEL_OWNER: u64 = u64::MAX;

#[derive(Clone, Copy)]
struct Pending {
    src: Ipv4Addr,
    src_port: u16,
    len: u16,
    data: [u8; MAX_DATAGRAM],
}

const EMPTY_PENDING: Pending = Pending {
    src: Ipv4Addr::UNSPECIFIED,
    src_port: 0,
    len: 0,
    data: [0; MAX_DATAGRAM],
};

#[derive(Clone, Copy)]
struct Socket {
    used: bool,
    owner: u64,
    port: u16,
    queue: [Pending; QUEUE_LEN],
    /// Number of queued datagrams; the ring runs from `head` for `count`.
    head: usize,
    count: usize,
    /// Datagrams dropped because the queue was full. Reported by `net` so a
    /// slow reader is visible rather than mysterious.
    dropped: u64,
    received: u64,
}

const EMPTY_SOCKET: Socket = Socket {
    used: false,
    owner: 0,
    port: 0,
    queue: [EMPTY_PENDING; QUEUE_LEN],
    head: 0,
    count: 0,
    dropped: 0,
    received: 0,
};

static TABLE: Mutex<[Socket; MAX_SOCKETS]> = Mutex::new([EMPTY_SOCKET; MAX_SOCKETS]);

/// Why a bind was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// Port 0 is reserved and can never be bound.
    ZeroPort,
    /// Another socket already holds this port. Ports are not shared: two
    /// readers of one port would make delivery ambiguous.
    InUse,
    /// The socket table is full.
    NoSpace,
}

fn bind_as(owner: u64, port: u16) -> Result<usize, BindError> {
    if port == 0 {
        return Err(BindError::ZeroPort);
    }
    let mut table = TABLE.lock();
    if table.iter().any(|s| s.used && s.port == port) {
        return Err(BindError::InUse);
    }
    let slot = table
        .iter()
        .position(|s| !s.used)
        .ok_or(BindError::NoSpace)?;
    table[slot] = Socket {
        used: true,
        owner,
        port,
        ..EMPTY_SOCKET
    };
    Ok(slot)
}

/// Bind `port` on behalf of `pid`.
pub fn bind(pid: u64, port: u16) -> Result<usize, BindError> {
    bind_as(pid, port)
}

/// Bind a port for the kernel's own use (DNS). Returns `None` rather than an
/// error because the caller has no way to act on the distinction.
pub fn bind_kernel(port: u16) -> Option<usize> {
    bind_as(KERNEL_OWNER, port).ok()
}

pub fn close_kernel(index: usize) {
    close(index, KERNEL_OWNER);
}

/// Release a socket. A mismatched owner is ignored, so a process cannot close
/// a descriptor it does not hold.
pub fn close(index: usize, owner: u64) -> bool {
    let mut table = TABLE.lock();
    match table.get_mut(index) {
        Some(s) if s.used && s.owner == owner => {
            *s = EMPTY_SOCKET;
            true
        }
        _ => false,
    }
}

/// Close every socket belonging to `pid`. Called from process teardown, so a
/// crashed program cannot leave a port bound forever.
pub fn close_owner(pid: u64) -> usize {
    let mut table = TABLE.lock();
    let mut n = 0;
    for s in table.iter_mut() {
        if s.used && s.owner == pid {
            *s = EMPTY_SOCKET;
            n += 1;
        }
    }
    n
}

/// The port a descriptor is bound to, if the caller owns it.
pub fn port_of(index: usize, owner: u64) -> Option<u16> {
    let table = TABLE.lock();
    table
        .get(index)
        .filter(|s| s.used && s.owner == owner)
        .map(|s| s.port)
}

/// Queue a received datagram for whichever socket holds `port`.
/// Returns false when nobody is listening — the caller counts that as an
/// unwanted packet rather than silently discarding it.
pub fn deliver(port: u16, src: Ipv4Addr, src_port: u16, payload: &[u8]) -> bool {
    if payload.len() > MAX_DATAGRAM {
        return false;
    }
    let mut table = TABLE.lock();
    let Some(sock) = table.iter_mut().find(|s| s.used && s.port == port) else {
        return false;
    };
    if sock.count == QUEUE_LEN {
        // Drop the oldest: a full queue means the reader is behind, and the
        // newest datagram is the one most likely to still matter.
        sock.head = (sock.head + 1) % QUEUE_LEN;
        sock.count -= 1;
        sock.dropped += 1;
    }
    let slot = (sock.head + sock.count) % QUEUE_LEN;
    let entry = &mut sock.queue[slot];
    entry.src = src;
    entry.src_port = src_port;
    entry.len = payload.len() as u16;
    entry.data[..payload.len()].copy_from_slice(payload);
    sock.count += 1;
    sock.received += 1;
    true
}

/// Take the oldest queued datagram, copying it into `out`.
/// Returns `(bytes_copied, source_address, source_port)`.
pub fn take(index: usize, out: &mut [u8]) -> Option<(usize, Ipv4Addr, u16)> {
    take_owned(index, KERNEL_OWNER, out)
}

/// As [`take`], but only for the socket's owner.
pub fn take_owned(index: usize, owner: u64, out: &mut [u8]) -> Option<(usize, Ipv4Addr, u16)> {
    let mut table = TABLE.lock();
    let sock = table.get_mut(index)?;
    if !sock.used || sock.owner != owner || sock.count == 0 {
        return None;
    }
    let slot = sock.head;
    let entry = sock.queue[slot];
    sock.head = (sock.head + 1) % QUEUE_LEN;
    sock.count -= 1;
    let len = core::cmp::min(entry.len as usize, out.len());
    out[..len].copy_from_slice(&entry.data[..len]);
    Some((len, entry.src, entry.src_port))
}

/// One row of the socket table, for diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct SocketInfo {
    pub index: usize,
    pub owner: u64,
    pub port: u16,
    pub queued: usize,
    pub received: u64,
    pub dropped: u64,
}

/// Run `f` over a snapshot of the bound sockets.
pub fn with_sockets<R>(f: impl FnOnce(&[SocketInfo]) -> R) -> R {
    let table = TABLE.lock();
    let mut rows = [SocketInfo {
        index: 0,
        owner: 0,
        port: 0,
        queued: 0,
        received: 0,
        dropped: 0,
    }; MAX_SOCKETS];
    let mut n = 0;
    for (i, s) in table.iter().enumerate() {
        if s.used {
            rows[n] = SocketInfo {
                index: i,
                owner: s.owner,
                port: s.port,
                queued: s.count,
                received: s.received,
                dropped: s.dropped,
            };
            n += 1;
        }
    }
    drop(table);
    f(&rows[..n])
}
