//! TCP connections (V0.9): the host-tested state machine in
//! `kernel_core::net::tcp`, driven by the polled NIC and the TSC clock.
//!
//! The state machine owns every protocol decision; this module only moves its
//! segments on and off the wire, keeps a bounded table of connections, and
//! supplies time. Nothing is unbounded: at most [`MAX_CONNS`] connections,
//! each with the fixed buffers the `Tcb` carries, and one shared outbox. A
//! segment for a connection that does not exist is answered with a reset
//! (RFC 9293 §3.10.7.1), which is what a real host does and what lets a peer
//! fail fast instead of retrying.
//!
//! Time is driven from [`super::poll`]: every drain of the NIC also runs each
//! connection's timers, so a Ring 3 program polling its connection is also
//! what retransmits for it. There is no background timer — the same property
//! as the rest of this polled stack, and stated as a limitation.
//!
//! A test-only knob, [`drop_next_data_segments`], discards outgoing data
//! segments before they reach the NIC, so the retransmission timer can be
//! exercised against a real peer without a lossy link.

use super::{try_send_ipv4, IFACE, MAX_FRAME};
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use kernel_core::net::eth;
use kernel_core::net::ipv4::{self, Ipv4Addr};
use kernel_core::net::tcp::{self, Event, Outbox, State, Tcb};

/// Connection slots; a descriptor is an index into this table.
pub const MAX_CONNS: usize = 8;

/// Owner of a connection its process abandoned mid-close: it is driven to
/// `Closed` by the timers and then freed, never handed to anyone.
const ORPHAN: u64 = u64::MAX;

/// Outgoing data segments still to be discarded (loss injection).
static DROP_DATA: AtomicU32 = AtomicU32::new(0);
/// Segments the injection actually discarded.
pub static DROPPED: AtomicU64 = AtomicU64::new(0);
pub static SEGMENTS_TX: AtomicU64 = AtomicU64::new(0);
pub static SEGMENTS_RX: AtomicU64 = AtomicU64::new(0);
/// Data segments sent again below the highest sequence already sent — the
/// retransmission timer at work, counted here independently of the Tcb.
pub static RETRANSMITS: AtomicU64 = AtomicU64::new(0);
pub static RESETS_SENT: AtomicU64 = AtomicU64::new(0);
/// Segments that failed to parse (checksum, truncation, bad option).
pub static MALFORMED: AtomicU64 = AtomicU64::new(0);

/// How a connection ended, as its owner will be told.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Closed,
    Reset,
    TimedOut,
}

/// One slot. Slots are never built or moved by value: a `Tcb` is 8 KB, and
/// in an unoptimized build a single `Some(Conn { .. })` assignment made
/// enough stack copies of it to run off the end of the 32 KB syscall stack and
/// corrupt the capability table beside it (found by the first V0.9 TCP run).
/// A slot is claimed by flipping `used` and reinitializing its `Tcb` in place.
struct Conn {
    used: bool,
    tcb: Tcb,
    /// Owning process (0 = the kernel console, [`ORPHAN`] = abandoned).
    owner: u64,
    end: Option<End>,
    /// End of the highest data sequence sent so far, to spot retransmissions.
    high_water: u32,
    /// The port the owner's network authority was checked against when the
    /// connection was opened: the DESTINATION port for an active open, the
    /// LOCAL port for a listener. Every later call re-checks this, not the
    /// peer's port — a server's clients arrive from ports nobody chose.
    scope_port: u16,
}

impl Conn {
    const FREE: Conn = Conn {
        used: false,
        tcb: Tcb::new(),
        owner: 0,
        end: None,
        high_water: 0,
        scope_port: 0,
    };

    fn free(&mut self) {
        self.used = false;
        self.owner = 0;
        self.end = None;
        self.tcb.reset_in_place();
    }
}

struct Table {
    conns: [Conn; MAX_CONNS],
    /// One outbox shared by every connection: it is 6 KB, too large for the
    /// syscall stack, and only ever used under this lock.
    out: Outbox,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    conns: [const { Conn::FREE }; MAX_CONNS],
    out: Outbox::new(),
});
static NEXT_PORT: AtomicU32 = AtomicU32::new(0);

/// Milliseconds on the TSC — the only clock that also advances with
/// interrupts masked, which they are inside every syscall.
pub fn now_ms() -> u64 {
    crate::interrupts::tsc() / crate::interrupts::cycles_for_ms(1).max(1)
}

/// Discard the next `n` outgoing segments that carry data.
pub fn drop_next_data_segments(n: u32) {
    DROP_DATA.store(n, Ordering::SeqCst);
}

/// An initial sequence number nobody off the machine can predict from the
/// last one: the TSC (a clock, as RFC 9293 asks) mixed with the connection's
/// four-tuple through SplitMix64. Not RFC 6528's keyed hash — there is no
/// per-boot secret yet — which is recorded as a limitation.
fn initial_sequence(local_port: u16, remote: Ipv4Addr, remote_port: u16) -> u32 {
    let mut z = crate::interrupts::tsc()
        ^ ((local_port as u64) << 48)
        ^ ((u32::from_be_bytes(remote.octets()) as u64) << 16)
        ^ remote_port as u64;
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as u32
}

/// Put the outbox on the wire and empty it. A segment that cannot be sent
/// (next hop not resolved yet) is simply not sent: TCP's own retransmission
/// recovers it, which is the whole point of having one.
fn flush(out: &mut Outbox, high_water: Option<&mut u32>) {
    let mut high = high_water;
    for seg in out.segments() {
        let len = seg.payload().len() as u32;
        if len > 0 {
            let end = seg.header.seq.wrapping_add(len);
            if let Some(h) = high.as_deref_mut() {
                if tcp::seq_lt(seg.header.seq, *h) {
                    RETRANSMITS.fetch_add(1, Ordering::Relaxed);
                    crate::serial_println!(
                        "[ITISYOU:TCP] retransmit seq={} bytes={}",
                        seg.header.seq,
                        len
                    );
                }
                if tcp::seq_gt(end, *h) {
                    *h = end;
                }
            }
            if DROP_DATA.load(Ordering::SeqCst) > 0 {
                DROP_DATA.fetch_sub(1, Ordering::SeqCst);
                DROPPED.fetch_add(1, Ordering::Relaxed);
                crate::serial_println!(
                    "[ITISYOU:TCP] injected_loss seq={} bytes={}",
                    seg.header.seq,
                    len
                );
                continue;
            }
        }
        let mut buf = [0u8; MAX_FRAME - eth::HEADER_LEN];
        let Ok(n) = seg.encode(&mut buf) else {
            continue;
        };
        if try_send_ipv4(seg.dst, ipv4::proto::TCP, &buf[..n]).is_ok() {
            SEGMENTS_TX.fetch_add(1, Ordering::Relaxed);
            if seg.header.has(tcp::flags::RST) {
                RESETS_SENT.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    out.clear();
}

/// Record how a call left the connection; a terminal event is kept so the
/// owner can learn it even after the slot has gone quiet.
fn note(conn: &mut Conn, event: Event) {
    let end = match event {
        Event::Closed => Some(End::Closed),
        Event::Reset => Some(End::Reset),
        Event::TimedOut => Some(End::TimedOut),
        _ => None,
    };
    if conn.end.is_none() {
        conn.end = end;
    }
}

/// Open a connection to `remote:port` for `owner`. Non-blocking: the SYN is
/// on its way when this returns, and the descriptor is the slot index.
/// `None` when every slot (or every ephemeral port) is taken.
pub fn connect(owner: u64, remote: Ipv4Addr, port: u16) -> Option<usize> {
    let local = IFACE.lock().ip;
    let now = now_ms();
    let mut table = TABLE.lock();
    let Table { conns, out } = &mut *table;
    let slot = conns.iter().position(|c| !c.used)?;
    // An ephemeral port no live connection is using.
    let local_port = (0..16384)
        .map(|_| 49152 + (NEXT_PORT.fetch_add(1, Ordering::Relaxed) % 16384) as u16)
        .find(|p| !conns.iter().any(|c| c.used && c.tcb.local().1 == *p))?;
    let iss = initial_sequence(local_port, remote, port);
    let conn = &mut conns[slot];
    conn.tcb
        .connect_in_place(local, local_port, remote, port, iss, now, out);
    if conn.tcb.state() == State::Closed {
        out.clear();
        return None;
    }
    conn.used = true;
    conn.owner = owner;
    conn.end = None;
    conn.high_water = iss.wrapping_add(1);
    conn.scope_port = port;
    flush(out, Some(&mut conn.high_water));
    Some(slot)
}

/// Passive open on `port` for `owner`: the slot waits in LISTEN and becomes
/// the connection when a SYN arrives (one connection per listen, like the
/// state machine's own passive open). `None` when no slot is free or the
/// port is taken.
pub fn listen(owner: u64, port: u16) -> Option<usize> {
    let local = IFACE.lock().ip;
    let mut table = TABLE.lock();
    let Table { conns, .. } = &mut *table;
    if port == 0 || conns.iter().any(|c| c.used && c.tcb.local().1 == port) {
        return None;
    }
    let slot = conns.iter().position(|c| !c.used)?;
    let iss = initial_sequence(port, Ipv4Addr::UNSPECIFIED, 0);
    let conn = &mut conns[slot];
    conn.tcb.listen_in_place(local, port, iss);
    conn.used = true;
    conn.owner = owner;
    conn.end = None;
    conn.high_water = iss.wrapping_add(1);
    conn.scope_port = port;
    Some(slot)
}

/// The peer of `index`, if `owner` holds it (unspecified while listening).
pub fn peer(index: usize, owner: u64) -> Option<(Ipv4Addr, u16)> {
    let mut table = TABLE.lock();
    owned(&mut table.conns, index, owner).map(|c| c.tcb.remote())
}

fn owned(conns: &mut [Conn; MAX_CONNS], index: usize, owner: u64) -> Option<&mut Conn> {
    conns.get_mut(index).filter(|c| c.used && c.owner == owner)
}

/// The port `index`'s network authority is scoped to, if `owner` holds it —
/// what the syscall layer re-checks the capability against on every call.
pub fn scope_port(index: usize, owner: u64) -> Option<u16> {
    let mut table = TABLE.lock();
    owned(&mut table.conns, index, owner).map(|c| c.scope_port)
}

/// Queue bytes; `Some(accepted)` (0 when the buffer is full or the
/// connection is not open for sending), `None` for a bad descriptor.
pub fn send(index: usize, owner: u64, data: &[u8]) -> Option<usize> {
    let now = now_ms();
    let mut table = TABLE.lock();
    let Table { conns, out } = &mut *table;
    let conn = owned(conns, index, owner)?;
    let n = conn.tcb.send(data, now, out);
    flush(out, Some(&mut conn.high_water));
    Some(n)
}

/// The outcome of a read.
pub enum Read {
    Data(usize),
    /// The peer closed and everything it sent has been read.
    Eof,
    /// Nothing yet.
    Again,
    /// The connection was reset or timed out.
    Failed,
}

/// Read received bytes into `dst`.
pub fn recv(index: usize, owner: u64, dst: &mut [u8]) -> Option<Read> {
    let now = now_ms();
    let mut table = TABLE.lock();
    let Table { conns, out } = &mut *table;
    let conn = owned(conns, index, owner)?;
    let n = conn.tcb.recv(dst);
    // `recv` may have reopened the window; the timer path advertises it.
    let event = conn.tcb.on_timer(now, out);
    note(conn, event);
    flush(out, Some(&mut conn.high_water));
    if n > 0 {
        return Some(Read::Data(n));
    }
    if matches!(conn.end, Some(End::Reset | End::TimedOut)) {
        return Some(Read::Failed);
    }
    let peer_finished = matches!(
        conn.tcb.state(),
        State::CloseWait | State::Closing | State::LastAck | State::TimeWait | State::Closed
    );
    Some(if peer_finished {
        Read::Eof
    } else {
        Read::Again
    })
}

/// `tcp_state` codes (the ABI in `ulib`).
pub const CODE_CONNECTING: u64 = 0;
pub const CODE_ESTABLISHED: u64 = 1;
pub const CODE_CLOSING: u64 = 2;
pub const CODE_CLOSED: u64 = 3;
pub const CODE_RESET: u64 = 4;
pub const CODE_TIMED_OUT: u64 = 5;

/// The connection's state as its owner sees it. TIME-WAIT reads as closed:
/// the local side is finished; the slot lingers only to absorb a late
/// retransmission.
pub fn state_code(index: usize, owner: u64) -> Option<u64> {
    let mut table = TABLE.lock();
    let conn = owned(&mut table.conns, index, owner)?;
    Some(match (conn.end, conn.tcb.state()) {
        (Some(End::Reset), _) => CODE_RESET,
        (Some(End::TimedOut), _) => CODE_TIMED_OUT,
        (Some(End::Closed), _) | (_, State::TimeWait | State::Closed) => CODE_CLOSED,
        (_, State::SynSent | State::SynReceived | State::Listen) => CODE_CONNECTING,
        (_, State::Established | State::CloseWait) => CODE_ESTABLISHED,
        _ => CODE_CLOSING,
    })
}

/// Orderly close: FIN after anything still queued. A connection that has
/// already ended is freed at once; otherwise the slot stays with its owner
/// until the close completes (TIME-WAIT is reclaimed by the timers).
pub fn close(index: usize, owner: u64) -> bool {
    let now = now_ms();
    let mut table = TABLE.lock();
    let Table { conns, out } = &mut *table;
    let Some(conn) = owned(conns, index, owner) else {
        return false;
    };
    if conn.end.is_some() {
        conn.free();
        return true;
    }
    let event = conn.tcb.close(now, out);
    note(conn, event);
    flush(out, Some(&mut conn.high_water));
    true
}

/// A process exited: abort what it left open (RST, so the peer does not wait
/// forever), and let what it was already closing finish as an orphan.
pub fn close_owner(owner: u64) -> usize {
    let mut table = TABLE.lock();
    let Table { conns, out } = &mut *table;
    let (mut orphaned, mut aborted) = (0, 0);
    for conn in conns.iter_mut() {
        if !conn.used || conn.owner != owner {
            continue;
        }
        let closing = matches!(
            conn.tcb.state(),
            State::FinWait1 | State::FinWait2 | State::Closing | State::LastAck | State::TimeWait
        );
        if closing && conn.end.is_none() {
            conn.owner = ORPHAN;
            orphaned += 1;
        } else {
            conn.tcb.abort(out);
            flush(out, None);
            conn.free();
            aborted += 1;
        }
    }
    if orphaned + aborted > 0 {
        crate::serial_println!(
            "[ITISYOU:TCP] owner_exit pid={owner} orphaned={orphaned} aborted={aborted}"
        );
    }
    orphaned + aborted
}

/// Deliver one received segment (already addressed to us). Called from the
/// IPv4 dispatch with the packet's own source and destination, which the
/// checksum binds the segment to.
pub fn on_segment(src: Ipv4Addr, dst: Ipv4Addr, bytes: &[u8]) {
    let seg = match tcp::parse(bytes, src, dst) {
        Ok(s) => s,
        Err(_) => {
            MALFORMED.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    SEGMENTS_RX.fetch_add(1, Ordering::Relaxed);
    let now = now_ms();
    let mut table = TABLE.lock();
    let Table { conns, out } = &mut *table;
    // An established four-tuple first; failing that, a listener on the port.
    let exact = conns.iter().position(|c| {
        c.used
            && c.tcb.local().1 == seg.header.dst_port
            && c.tcb.remote() == (seg.src, seg.header.src_port)
            && !matches!(c.tcb.state(), State::Closed | State::Listen)
    });
    let listener = || {
        conns.iter().position(|c| {
            c.used && c.tcb.state() == State::Listen && c.tcb.local().1 == seg.header.dst_port
        })
    };
    let hit = exact.or_else(listener).map(|i| &mut conns[i]);
    match hit {
        Some(conn) => {
            let event = conn.tcb.on_segment(&seg, now, out);
            note(conn, event);
            flush(out, Some(&mut conn.high_water));
        }
        None => {
            // Nobody has this four-tuple: refuse it the way RFC 9293 says, so
            // the peer learns at once instead of retrying into silence.
            if tcp::reset_reply(&seg, out) {
                flush(out, None);
            }
        }
    }
}

/// Run every connection's timers: retransmission, window updates, TIME-WAIT
/// expiry. Frees orphans that have finished closing.
pub fn tick() {
    let now = now_ms();
    let mut table = TABLE.lock();
    let Table { conns, out } = &mut *table;
    for conn in conns.iter_mut() {
        if !conn.used {
            continue;
        }
        if conn.tcb.state() != State::Closed {
            let event = conn.tcb.on_timer(now, out);
            note(conn, event);
            flush(out, Some(&mut conn.high_water));
        }
        if conn.owner == ORPHAN && conn.tcb.state() == State::Closed {
            conn.free();
        }
    }
}

/// One row of the console's `tcp` listing.
pub struct ConnInfo {
    pub index: usize,
    pub owner: u64,
    pub state: State,
    pub local_port: u16,
    pub remote: Ipv4Addr,
    pub remote_port: u16,
    pub snd_una: u32,
    pub rcv_nxt: u32,
}

/// Visit every live connection (for the console).
pub fn for_each(mut f: impl FnMut(&ConnInfo)) {
    let table = TABLE.lock();
    for (index, c) in table.conns.iter().enumerate() {
        if c.used {
            let (remote, remote_port) = c.tcb.remote();
            f(&ConnInfo {
                index,
                owner: c.owner,
                state: c.tcb.state(),
                local_port: c.tcb.local().1,
                remote,
                remote_port,
                snd_una: c.tcb.snd_una(),
                rcv_nxt: c.tcb.rcv_nxt(),
            });
        }
    }
}
