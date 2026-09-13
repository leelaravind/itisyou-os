//! TCP (RFC 9293 subset) over IPv4: a segment codec and one bounded,
//! allocation-free connection state machine ([`Tcb`]).
//!
//! The decisions that shape this module, and why:
//!
//! * **No clocks, no I/O.** Time arrives as `now_ms`, and every segment the
//!   state machine wants transmitted is appended to a caller-owned
//!   [`Outbox`]. That is what lets the whole machine — handshake, loss,
//!   retransmission, teardown — run under `cargo test` against a scripted
//!   peer, or against a second `Tcb` across a lossy simulated wire.
//! * **Fixed memory per connection.** 4 KiB send and 4 KiB receive buffers
//!   and at most four peer-MSS segments in flight. A peer can make a
//!   connection slow; it cannot make it grow.
//! * **In-order data only.** Out-of-order segments are not queued: a
//!   reassembly queue is attacker-shaped state, the same class of problem as
//!   the IP fragment reassembly `ipv4` refuses. The receiver ACKs `rcv_nxt`
//!   instead and the sender's retransmission timer fills the hole.
//! * **Blind-injection hardening (RFC 5961).** A RST resets a connection only
//!   at exactly `rcv_nxt`; an in-window RST, or any SYN on a synchronized
//!   connection, earns a challenge ACK instead. An off-path attacker must
//!   guess the exact sequence number, not merely land inside the window.
//! * **The checksum is mandatory.** Unlike UDP, a TCP checksum of zero is not
//!   "omitted"; it is verified like any other value.
//! * **A full [`Outbox`] behaves like a lossy wire.** Data that does not fit
//!   waits in the send buffer; a dropped ACK or control segment is recovered
//!   by the peer's or our own timers, exactly as if the network lost it.
//!
//! Deliberately not implemented: congestion control beyond the in-flight cap,
//! SACK, window scaling, timestamps, urgent data, delayed ACKs, fast
//! retransmit, RTT estimation (the RTO is a fixed start with exponential
//! backoff), a zero-window persist timer (if the peer's window-opening ACK is
//! lost, sending resumes only when the peer next sends), a FIN-WAIT-2 timeout
//! (the owner decides when to `abort` a half-closed peer), challenge-ACK rate
//! limiting, and data carried on a SYN (ignored; the peer retransmits it).

use super::checksum;
use super::ipv4::{proto, Ipv4Addr};

/// Option-free header length.
pub const HEADER_LEN: usize = 20;
/// Byte offset of the checksum field within the header.
pub const CHECKSUM_OFFSET: usize = 16;
/// Largest segment (header + payload) whose IPv4 packet still fits the
/// 16-bit total-length field.
pub const MAX_SEGMENT_LEN: usize = u16::MAX as usize - super::ipv4::MIN_HEADER_LEN;
/// Largest payload one of our segments carries, and the MSS we advertise: a
/// 1500-byte MTU less the option-free IPv4 and TCP headers.
pub const MAX_SEG_PAYLOAD: usize = 1500 - super::ipv4::MIN_HEADER_LEN - HEADER_LEN;
const LOCAL_MSS: u16 = MAX_SEG_PAYLOAD as u16;
/// Peer MSS assumed when its SYN carries no MSS option (RFC 9293 §3.7.1).
pub const DEFAULT_MSS: u16 = 536;
/// Floor for a peer-advertised MSS. A hostile MSS of 1 would turn every send
/// into a flood of 41-byte packets, so anything below this is raised to it.
pub const MIN_PEER_MSS: u16 = 64;
/// Per-connection send buffer: unacknowledged bytes, then unsent ones.
pub const SEND_BUF_LEN: usize = 4096;
/// Per-connection receive buffer; its free space is the advertised window.
pub const RECV_BUF_LEN: usize = 4096;
/// In-flight cap in peer-MSS units — the only congestion control there is.
pub const MAX_IN_FLIGHT_SEGMENTS: usize = 4;
/// First retransmission timeout. Fixed, not measured (there is no RTT
/// estimator): short enough for a LAN-like virtual link, then backed off.
pub const INITIAL_RTO_MS: u64 = 300;
/// Ceiling on the backed-off RTO, so the retry schedule stays bounded.
pub const MAX_RTO_MS: u64 = 60_000;
/// Retransmissions of the oldest unacknowledged segment before the
/// connection is abandoned with [`Event::TimedOut`].
pub const MAX_RETRIES: u8 = 6;
/// Maximum segment lifetime: 500 ms, far below RFC 9293's two minutes, on
/// purpose. This kernel talks to one host over a QEMU virtual link, where a
/// stale duplicate cannot survive for minutes, and a short TIME-WAIT keeps a
/// fixed connection table from filling with the dead.
pub const MSL_MS: u64 = 500;
/// TIME-WAIT lasts 2 × MSL.
pub const TIME_WAIT_MS: u64 = 2 * MSL_MS;
/// Segments one [`Outbox`] holds: enough for a full in-flight window
/// released by a single ACK.
pub const OUTBOX_CAP: usize = 4;

/// Control bits: the low six bits of header byte 13. ECE/CWR are masked off
/// on parse — without ECN support, honouring half of it would be worse than
/// ignoring it.
pub mod flags {
    /// The sender has no more data.
    pub const FIN: u8 = 0x01;
    /// Synchronize sequence numbers; occupies one sequence number.
    pub const SYN: u8 = 0x02;
    /// Reset the connection.
    pub const RST: u8 = 0x04;
    /// Push; set on the segment that empties our send buffer.
    pub const PSH: u8 = 0x08;
    /// The acknowledgment field is significant.
    pub const ACK: u8 = 0x10;
    /// Parsed, never acted on: urgent data is not implemented.
    pub const URG: u8 = 0x20;
}
use flags::{ACK, FIN, PSH, RST, SYN};
const FLAG_MASK: u8 = 0x3F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpError {
    /// Fewer than [`HEADER_LEN`] bytes.
    TooShort,
    /// Data offset below five words: the header claims to be shorter than
    /// its own fixed part.
    BadDataOffset,
    /// The data offset is plausible but runs past the buffer.
    HeaderPastBuffer,
    /// The checksum did not verify against the pseudo-header (zero included).
    BadChecksum,
    /// Port 0 is reserved and never a valid endpoint.
    ZeroPort,
    /// An option whose length is below 2 or runs past the header, or an MSS
    /// option that is not exactly 4 bytes.
    BadOption,
    /// Build: the destination buffer cannot hold the segment.
    BufferTooSmall,
    /// The segment would not fit an IPv4 packet's 16-bit length field.
    PayloadTooLarge,
}

/// The header fields this stack reads and writes. The urgent pointer and
/// reserved bits are zero on output and ignored on input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SegmentHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    /// [`flags`] bits.
    pub flags: u8,
    /// Receive window in bytes (no window scaling).
    pub window: u16,
}

impl SegmentHeader {
    /// True when the [`flags`] bit `flag` is set.
    pub const fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// A validated segment borrowing the caller's buffer. It carries the
/// addresses it was verified against, because the checksum binds it to them
/// and connection lookup needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment<'a> {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub header: SegmentHeader,
    /// The MSS option, when the sender included one.
    pub mss: Option<u16>,
    pub payload: &'a [u8],
}

impl Segment<'_> {
    /// True when the [`flags`] bit `flag` is set.
    pub const fn has(&self, flag: u8) -> bool {
        self.header.has(flag)
    }

    /// Sequence space the segment occupies: its payload, plus one each for
    /// SYN and FIN.
    pub fn seq_len(&self) -> u32 {
        (self.payload.len() as u32)
            .wrapping_add(self.has(SYN) as u32)
            .wrapping_add(self.has(FIN) as u32)
    }
}

/// Parse and fully validate a segment. `src`/`dst` are the IPv4 addresses it
/// arrived between: the checksum covers a pseudo-header built from them, which
/// is what stops a segment being replayed against a rewritten address.
pub fn parse<'a>(bytes: &'a [u8], src: Ipv4Addr, dst: Ipv4Addr) -> Result<Segment<'a>, TcpError> {
    if bytes.len() < HEADER_LEN {
        return Err(TcpError::TooShort);
    }
    // Every fixed-offset index below is < HEADER_LEN <= bytes.len().
    let be16 = |i: usize| u16::from_be_bytes([bytes[i], bytes[i + 1]]);
    let be32 = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let header_len = (bytes[12] >> 4) as usize * 4;
    if header_len < HEADER_LEN {
        return Err(TcpError::BadDataOffset);
    }
    let (header, payload) = bytes
        .split_at_checked(header_len)
        .ok_or(TcpError::HeaderPastBuffer)?;
    let (src_port, dst_port) = (be16(0), be16(2));
    if src_port == 0 || dst_port == 0 {
        return Err(TcpError::ZeroPort);
    }
    let computed = checksum::transport_checksum(
        src.octets(),
        dst.octets(),
        proto::TCP,
        header,
        CHECKSUM_OFFSET,
        payload,
    )
    .ok_or(TcpError::PayloadTooLarge)?;
    let stored = be16(CHECKSUM_OFFSET);
    // A stored zero is a claim to verify, never "omitted". 0x0000 and 0xFFFF
    // are the same one's-complement value, so a sender that encodes a zero sum
    // UDP-style as 0xFFFF verifies too.
    if stored != computed && !(computed == 0 && stored == 0xFFFF) {
        return Err(TcpError::BadChecksum);
    }
    let mss = parse_options(header.get(HEADER_LEN..).unwrap_or(&[]))?;
    Ok(Segment {
        src,
        dst,
        header: SegmentHeader {
            src_port,
            dst_port,
            seq: be32(4),
            ack: be32(8),
            flags: bytes[13] & FLAG_MASK,
            window: be16(14),
        },
        mss,
        payload,
    })
}

/// Walk the option list and return the MSS, if any. Bounded by the header:
/// every step consumes at least one byte, and a length below 2 (which would
/// stall the walk) or past the header (which would read payload as options)
/// is refused rather than guessed around. Unknown kinds are skipped by length.
fn parse_options(mut opts: &[u8]) -> Result<Option<u16>, TcpError> {
    let mut mss = None;
    while let Some((&kind, rest)) = opts.split_first() {
        match kind {
            0 => break,       // end of option list
            1 => opts = rest, // no-op padding
            _ => {
                let len = *rest.first().ok_or(TcpError::BadOption)? as usize;
                // `get(2..len)` is None for len < 2 and for len past the end.
                let body = opts.get(2..len).ok_or(TcpError::BadOption)?;
                if kind == 2 {
                    let &[hi, lo] = body else {
                        return Err(TcpError::BadOption);
                    };
                    mss = Some(u16::from_be_bytes([hi, lo]));
                }
                opts = opts.get(len..).unwrap_or(&[]);
            }
        }
    }
    Ok(mss)
}

/// Build a segment into `buf`, always checksummed. The MSS option, when
/// given, is exactly one 32-bit word, so the header needs no padding.
pub fn build_into(
    buf: &mut [u8],
    src: Ipv4Addr,
    dst: Ipv4Addr,
    hdr: &SegmentHeader,
    mss: Option<u16>,
    payload: &[u8],
) -> Result<usize, TcpError> {
    if hdr.src_port == 0 || hdr.dst_port == 0 {
        return Err(TcpError::ZeroPort);
    }
    let header_len = if mss.is_some() {
        HEADER_LEN + 4
    } else {
        HEADER_LEN
    };
    let total = header_len
        .checked_add(payload.len())
        .filter(|&t| t <= MAX_SEGMENT_LEN)
        .ok_or(TcpError::PayloadTooLarge)?;
    let out = buf.get_mut(..total).ok_or(TcpError::BufferTooSmall)?;
    out[0..2].copy_from_slice(&hdr.src_port.to_be_bytes());
    out[2..4].copy_from_slice(&hdr.dst_port.to_be_bytes());
    out[4..8].copy_from_slice(&hdr.seq.to_be_bytes());
    out[8..12].copy_from_slice(&hdr.ack.to_be_bytes());
    out[12] = ((header_len / 4) as u8) << 4;
    out[13] = hdr.flags & FLAG_MASK;
    out[14..16].copy_from_slice(&hdr.window.to_be_bytes());
    out[16..20].fill(0); // checksum (computed below) and urgent pointer
    if let Some(m) = mss {
        let [hi, lo] = m.to_be_bytes();
        out[20..24].copy_from_slice(&[2, 4, hi, lo]);
    }
    out[header_len..].copy_from_slice(payload);
    let sum = checksum::transport_checksum(
        src.octets(),
        dst.octets(),
        proto::TCP,
        &out[..header_len],
        CHECKSUM_OFFSET,
        payload,
    )
    .ok_or(TcpError::PayloadTooLarge)?;
    out[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2].copy_from_slice(&sum.to_be_bytes());
    Ok(total)
}

/// Sequence-number comparison modulo 2^32 (RFC 9293 §3.4): `a` precedes `b`.
/// Valid whenever the two are within 2^31 of each other, which windows bounded
/// by a 16-bit field guarantee by a wide margin.
pub const fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// `a` precedes or equals `b`, modulo 2^32.
pub const fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

/// `a` follows `b`, modulo 2^32.
pub const fn seq_gt(a: u32, b: u32) -> bool {
    seq_lt(b, a)
}

/// `a` follows or equals `b`, modulo 2^32.
pub const fn seq_ge(a: u32, b: u32) -> bool {
    seq_le(b, a)
}

/// One segment a [`Tcb`] wants transmitted. It owns a copy of its payload so
/// the send buffer can be compacted while the segment waits in the outbox.
#[derive(Debug, Clone)]
pub struct OutSegment {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub header: SegmentHeader,
    pub mss: Option<u16>,
    len: usize,
    data: [u8; MAX_SEG_PAYLOAD],
}

impl OutSegment {
    const EMPTY: OutSegment = OutSegment {
        src: Ipv4Addr::UNSPECIFIED,
        dst: Ipv4Addr::UNSPECIFIED,
        header: SegmentHeader {
            src_port: 0,
            dst_port: 0,
            seq: 0,
            ack: 0,
            flags: 0,
            window: 0,
        },
        mss: None,
        len: 0,
        data: [0; MAX_SEG_PAYLOAD],
    };

    /// The payload bytes this segment carries (possibly empty).
    pub fn payload(&self) -> &[u8] {
        self.data.get(..self.len).unwrap_or(&[])
    }

    /// Encode through [`build_into`] with the segment's own addresses.
    pub fn encode(&self, buf: &mut [u8]) -> Result<usize, TcpError> {
        build_into(
            buf,
            self.src,
            self.dst,
            &self.header,
            self.mss,
            self.payload(),
        )
    }
}

/// Fixed-capacity queue of outgoing segments. `Tcb` methods append; the
/// caller transmits [`Outbox::segments`] and then calls [`Outbox::clear`].
#[derive(Debug, Clone)]
pub struct Outbox {
    segs: [OutSegment; OUTBOX_CAP],
    len: usize,
}

impl Default for Outbox {
    fn default() -> Self {
        Outbox::new()
    }
}

impl Outbox {
    /// An empty outbox.
    pub const fn new() -> Outbox {
        Outbox {
            segs: [OutSegment::EMPTY; OUTBOX_CAP],
            len: 0,
        }
    }

    /// Queued segments, oldest first; transmit them in this order.
    pub fn segments(&self) -> &[OutSegment] {
        self.segs.get(..self.len).unwrap_or(&[])
    }

    /// Number of queued segments.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Forget everything queued; call once the segments are transmitted.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Queue a segment; false when full (the caller treats that as loss).
    fn push(
        &mut self,
        src: Ipv4Addr,
        dst: Ipv4Addr,
        header: SegmentHeader,
        mss: Option<u16>,
        payload: &[u8],
    ) -> bool {
        let Some(slot) = self.segs.get_mut(self.len) else {
            return false;
        };
        let n = payload.len().min(MAX_SEG_PAYLOAD);
        slot.data[..n].copy_from_slice(&payload[..n]);
        slot.len = n;
        slot.src = src;
        slot.dst = dst;
        slot.header = header;
        slot.mss = mss;
        self.len += 1;
        true
    }
}

/// Queue the RST that answers `seg` when no connection wants it (RFC 9293
/// §3.10.7.1). An ACK-bearing segment is refused at the sequence number it
/// acknowledged, which is the one number its sender will accept; one without
/// ACK gets a RST|ACK covering it. A RST is never answered, or two stacks
/// could volley resets forever. Returns whether a reply was queued.
pub fn reset_reply(seg: &Segment, out: &mut Outbox) -> bool {
    let h = &seg.header;
    if h.has(RST) {
        return false;
    }
    let (seq, ack, flags) = if h.has(ACK) {
        (h.ack, 0, RST)
    } else {
        (0, h.seq.wrapping_add(seg.seq_len()), RST | ACK)
    };
    let header = SegmentHeader {
        src_port: h.dst_port,
        dst_port: h.src_port,
        seq,
        ack,
        flags,
        window: 0,
    };
    out.push(seg.dst, seg.src, header, None, &[])
}

/// RFC 9293 connection states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// The most significant transition one call caused; variants are ordered by
/// significance. A segment can cause several (the handshake's final ACK may
/// carry data and a FIN); lesser ones are never lost, only unannounced —
/// [`Tcb::state`] and [`Tcb::recv_available`] always tell the whole story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Event {
    /// Nothing the owner must act on.
    None,
    /// New bytes wait for [`Tcb::recv`].
    DataReadable,
    /// The peer sent FIN: no more data will arrive.
    PeerClosed,
    /// The handshake completed (ESTABLISHED, or FIN-WAIT-1 if `close` came
    /// first).
    Connected,
    /// Graceful teardown finished; the Tcb is now `Closed`.
    Closed,
    /// An acceptable RST arrived; the Tcb is now `Closed`, buffers flushed.
    Reset,
    /// [`MAX_RETRIES`] retransmissions went unanswered; the Tcb is `Closed`.
    TimedOut,
}

/// Clamp a peer's MSS option into what we can honour.
fn clamp_mss(mss: Option<u16>) -> u16 {
    mss.unwrap_or(DEFAULT_MSS).clamp(MIN_PEER_MSS, LOCAL_MSS)
}

/// Transmission Control Block: one connection's entire state, fixed size.
///
/// Send buffer layout: `send_buf[..send_len]` holds the bytes from `snd_una`
/// onward — sent-but-unacknowledged first, then unsent. An ACK drains the
/// front and compacts. The receive window is not stored: it is always the free
/// receive-buffer space, so the advertised right edge can never shrink.
#[derive(Debug, Clone)]
pub struct Tcb {
    state: State,
    local: Ipv4Addr,
    local_port: u16,
    remote: Ipv4Addr,
    remote_port: u16,
    /// Came from `listen`: a RST in SYN-RECEIVED returns to LISTEN.
    passive: bool,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    snd_wnd: u32,
    snd_wl1: u32,
    snd_wl2: u32,
    irs: u32,
    rcv_nxt: u32,
    peer_mss: u16,
    send_buf: [u8; SEND_BUF_LEN],
    send_len: usize,
    recv_buf: [u8; RECV_BUF_LEN],
    recv_len: usize,
    /// `close` was called: FIN goes out once the send buffer drains.
    fin_queued: bool,
    fin_sent: bool,
    rto_ms: u64,
    retries: u8,
    /// Retransmission deadline, or the 2×MSL deadline in TIME-WAIT.
    timer: Option<u64>,
    /// `recv` reopened a small window; `on_timer` advertises it.
    window_update: bool,
}

impl Default for Tcb {
    fn default() -> Self {
        Tcb::new()
    }
}

impl Tcb {
    /// A `Closed` block, the resting state of a free connection slot.
    pub const fn new() -> Tcb {
        Tcb {
            state: State::Closed,
            local: Ipv4Addr::UNSPECIFIED,
            local_port: 0,
            remote: Ipv4Addr::UNSPECIFIED,
            remote_port: 0,
            passive: false,
            iss: 0,
            snd_una: 0,
            snd_nxt: 0,
            snd_wnd: 0,
            snd_wl1: 0,
            snd_wl2: 0,
            irs: 0,
            rcv_nxt: 0,
            peer_mss: DEFAULT_MSS,
            send_buf: [0; SEND_BUF_LEN],
            send_len: 0,
            recv_buf: [0; RECV_BUF_LEN],
            recv_len: 0,
            fin_queued: false,
            fin_sent: false,
            rto_ms: INITIAL_RTO_MS,
            retries: 0,
            timer: None,
            window_update: false,
        }
    }

    /// Active open: queue a SYN (advertising MSS 1460) and enter SYN-SENT.
    /// `iss` is the caller's choice so that it can come from a keyed hash or
    /// RNG the kernel owns. A zero port yields a `Closed` block and no SYN.
    pub fn connect(
        local: Ipv4Addr,
        local_port: u16,
        remote: Ipv4Addr,
        remote_port: u16,
        iss: u32,
        now_ms: u64,
        out: &mut Outbox,
    ) -> Tcb {
        let mut t = Tcb::new();
        t.connect_in_place(local, local_port, remote, remote_port, iss, now_ms, out);
        t
    }

    /// [`Tcb::connect`] into an existing block, reusing it. A kernel keeps its
    /// blocks in a static table, and building one by value costs several
    /// copies of an 8 KB structure — enough, in an unoptimized build, to run
    /// straight off the end of a 32 KB syscall stack (V0.9 found exactly
    /// that). Only the scalar state is reset; the buffers are dead space until
    /// their lengths say otherwise.
    #[allow(clippy::too_many_arguments)]
    pub fn connect_in_place(
        &mut self,
        local: Ipv4Addr,
        local_port: u16,
        remote: Ipv4Addr,
        remote_port: u16,
        iss: u32,
        now_ms: u64,
        out: &mut Outbox,
    ) {
        self.reset_in_place();
        if local_port == 0 || remote_port == 0 {
            return;
        }
        self.local = local;
        self.local_port = local_port;
        self.remote = remote;
        self.remote_port = remote_port;
        self.state = State::SynSent;
        self.iss = iss;
        self.snd_una = iss;
        self.snd_nxt = iss.wrapping_add(1);
        self.send_syn(out);
        self.arm(now_ms);
    }

    /// Return to exactly [`Tcb::new`]'s observable state without building a
    /// second block: every scalar reset, both buffers logically empty.
    pub fn reset_in_place(&mut self) {
        self.state = State::Closed;
        self.local = Ipv4Addr::UNSPECIFIED;
        self.local_port = 0;
        self.remote = Ipv4Addr::UNSPECIFIED;
        self.remote_port = 0;
        self.passive = false;
        self.iss = 0;
        self.snd_una = 0;
        self.snd_nxt = 0;
        self.snd_wnd = 0;
        self.snd_wl1 = 0;
        self.snd_wl2 = 0;
        self.irs = 0;
        self.rcv_nxt = 0;
        self.peer_mss = DEFAULT_MSS;
        self.send_len = 0;
        self.recv_len = 0;
        self.fin_queued = false;
        self.fin_sent = false;
        self.rto_ms = INITIAL_RTO_MS;
        self.retries = 0;
        self.timer = None;
        self.window_update = false;
    }

    /// Passive open: wait in LISTEN for a SYN addressed to `local:local_port`.
    pub fn listen(local: Ipv4Addr, local_port: u16, iss: u32) -> Tcb {
        let mut t = Tcb::new();
        if local_port != 0 {
            t.local = local;
            t.local_port = local_port;
            t.state = State::Listen;
            t.passive = true;
            t.iss = iss;
        }
        t
    }

    /// Current RFC 9293 state.
    pub fn state(&self) -> State {
        self.state
    }

    /// Local address and port.
    pub fn local(&self) -> (Ipv4Addr, u16) {
        (self.local, self.local_port)
    }

    /// The peer; unspecified until a LISTEN block accepts a SYN.
    pub fn remote(&self) -> (Ipv4Addr, u16) {
        (self.remote, self.remote_port)
    }

    /// Our initial send sequence number.
    pub fn iss(&self) -> u32 {
        self.iss
    }

    /// The peer's initial sequence number, once its SYN has arrived.
    pub fn irs(&self) -> u32 {
        self.irs
    }

    /// Oldest sequence number sent but not yet acknowledged.
    pub fn snd_una(&self) -> u32 {
        self.snd_una
    }

    /// Next sequence number to send.
    pub fn snd_nxt(&self) -> u32 {
        self.snd_nxt
    }

    /// The peer's advertised window.
    pub fn snd_wnd(&self) -> u32 {
        self.snd_wnd
    }

    /// Next sequence number expected from the peer.
    pub fn rcv_nxt(&self) -> u32 {
        self.rcv_nxt
    }

    /// Our receive window: the free receive-buffer space.
    pub fn rcv_wnd(&self) -> u32 {
        RECV_BUF_LEN.saturating_sub(self.recv_len) as u32
    }

    /// The peer's MSS option clamped to `MIN_PEER_MSS..=1460`; 536 if absent.
    pub fn peer_mss(&self) -> u16 {
        self.peer_mss
    }

    /// Bytes waiting for [`Tcb::recv`].
    pub fn recv_available(&self) -> usize {
        self.recv_len
    }

    /// Queue up to `data.len()` bytes and transmit what the peer's window
    /// allows. Returns the bytes accepted: 0 unless ESTABLISHED or CLOSE-WAIT
    /// (no sending after `close`), or when the send buffer is full.
    pub fn send(&mut self, data: &[u8], now_ms: u64, out: &mut Outbox) -> usize {
        if !matches!(self.state, State::Established | State::CloseWait) {
            return 0;
        }
        let n = data.len().min(SEND_BUF_LEN.saturating_sub(self.send_len));
        if let (Some(dst), Some(src)) = (
            self.send_buf.get_mut(self.send_len..self.send_len + n),
            data.get(..n),
        ) {
            dst.copy_from_slice(src);
            self.send_len += n;
        }
        self.push_data(now_ms, out);
        n
    }

    /// Copy received bytes into `dst`, freeing window. Works in any state, so
    /// data that arrived before a FIN stays readable after it. When this
    /// reopens a window that had fallen below one MSS, the next
    /// [`Tcb::on_timer`] advertises it — otherwise a peer stalled on our full
    /// buffer would never learn it may continue.
    pub fn recv(&mut self, dst: &mut [u8]) -> usize {
        let n = dst.len().min(self.recv_len);
        let before = self.rcv_wnd();
        if let (Some(d), Some(s)) = (dst.get_mut(..n), self.recv_buf.get(..n)) {
            d.copy_from_slice(s);
        }
        self.recv_buf.copy_within(n..self.recv_len, 0);
        self.recv_len -= n;
        let threshold = (RECV_BUF_LEN / 2).min(MAX_SEG_PAYLOAD) as u32;
        let peer_may_send = matches!(
            self.state,
            State::Established | State::FinWait1 | State::FinWait2
        );
        if peer_may_send && before < threshold && self.rcv_wnd() >= threshold {
            self.window_update = true;
        }
        n
    }

    /// Graceful close: FIN follows any data still queued. LISTEN and SYN-SENT
    /// have nothing to tear down and close at once ([`Event::Closed`]); in
    /// SYN-RECEIVED the FIN waits for the handshake to finish.
    pub fn close(&mut self, now_ms: u64, out: &mut Outbox) -> Event {
        match self.state {
            State::Listen | State::SynSent => {
                self.teardown();
                return Event::Closed;
            }
            State::SynReceived => self.fin_queued = true,
            State::Established | State::CloseWait => {
                self.fin_queued = true;
                self.state = if self.state == State::Established {
                    State::FinWait1
                } else {
                    State::LastAck
                };
                self.push_data(now_ms, out);
            }
            _ => {}
        }
        Event::None
    }

    /// Abortive close: flush everything and, where the peer holds state for
    /// this connection (RFC 9293 §3.10.5), queue a RST at `snd_nxt`.
    pub fn abort(&mut self, out: &mut Outbox) {
        if matches!(
            self.state,
            State::SynReceived
                | State::Established
                | State::FinWait1
                | State::FinWait2
                | State::CloseWait
        ) {
            self.emit(out, self.snd_nxt, RST, None, &[]);
        }
        self.teardown();
    }

    /// Drive time: send a pending window update, retry data a full outbox
    /// held back, and fire the retransmission or TIME-WAIT timer. Call it on
    /// every tick and after `recv`.
    pub fn on_timer(&mut self, now_ms: u64, out: &mut Outbox) -> Event {
        if self.window_update {
            self.window_update = false;
            self.send_ack(out);
        }
        self.push_data(now_ms, out);
        let Some(deadline) = self.timer else {
            return Event::None;
        };
        if now_ms < deadline {
            return Event::None;
        }
        if self.state == State::TimeWait {
            self.state = State::Closed;
            self.timer = None;
            return Event::Closed;
        }
        if self.in_flight() == 0 {
            self.timer = None;
            return Event::None;
        }
        if self.retries >= MAX_RETRIES {
            self.teardown();
            return Event::TimedOut;
        }
        self.retries += 1;
        self.rto_ms = self.rto_ms.saturating_mul(2).min(MAX_RTO_MS);
        self.timer = Some(now_ms.saturating_add(self.rto_ms));
        self.retransmit(out);
        Event::None
    }

    /// Process one validated segment (RFC 9293 §3.10.7). Segments not
    /// addressed to this connection's 4-tuple are ignored; a CLOSED block
    /// ignores everything — answer those with [`reset_reply`].
    pub fn on_segment(&mut self, seg: &Segment, now_ms: u64, out: &mut Outbox) -> Event {
        let h = &seg.header;
        if seg.dst != self.local || h.dst_port != self.local_port {
            return Event::None;
        }
        match self.state {
            State::Closed => Event::None,
            State::Listen => {
                self.on_listen(seg, now_ms, out);
                Event::None
            }
            _ if seg.src != self.remote || h.src_port != self.remote_port => Event::None,
            State::SynSent => self.on_syn_sent(seg, now_ms, out),
            _ => self.on_synchronized(seg, now_ms, out),
        }
    }

    fn on_listen(&mut self, seg: &Segment, now_ms: u64, out: &mut Outbox) {
        let h = &seg.header;
        if h.has(RST) {
            return;
        }
        if h.has(ACK) {
            // Acknowledges a connection that does not exist here.
            reset_reply(seg, out);
            return;
        }
        if !h.has(SYN) {
            return;
        }
        self.remote = seg.src;
        self.remote_port = h.src_port;
        self.learn_peer(seg);
        self.snd_una = self.iss;
        self.snd_nxt = self.iss.wrapping_add(1);
        self.state = State::SynReceived;
        self.send_syn(out);
        self.arm(now_ms);
    }

    fn on_syn_sent(&mut self, seg: &Segment, now_ms: u64, out: &mut Outbox) -> Event {
        let h = &seg.header;
        let ack_ok = seq_gt(h.ack, self.iss) && seq_le(h.ack, self.snd_nxt);
        if h.has(ACK) && !ack_ok {
            // Acknowledges something we never sent: stale or forged.
            reset_reply(seg, out);
            return Event::None;
        }
        if h.has(RST) {
            // RFC 5961 §3: only a RST that acknowledges our SYN is believed.
            if h.has(ACK) {
                self.teardown();
                return Event::Reset;
            }
            return Event::None;
        }
        if !h.has(SYN) {
            return Event::None;
        }
        self.learn_peer(seg);
        if h.has(ACK) {
            self.take_ack(h.ack, now_ms);
            self.state = State::Established;
            self.send_ack(out);
            Event::Connected
        } else {
            // Simultaneous open: answer with SYN-ACK from our original ISS.
            self.state = State::SynReceived;
            self.send_syn(out);
            Event::None
        }
    }

    fn on_synchronized(&mut self, seg: &Segment, now_ms: u64, out: &mut Outbox) -> Event {
        let h = &seg.header;
        // A repeated SYN means our SYN-ACK was lost: answer now, not at RTO.
        if self.state == State::SynReceived
            && h.flags & (SYN | ACK | RST) == SYN
            && h.seq == self.irs
        {
            self.send_syn(out);
            return Event::None;
        }
        if !self.acceptable(seg) {
            // Old duplicates and out-of-window segments are answered with our
            // view of the connection; a RST is never answered.
            if !h.has(RST) {
                if self.state == State::TimeWait && h.has(FIN) {
                    self.timer = Some(now_ms.saturating_add(TIME_WAIT_MS));
                }
                self.send_ack(out);
            }
            return Event::None;
        }
        if h.has(RST) {
            if h.seq != self.rcv_nxt {
                self.send_ack(out); // RFC 5961 §3.2 challenge ACK
                return Event::None;
            }
            if self.state == State::SynReceived && self.passive {
                *self = Tcb::listen(self.local, self.local_port, self.iss);
                return Event::None;
            }
            self.teardown();
            return Event::Reset;
        }
        if h.has(SYN) {
            self.send_ack(out); // RFC 5961 §4 challenge ACK
            return Event::None;
        }
        if !h.has(ACK) {
            return Event::None;
        }

        let mut event = Event::None;
        if self.state == State::SynReceived {
            if !(seq_gt(h.ack, self.snd_una) && seq_le(h.ack, self.snd_nxt)) {
                reset_reply(seg, out);
                return Event::None;
            }
            self.take_ack(h.ack, now_ms);
            self.set_window(h);
            self.state = if self.fin_queued {
                State::FinWait1
            } else {
                State::Established
            };
            event = Event::Connected;
        } else if seq_gt(h.ack, self.snd_nxt) {
            // Acknowledges data never sent (RFC 9293 §3.10.7.4): answer, drop.
            self.send_ack(out);
            return Event::None;
        } else if seq_gt(h.ack, self.snd_una) {
            self.take_ack(h.ack, now_ms);
        }
        // Only a segment at least as new as the last window-setter may move
        // the window, so a reordered old segment cannot resize it.
        if seq_le(self.snd_una, h.ack)
            && (seq_lt(self.snd_wl1, h.seq)
                || (self.snd_wl1 == h.seq && seq_le(self.snd_wl2, h.ack)))
        {
            self.set_window(h);
        }
        let fin_acked = self.fin_sent && self.snd_una == self.snd_nxt;
        match self.state {
            State::FinWait1 if fin_acked => self.state = State::FinWait2,
            State::Closing if fin_acked => self.enter_time_wait(now_ms),
            State::LastAck if fin_acked => {
                self.state = State::Closed;
                self.timer = None;
                return Event::Closed;
            }
            _ => {}
        }

        let mut need_ack = false;
        let receiving = matches!(
            self.state,
            State::Established | State::FinWait1 | State::FinWait2
        );
        if seq_gt(h.seq, self.rcv_nxt) {
            // Out of order: not queued; the ACK tells the sender where the
            // hole is and its retransmission timer fills it.
            need_ack = true;
        } else if receiving {
            // Trim what we already hold. A skip past the payload means even
            // the FIN is a duplicate.
            let skip = self.rcv_nxt.wrapping_sub(h.seq) as usize;
            let (data, mut fin) = match seg.payload.get(skip..) {
                Some(d) => (d, h.has(FIN)),
                None => (&[][..], false),
            };
            if !data.is_empty() {
                let n = data.len().min(RECV_BUF_LEN - self.recv_len);
                if let (Some(dst), Some(src)) = (
                    self.recv_buf.get_mut(self.recv_len..self.recv_len + n),
                    data.get(..n),
                ) {
                    dst.copy_from_slice(src);
                    self.recv_len += n;
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
                }
                if n > 0 {
                    event = event.max(Event::DataReadable);
                }
                // A FIN beyond bytes we could not take is beyond the window.
                fin &= n == data.len();
                need_ack = true;
            }
            if fin {
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                need_ack = true;
                event = event.max(Event::PeerClosed);
                match self.state {
                    State::Established => self.state = State::CloseWait,
                    State::FinWait1 => self.state = State::Closing,
                    _ => self.enter_time_wait(now_ms), // FIN-WAIT-2
                }
            }
        }
        let queued = out.len();
        self.push_data(now_ms, out);
        if need_ack && out.len() == queued {
            self.send_ack(out);
        }
        event
    }

    /// RFC 9293 §3.10.7.4 acceptability test. With a zero window only
    /// `seq == rcv_nxt` passes, whatever the length, so ACKs and RSTs still
    /// get through to a connection whose receive buffer is full.
    fn acceptable(&self, seg: &Segment) -> bool {
        let seq = seg.header.seq;
        let wnd = self.rcv_wnd();
        let in_window =
            |s: u32| seq_le(self.rcv_nxt, s) && seq_lt(s, self.rcv_nxt.wrapping_add(wnd));
        match (seg.seq_len(), wnd) {
            (_, 0) => seq == self.rcv_nxt,
            (0, _) => in_window(seq),
            (len, _) => in_window(seq) || in_window(seq.wrapping_add(len - 1)),
        }
    }

    /// Record what a SYN told us about the peer.
    fn learn_peer(&mut self, seg: &Segment) {
        self.irs = seg.header.seq;
        self.rcv_nxt = seg.header.seq.wrapping_add(1);
        self.peer_mss = clamp_mss(seg.mss);
        self.set_window(&seg.header);
    }

    fn set_window(&mut self, h: &SegmentHeader) {
        self.snd_wnd = h.window as u32;
        self.snd_wl1 = h.seq;
        self.snd_wl2 = h.ack;
    }

    /// Advance `snd_una` to an acceptable `ack`: consume the SYN's sequence
    /// number if it was outstanding, drain acknowledged data, and restart the
    /// retransmission clock (fresh RTO, zero retries) for what remains.
    fn take_ack(&mut self, ack: u32, now_ms: u64) {
        let mut acked = ack.wrapping_sub(self.snd_una) as usize;
        if self.syn_outstanding() {
            acked = acked.saturating_sub(1);
        }
        let data = acked.min(self.send_len);
        self.send_buf.copy_within(data..self.send_len, 0);
        self.send_len -= data;
        self.snd_una = ack;
        self.rto_ms = INITIAL_RTO_MS;
        self.retries = 0;
        self.timer = None;
        if self.in_flight() > 0 {
            self.arm(now_ms);
        }
    }

    /// Transmit unsent data within `min(snd_wnd, 4 × peer MSS)` bytes in
    /// flight, in peer-MSS segments, then the FIN once the buffer has drained.
    fn push_data(&mut self, now_ms: u64, out: &mut Outbox) {
        // CLOSING is here because the peer's FIN can overtake a FIN of ours
        // still queued behind unsent data; that data and FIN must still go.
        let sending = matches!(
            self.state,
            State::Established
                | State::CloseWait
                | State::FinWait1
                | State::Closing
                | State::LastAck
        );
        if !sending || self.fin_sent {
            return;
        }
        let mss = self.peer_mss as usize;
        let limit = (self.snd_wnd as usize).min(mss * MAX_IN_FLIGHT_SEGMENTS);
        loop {
            let sent = self.sent_data();
            let unsent = self.send_len - sent;
            let in_flight = self.in_flight();
            if unsent == 0 || in_flight >= limit {
                break;
            }
            let n = unsent.min(limit - in_flight).min(mss);
            let flags = if n == unsent { ACK | PSH } else { ACK };
            let Some(chunk) = self.send_buf.get(sent..sent + n) else {
                break;
            };
            if !self.emit(out, self.snd_nxt, flags, None, chunk) {
                break; // outbox full: the rest waits for the next call
            }
            self.snd_nxt = self.snd_nxt.wrapping_add(n as u32);
            self.arm(now_ms);
        }
        if self.fin_queued && self.sent_data() == self.send_len {
            // Counted as sent even if the outbox was full: the RTO recovers
            // a dropped FIN like any lost segment.
            self.emit(out, self.snd_nxt, FIN | ACK, None, &[]);
            self.fin_sent = true;
            self.snd_nxt = self.snd_nxt.wrapping_add(1);
            self.arm(now_ms);
        }
    }

    /// Resend the oldest unacknowledged segment: the SYN (or SYN-ACK), else
    /// up to one MSS of data from `snd_una` — the FIN rides along when it
    /// fits — else the lone FIN.
    fn retransmit(&mut self, out: &mut Outbox) {
        if self.syn_outstanding() {
            self.send_syn(out);
            return;
        }
        let sent = self.sent_data();
        let n = sent.min(self.peer_mss as usize);
        let mut flags = if n > 0 && n == sent { ACK | PSH } else { ACK };
        if self.fin_outstanding() && n == sent {
            flags |= FIN;
        }
        if let Some(chunk) = self.send_buf.get(..n) {
            self.emit(out, self.snd_una, flags, None, chunk);
        }
    }

    fn syn_outstanding(&self) -> bool {
        matches!(self.state, State::SynSent | State::SynReceived)
    }

    fn fin_outstanding(&self) -> bool {
        self.fin_sent && self.snd_una != self.snd_nxt
    }

    /// Sequence space sent but not yet acknowledged (SYN and FIN included).
    fn in_flight(&self) -> usize {
        self.snd_nxt.wrapping_sub(self.snd_una) as usize
    }

    /// Data bytes (no SYN/FIN) sent but not yet acknowledged.
    fn sent_data(&self) -> usize {
        let control = self.syn_outstanding() as usize + self.fin_outstanding() as usize;
        self.in_flight().saturating_sub(control).min(self.send_len)
    }

    fn arm(&mut self, now_ms: u64) {
        if self.timer.is_none() {
            self.timer = Some(now_ms.saturating_add(self.rto_ms));
        }
    }

    fn enter_time_wait(&mut self, now_ms: u64) {
        self.state = State::TimeWait;
        self.timer = Some(now_ms.saturating_add(TIME_WAIT_MS));
    }

    /// Abrupt end (RST, timeout, abort): nothing survives.
    fn teardown(&mut self) {
        self.state = State::Closed;
        self.timer = None;
        self.send_len = 0;
        self.recv_len = 0;
        self.fin_queued = false;
        self.fin_sent = false;
        self.window_update = false;
    }

    /// Queue a segment from this connection. ACK-bearing segments carry
    /// `rcv_nxt`, and every segment advertises the current receive window.
    fn emit(
        &self,
        out: &mut Outbox,
        seq: u32,
        flags: u8,
        mss: Option<u16>,
        payload: &[u8],
    ) -> bool {
        let header = SegmentHeader {
            src_port: self.local_port,
            dst_port: self.remote_port,
            seq,
            ack: if flags & ACK != 0 { self.rcv_nxt } else { 0 },
            flags,
            window: self.rcv_wnd().min(u16::MAX as u32) as u16,
        };
        out.push(self.local, self.remote, header, mss, payload)
    }

    fn send_ack(&self, out: &mut Outbox) {
        self.emit(out, self.snd_nxt, ACK, None, &[]);
    }

    /// SYN from SYN-SENT, SYN-ACK from SYN-RECEIVED; always from `iss`.
    fn send_syn(&self, out: &mut Outbox) {
        let flags = if self.state == State::SynSent {
            SYN
        } else {
            SYN | ACK
        };
        self.emit(out, self.iss, flags, Some(LOCAL_MSS), &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const A: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
    const B: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
    const AP: u16 = 40000;
    const BP: u16 = 80;
    const ISS: u32 = 1000;
    /// The scripted peer's ISS is 5000, so this is `rcv_nxt` once synced.
    const RCV: u32 = 5001;

    type Sent = (SegmentHeader, Option<u16>, Vec<u8>);

    /// A Tcb at A:40000 and a scripted peer at B:80. Every peer segment goes
    /// through `build_into` and `parse`, so the codec is always in the loop.
    struct Peer {
        t: Tcb,
        out: Outbox,
        /// Window and MSS option the peer's next segments carry.
        win: u16,
        mss: Option<u16>,
    }

    impl Peer {
        fn new(t: Tcb, out: Outbox) -> Peer {
            let (win, mss) = (8192, None);
            Peer { t, out, win, mss }
        }

        fn connect() -> Peer {
            let mut out = Outbox::new();
            let t = Tcb::connect(A, AP, B, BP, ISS, 0, &mut out);
            Peer::new(t, out)
        }

        /// The Tcb receives one segment from the peer.
        fn rx(&mut self, now: u64, seq: u32, ack: u32, flags: u8, data: &[u8]) -> Event {
            let (src_port, dst_port, window) = (BP, AP, self.win);
            let h = SegmentHeader {
                src_port,
                dst_port,
                seq,
                ack,
                flags,
                window,
            };
            let mut buf = [0u8; 2048];
            let n = build_into(&mut buf, B, A, &h, self.mss, data).unwrap();
            let seg = parse(&buf[..n], B, A).unwrap();
            self.t.on_segment(&seg, now, &mut self.out)
        }

        fn drain(&mut self) -> Vec<Sent> {
            let segs = self.out.segments().iter();
            let v = segs
                .map(|s| (s.header, s.mss, s.payload().to_vec()))
                .collect();
            self.out.clear();
            v
        }

        /// `(flags, seq, ack)` of every queued segment, draining them.
        fn sent(&mut self) -> Vec<(u8, u32, u32)> {
            let v = self.drain();
            v.iter().map(|s| (s.0.flags, s.0.seq, s.0.ack)).collect()
        }
    }

    #[test]
    fn a_reused_block_connects_exactly_like_a_fresh_one() {
        // Leave a block mid-transfer with buffered data both ways, then reuse
        // it in place: nothing of the old connection may leak into the new.
        let mut used = established(8192, 1460);
        used.t.send(b"stale outgoing", 0, &mut used.out);
        used.rx(0, RCV, ISS + 1, ACK, b"stale incoming");
        used.out.clear();
        used.t
            .connect_in_place(A, AP + 1, B, BP, ISS + 77, 5, &mut used.out);
        let mut fresh_out = Outbox::new();
        let fresh = Tcb::connect(A, AP + 1, B, BP, ISS + 77, 5, &mut fresh_out);
        let (r, f) = (&used.t, &fresh);
        assert_eq!(r.state(), State::SynSent);
        assert_eq!(
            (
                r.state(),
                r.local(),
                r.remote(),
                r.iss(),
                r.snd_una(),
                r.snd_nxt()
            ),
            (
                f.state(),
                f.local(),
                f.remote(),
                f.iss(),
                f.snd_una(),
                f.snd_nxt()
            )
        );
        assert_eq!((r.recv_available(), r.rcv_wnd()), (0, RECV_BUF_LEN as u32));
        assert_eq!((r.rcv_nxt(), r.irs(), r.peer_mss()), (0, 0, DEFAULT_MSS));
        let (a, b) = (used.out.segments(), fresh_out.segments());
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].header, a[0].mss), (b[0].header, b[0].mss));
        assert!(a[0].payload().is_empty());
        // A zero port leaves a reused block closed, like a fresh one.
        used.out.clear();
        used.t.connect_in_place(A, 0, B, BP, 1, 0, &mut used.out);
        assert_eq!(used.t.state(), State::Closed);
        assert!(used.out.is_empty());
    }

    /// A client connection ESTABLISHED with a peer advertising `win`/`mss`.
    fn established(win: u16, mss: u16) -> Peer {
        let mut p = Peer::connect();
        (p.win, p.mss) = (win, Some(mss));
        assert_eq!(p.rx(0, RCV - 1, ISS + 1, SYN | ACK, b""), Event::Connected);
        p.mss = None;
        p.out.clear();
        p
    }

    /// Recompute the checksum over a hand-edited segment from A to B.
    fn reseal(seg: &mut [u8]) {
        let sum = checksum::transport_checksum(A.octets(), B.octets(), 6, seg, 16, &[]);
        seg[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2].copy_from_slice(&sum.unwrap().to_be_bytes());
    }

    fn built(payload: &[u8]) -> ([u8; 128], usize) {
        let mut buf = [0u8; 128];
        let (src_port, dst_port) = (AP, BP);
        let h = SegmentHeader {
            src_port,
            dst_port,
            ..SegmentHeader::default()
        };
        let n = build_into(&mut buf, A, B, &h, None, payload).unwrap();
        (buf, n)
    }

    #[test]
    fn codec_round_trips_with_and_without_mss() {
        let (seq, ack, flags) = (0xDEAD_BEEF, 0x0102_0304, SYN | ACK);
        let h = SegmentHeader {
            src_port: AP,
            dst_port: BP,
            seq,
            ack,
            flags,
            window: 4096,
        };
        let mut buf = [0u8; 128];
        let n = build_into(&mut buf, A, B, &h, Some(1460), b"").unwrap();
        let s = parse(&buf[..n], A, B).unwrap();
        let got = (n, s.src, s.dst, s.header, s.mss, s.seq_len());
        assert_eq!(got, (24, A, B, h, Some(1460), 1));
        let n = build_into(&mut buf, A, B, &h, None, b"odd").unwrap();
        let s = parse(&buf[..n], A, B).unwrap();
        assert_eq!((n, s.mss, s.payload), (23, None, &b"odd"[..]));
        assert!(s.has(SYN) && s.has(ACK) && !s.has(FIN));
        let small = build_into(&mut buf[..22], A, B, &h, None, b"abc");
        assert_eq!(small, Err(TcpError::BufferTooSmall));
        let (mut big, huge) = (vec![0u8; 70_000], vec![0u8; MAX_SEGMENT_LEN]);
        let too_big = build_into(&mut big, A, B, &h, None, &huge);
        assert_eq!(too_big, Err(TcpError::PayloadTooLarge));
    }

    #[test]
    fn the_checksum_is_mandatory_and_bound_to_the_addresses() {
        let (buf, n) = built(b"hello");
        for i in HEADER_LEN..n {
            let mut bad = buf;
            bad[i] ^= 0x20;
            assert_eq!(parse(&bad[..n], A, B), Err(TcpError::BadChecksum));
        }
        let other = Ipv4Addr::new(10, 0, 2, 16);
        assert_eq!(parse(&buf[..n], other, B), Err(TcpError::BadChecksum));
        assert_eq!(parse(&buf[..n], A, other), Err(TcpError::BadChecksum));
        // A zeroed field is not "no checksum" in TCP: it must verify.
        let mut zeroed = buf;
        assert_ne!(zeroed[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2], [0, 0]);
        zeroed[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2].fill(0);
        assert_eq!(parse(&zeroed[..n], A, B), Err(TcpError::BadChecksum));
    }

    #[test]
    fn rejects_truncation_and_bad_data_offsets() {
        let (mut buf, n) = built(b"abcd");
        for len in 0..HEADER_LEN {
            assert_eq!(parse(&buf[..len], A, B), Err(TcpError::TooShort));
        }
        for words in [0u8, 1, 4] {
            buf[12] = words << 4;
            assert_eq!(parse(&buf[..n], A, B), Err(TcpError::BadDataOffset));
        }
        buf[12] = 15 << 4; // 60 bytes claimed, 24 present
        assert_eq!(parse(&buf[..n], A, B), Err(TcpError::HeaderPastBuffer));
        buf[12] = 6 << 4;
        reseal(&mut buf[..n]);
        // "abcd" is now option bytes: kind 'a' with length 'b' overruns.
        assert_eq!(parse(&buf[..n], A, B), Err(TcpError::BadOption));
    }

    fn with_options(opts: [u8; 8]) -> Result<Option<u16>, TcpError> {
        let mut seg = [0u8; 28];
        seg[..20].copy_from_slice(&built(b"").0[..20]);
        seg[12] = 7 << 4;
        seg[20..].copy_from_slice(&opts);
        reseal(&mut seg);
        parse(&seg, A, B).map(|s| s.mss)
    }

    #[test]
    fn walks_options_and_rejects_overruns() {
        let ok = with_options([1, 1, 2, 4, 0x05, 0xB4, 0, 0]);
        assert_eq!(ok, Ok(Some(1460)), "NOP NOP MSS EOL");
        let skipped = with_options([30, 6, 9, 9, 9, 9, 1, 0]);
        assert_eq!(skipped, Ok(None), "unknown kind skipped by length");
        let after_eol = with_options([0, 2, 4, 0, 0, 0, 0, 0]);
        assert_eq!(after_eol, Ok(None), "nothing after EOL is read");
        for bad in [
            [3, 1, 0, 0, 0, 0, 0, 0],  // length below 2
            [3, 0, 0, 0, 0, 0, 0, 0],  // length 0 would stall the walk
            [1, 1, 1, 1, 1, 1, 30, 3], // runs past the header
            [1, 1, 1, 1, 1, 1, 1, 30], // kind with no length byte
            [2, 3, 5, 1, 1, 1, 1, 1],  // MSS that is not 4 bytes
        ] {
            assert_eq!(with_options(bad), Err(TcpError::BadOption), "{bad:?}");
        }
    }

    #[test]
    fn rejects_port_zero_in_both_directions() {
        let mut buf = [0u8; 64];
        for (src_port, dst_port) in [(0, BP), (AP, 0)] {
            let h = SegmentHeader {
                src_port,
                dst_port,
                ..SegmentHeader::default()
            };
            let got = build_into(&mut buf, A, B, &h, None, b"");
            assert_eq!(got, Err(TcpError::ZeroPort));
        }
        for at in [0, 2] {
            let (mut wire, n) = built(b"x");
            wire[at..at + 2].fill(0);
            reseal(&mut wire[..n]);
            assert_eq!(parse(&wire[..n], A, B), Err(TcpError::ZeroPort));
        }
    }

    #[test]
    fn sequence_comparisons_survive_the_wrap() {
        assert!(seq_lt(0xFFFF_FFF0, 0x10) && seq_gt(0x10, 0xFFFF_FFF0));
        assert!(seq_lt(u32::MAX, 0) && !seq_lt(0, u32::MAX));
        assert!(seq_le(7, 7) && seq_ge(7, 7) && !seq_lt(7, 7) && !seq_gt(7, 7));
        assert!(seq_le(u32::MAX, 3) && seq_ge(3, u32::MAX));
        // Just under half the space apart is still ordered correctly.
        assert!(seq_lt(0, 0x7FFF_FFFF) && seq_gt(0x8000_0005, 6));
    }

    #[test]
    fn client_handshake_against_a_scripted_peer() {
        let mut p = Peer::connect();
        assert_eq!(p.t.state(), State::SynSent);
        let s = p.drain();
        assert_eq!(
            (s.len(), s[0].0.flags, s[0].0.seq, s[0].1),
            (1, SYN, ISS, Some(1460))
        );
        // A SYN-ACK acknowledging something never sent draws a RST at its
        // ack number and changes nothing.
        assert_eq!(p.rx(0, RCV - 1, ISS + 7, SYN | ACK, b""), Event::None);
        assert_eq!(p.sent(), [(RST, ISS + 7, 0)]);
        assert_eq!(p.t.state(), State::SynSent);
        p.mss = Some(1000);
        assert_eq!(p.rx(0, RCV - 1, ISS + 1, SYN | ACK, b""), Event::Connected);
        assert_eq!(p.sent(), [(ACK, ISS + 1, RCV)]);
        let t = &p.t;
        assert_eq!(
            (t.state(), t.peer_mss(), t.snd_wnd()),
            (State::Established, 1000, 8192)
        );
        let seqs = (t.irs(), t.snd_una(), t.snd_nxt(), t.rcv_nxt());
        assert_eq!(seqs, (RCV - 1, ISS + 1, ISS + 1, RCV));
    }

    #[test]
    fn server_handshake_from_listen() {
        let mut p = Peer::new(Tcb::listen(A, AP, 7000), Outbox::new());
        // A stray ACK to a listener is answered with RST at its ack number.
        assert_eq!(p.rx(0, 1, 4242, ACK, b""), Event::None);
        assert_eq!(p.sent(), [(RST, 4242, 0)]);
        assert_eq!(p.rx(0, RCV - 1, 0, SYN, b""), Event::None);
        let t = &p.t;
        assert_eq!(
            (t.state(), t.remote(), t.peer_mss()),
            (State::SynReceived, (B, BP), 536)
        );
        let s = p.drain();
        assert_eq!(
            (s[0].0.flags, s[0].0.seq, s[0].0.ack),
            (SYN | ACK, 7000, RCV)
        );
        assert_eq!((s[0].1, s[0].0.window as usize), (Some(1460), RECV_BUF_LEN));
        // The peer repeats its SYN (our SYN-ACK was lost): answered at once.
        p.rx(5, RCV - 1, 0, SYN, b"");
        assert_eq!(p.sent(), [(SYN | ACK, 7000, RCV)]);
        assert_eq!(p.rx(10, RCV, 7001, ACK, b""), Event::Connected);
        assert_eq!((p.t.state(), p.out.len()), (State::Established, 0));
    }

    #[test]
    fn sends_are_segmented_at_peer_mss_and_stop_at_the_window() {
        let mut p = established(1000, 536);
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 7 + 1) as u8).collect();
        assert_eq!(p.t.send(&data, 0, &mut p.out), 3000);
        // Window 1000 < 4 × 536: two segments, stopping exactly at the window.
        let s = p.drain();
        let lens: Vec<usize> = s.iter().map(|x| x.2.len()).collect();
        assert_eq!(
            (lens, s[0].0.seq, s[1].0.seq),
            (vec![536, 464], ISS + 1, ISS + 537)
        );
        assert_eq!([&s[0].2[..], &s[1].2[..]].concat(), &data[..1000]);
        // Everything acknowledged, but the window closes: nothing may go out.
        p.win = 0;
        assert_eq!(p.rx(10, RCV, ISS + 1001, ACK, b""), Event::None);
        assert_eq!((p.out.len(), p.t.snd_una()), (0, ISS + 1001));
        // A window update (same seq and ack) resumes the transfer.
        p.win = 2000;
        p.rx(20, RCV, ISS + 1001, ACK, b"");
        let s = p.drain();
        let lens: Vec<usize> = s.iter().map(|x| x.2.len()).collect();
        assert_eq!(lens, [536, 536, 536, 392]);
        let bytes: Vec<u8> = s.iter().flat_map(|x| x.2.clone()).collect();
        assert_eq!(bytes, &data[1000..]);
        assert_eq!(s[3].0.flags, ACK | PSH, "PSH marks the end of the buffer");
        p.rx(30, RCV, ISS + 3001, ACK, b"");
        assert_eq!(
            (p.t.snd_una(), p.t.snd_nxt(), p.out.len()),
            (ISS + 3001, ISS + 3001, 0)
        );
        // The other direction: in-order segments are ACKed and readable.
        let ev = p.rx(40, RCV, ISS + 3001, ACK | PSH, b"hello, ");
        assert_eq!(
            (ev, p.sent()),
            (Event::DataReadable, vec![(ACK, ISS + 3001, RCV + 7)])
        );
        assert_eq!(
            p.rx(41, RCV + 7, ISS + 3001, ACK, b"world"),
            Event::DataReadable
        );
        assert_eq!(p.drain()[0].0.window as usize, RECV_BUF_LEN - 12);
        let mut got = [0u8; 64];
        let n = p.t.recv(&mut got);
        assert_eq!(
            (&got[..n], p.t.rcv_wnd() as usize),
            (&b"hello, world"[..], RECV_BUF_LEN)
        );
    }

    #[test]
    fn the_advertised_window_is_the_free_receive_space() {
        let mut p = established(8192, 1460);
        let chunk = [0x5Au8; 1460];
        p.rx(0, RCV, ISS + 1, ACK, &chunk);
        p.rx(0, RCV + 1460, ISS + 1, ACK, &chunk);
        // The third only partly fits: the rest, and its FIN, are not taken.
        p.rx(0, RCV + 2920, ISS + 1, ACK | FIN, &chunk);
        let full = RCV + RECV_BUF_LEN as u32;
        let last = p.drain().last().unwrap().0;
        assert_eq!(
            (last.ack, last.window, p.t.state()),
            (full, 0, State::Established)
        );
        // Zero window: data at rcv_nxt is refused, but still ACKed.
        assert_eq!(p.rx(0, full, ISS + 1, ACK, b"x"), Event::None);
        let s = p.drain();
        assert_eq!((s[0].0.ack, s[0].0.window), (full, 0));
        // Reading reopens the window, and the next tick advertises it.
        assert_eq!(p.t.recv(&mut [0u8; 2048]), 2048);
        assert_eq!(p.t.on_timer(1, &mut p.out), Event::None);
        let s = p.drain();
        assert_eq!((s[0].0.flags, s[0].0.window), (ACK, 2048));
    }

    #[test]
    fn retransmits_the_oldest_segment_with_backoff_then_times_out() {
        let mut p = established(8192, 536);
        p.t.send(b"payload", 0, &mut p.out);
        let first = p.drain();
        let (mut deadline, mut rto) = (INITIAL_RTO_MS, INITIAL_RTO_MS);
        for _ in 0..MAX_RETRIES {
            assert_eq!(p.t.on_timer(deadline - 1, &mut p.out), Event::None);
            assert!(p.out.is_empty(), "nothing before the deadline");
            assert_eq!(p.t.on_timer(deadline, &mut p.out), Event::None);
            assert_eq!(p.drain(), first, "exactly the same segment");
            rto *= 2;
            deadline += rto;
        }
        assert_eq!(deadline, 38_100, "300 + 600 + ... + 19200");
        assert_eq!(p.t.on_timer(deadline - 1, &mut p.out), Event::None);
        assert_eq!(p.t.on_timer(deadline, &mut p.out), Event::TimedOut);
        assert_eq!((p.t.state(), p.out.len()), (State::Closed, 0));
    }

    #[test]
    fn a_new_ack_resets_the_backoff() {
        let mut p = established(8192, 536);
        p.t.send(b"abc", 0, &mut p.out);
        p.t.on_timer(300, &mut p.out);
        assert_eq!(p.drain().len(), 2, "original and one retransmission");
        p.rx(400, RCV, ISS + 4, ACK, b"");
        p.t.send(b"def", 500, &mut p.out);
        p.out.clear();
        p.t.on_timer(799, &mut p.out);
        assert!(p.out.is_empty());
        p.t.on_timer(800, &mut p.out);
        assert_eq!(p.drain()[0].2, b"def", "RTO back to 300 ms");
    }

    #[test]
    fn a_lost_syn_is_retransmitted() {
        let mut p = Peer::connect();
        let syn = p.drain();
        assert_eq!(p.t.on_timer(299, &mut p.out), Event::None);
        assert!(p.out.is_empty());
        p.t.on_timer(300, &mut p.out);
        assert_eq!(p.drain(), syn);
        assert_eq!(
            p.rx(310, RCV - 1, ISS + 1, SYN | ACK, b""),
            Event::Connected
        );
        p.out.clear();
        p.t.on_timer(10_000, &mut p.out);
        assert!(p.out.is_empty(), "acknowledged: the timer is disarmed");
    }

    #[test]
    fn a_rst_must_carry_exactly_rcv_nxt() {
        let mut p = established(8192, 1460);
        assert_eq!(p.rx(0, RCV + 10_000, 0, RST, b""), Event::None);
        assert!(p.out.is_empty(), "out of window: dropped silently");
        assert_eq!(p.rx(0, RCV + 100, 0, RST, b""), Event::None);
        assert_eq!(p.sent(), [(ACK, ISS + 1, RCV)], "in window: challenge ACK");
        assert_eq!(p.t.state(), State::Established);
        assert_eq!(p.rx(0, RCV, 0, RST, b""), Event::Reset);
        assert_eq!((p.t.state(), p.out.len()), (State::Closed, 0));
    }

    #[test]
    fn a_rst_in_syn_sent_must_acknowledge_our_syn() {
        let mut p = Peer::connect();
        p.out.clear();
        for (ack, flags) in [(0, RST), (ISS, RST | ACK)] {
            assert_eq!(p.rx(0, 0, ack, flags, b""), Event::None);
            assert_eq!(
                (p.t.state(), p.out.len()),
                (State::SynSent, 0),
                "never answered"
            );
        }
        assert_eq!(p.rx(0, 0, ISS + 1, RST | ACK, b""), Event::Reset);
        assert_eq!(p.t.state(), State::Closed);
    }

    #[test]
    fn a_syn_on_a_synchronized_connection_gets_a_challenge_ack() {
        let mut p = established(8192, 1460);
        assert_eq!(p.rx(0, RCV + 50, 0, SYN, b""), Event::None);
        assert_eq!(p.sent(), [(ACK, ISS + 1, RCV)]);
        assert_eq!((p.t.state(), p.t.rcv_nxt()), (State::Established, RCV));
    }

    #[test]
    fn out_of_order_and_duplicate_data_are_acked_not_delivered() {
        let mut p = established(8192, 1460);
        assert_eq!(p.rx(0, RCV + 5, ISS + 1, ACK, b"world"), Event::None);
        assert_eq!(p.sent(), [(ACK, ISS + 1, RCV)]);
        assert_eq!(p.t.recv_available(), 0, "no reassembly queue");
        assert_eq!(p.rx(0, RCV, ISS + 1, ACK, b"hello"), Event::DataReadable);
        assert_eq!(p.sent(), [(ACK, ISS + 1, RCV + 5)]);
        // The same segment again: ignored, but ACKed with rcv_nxt.
        assert_eq!(p.rx(0, RCV, ISS + 1, ACK, b"hello"), Event::None);
        assert_eq!(p.sent(), [(ACK, ISS + 1, RCV + 5)]);
        // An overlapping retransmission: only the new tail is taken.
        p.rx(0, RCV + 3, ISS + 1, ACK, b"lo!!");
        let mut got = [0u8; 16];
        let n = p.t.recv(&mut got);
        assert_eq!(&got[..n], b"hello!!");
        // An ACK for data never sent is answered and otherwise ignored.
        p.out.clear();
        assert_eq!(p.rx(0, RCV + 7, ISS + 99, ACK, b""), Event::None);
        assert_eq!(
            (p.sent(), p.t.snd_una()),
            (vec![(ACK, ISS + 1, RCV + 7)], ISS + 1)
        );
    }

    #[test]
    fn active_close_passes_time_wait_then_closes_after_two_msl() {
        let mut p = established(8192, 1460);
        p.t.send(b"bye", 0, &mut p.out);
        assert_eq!(p.t.close(0, &mut p.out), Event::None);
        assert_eq!(p.sent()[1], (FIN | ACK, ISS + 4, RCV), "FIN after the data");
        assert_eq!(p.t.state(), State::FinWait1);
        assert_eq!(
            p.t.send(b"late", 0, &mut p.out),
            0,
            "no sending after close"
        );
        p.rx(10, RCV, ISS + 5, ACK, b"");
        assert_eq!(p.t.state(), State::FinWait2);
        assert_eq!(p.rx(20, RCV, ISS + 5, FIN | ACK, b""), Event::PeerClosed);
        assert_eq!(
            (p.t.state(), p.sent()),
            (State::TimeWait, vec![(ACK, ISS + 5, RCV + 1)])
        );
        // A retransmitted FIN is re-ACKed and restarts the 2×MSL clock.
        p.rx(500, RCV, ISS + 5, FIN | ACK, b"");
        assert_eq!(p.sent(), [(ACK, ISS + 5, RCV + 1)]);
        assert_eq!(
            p.t.on_timer(500 + TIME_WAIT_MS - 1, &mut p.out),
            Event::None
        );
        assert_eq!(p.t.state(), State::TimeWait);
        assert_eq!(p.t.on_timer(500 + TIME_WAIT_MS, &mut p.out), Event::Closed);
        assert_eq!(p.t.state(), State::Closed);
    }

    #[test]
    fn passive_close_goes_through_close_wait_and_last_ack() {
        let mut p = established(8192, 1460);
        let ev = p.rx(0, RCV, ISS + 1, FIN | ACK, b"tail");
        assert_eq!(ev, Event::PeerClosed, "data and FIN in one segment");
        assert_eq!(
            (p.t.state(), p.sent()),
            (State::CloseWait, vec![(ACK, ISS + 1, RCV + 5)])
        );
        assert_eq!(
            p.t.send(b"reply", 0, &mut p.out),
            5,
            "CLOSE-WAIT may still send"
        );
        p.out.clear();
        p.t.close(0, &mut p.out);
        let fin = vec![(FIN | ACK, ISS + 6, RCV + 5)];
        assert_eq!((p.t.state(), p.sent()), (State::LastAck, fin));
        assert_eq!(p.rx(0, RCV + 5, ISS + 7, ACK, b""), Event::Closed);
        let mut got = [0u8; 8];
        let n = p.t.recv(&mut got);
        assert_eq!(
            (p.t.state(), &got[..n]),
            (State::Closed, &b"tail"[..]),
            "still readable"
        );
    }

    #[test]
    fn simultaneous_close_goes_through_closing() {
        let mut p = established(8192, 1460);
        p.t.close(0, &mut p.out);
        p.out.clear();
        // The peer's FIN crosses ours: it does not acknowledge our FIN.
        assert_eq!(p.rx(5, RCV, ISS + 1, FIN | ACK, b""), Event::PeerClosed);
        assert_eq!(
            (p.t.state(), p.sent()),
            (State::Closing, vec![(ACK, ISS + 2, RCV + 1)])
        );
        p.rx(10, RCV + 1, ISS + 2, ACK, b"");
        assert_eq!(p.t.state(), State::TimeWait);
        assert_eq!(p.t.on_timer(10 + TIME_WAIT_MS, &mut p.out), Event::Closed);
    }

    #[test]
    fn a_fin_queued_behind_data_still_goes_out_after_the_peer_closes() {
        // The window admits 1000 of 3000 bytes, so our FIN waits behind 2000.
        let mut p = established(1000, 1460);
        p.t.send(&[7u8; 3000], 0, &mut p.out);
        p.t.close(0, &mut p.out);
        assert_eq!((p.t.state(), p.sent().len()), (State::FinWait1, 1));
        // The peer's FIN arrives first, acknowledging 1000 and opening the
        // window: the rest of the data and then our FIN must still be sent.
        p.win = 4000;
        assert_eq!(p.rx(5, RCV, ISS + 1001, FIN | ACK, b""), Event::PeerClosed);
        assert_eq!(p.t.state(), State::Closing);
        let s = p.drain();
        let lens: Vec<usize> = s.iter().map(|x| x.2.len()).collect();
        assert_eq!(lens, [1460, 540, 0]);
        let fin = s[2].0;
        assert_eq!(
            (fin.flags, fin.seq, fin.ack),
            (FIN | ACK, ISS + 3001, RCV + 1)
        );
        p.rx(10, RCV + 1, ISS + 3002, ACK, b"");
        assert_eq!(p.t.state(), State::TimeWait);
    }

    #[test]
    fn abort_sends_rst_and_a_closed_port_answers_with_one() {
        let mut p = established(8192, 1460);
        p.t.abort(&mut p.out);
        assert_eq!(
            (p.sent(), p.t.state()),
            (vec![(RST, ISS + 1, 0)], State::Closed)
        );
        let (src_port, dst_port, seq, flags) = (BP, AP, 9, SYN);
        let h = SegmentHeader {
            src_port,
            dst_port,
            seq,
            flags,
            ..SegmentHeader::default()
        };
        let mut buf = [0u8; 64];
        let n = build_into(&mut buf, B, A, &h, None, b"").unwrap();
        assert!(reset_reply(&parse(&buf[..n], B, A).unwrap(), &mut p.out));
        assert_eq!(p.sent(), [(RST | ACK, 0, 10)], "covers the SYN");
    }

    /// Two Tcbs joined by a simulated wire. Every segment is encoded with
    /// `build_into` and decoded with `parse`, so the codec and the state
    /// machine are exercised together.
    struct Wire {
        a: Tcb,
        b: Tcb,
        a_to_b: VecDeque<Vec<u8>>,
        b_to_a: VecDeque<Vec<u8>>,
        now: u64,
        /// Index of an A→B data-bearing segment the wire silently loses.
        drop_a_data: Option<usize>,
        a_data_sent: usize,
    }

    impl Wire {
        fn new(iss_a: u32, iss_b: u32, drop_a_data: Option<usize>) -> Wire {
            let mut out = Outbox::new();
            let a = Tcb::connect(A, AP, B, BP, iss_a, 0, &mut out);
            let b = Tcb::listen(B, BP, iss_b);
            let (a_to_b, b_to_a) = (VecDeque::new(), VecDeque::new());
            let (now, a_data_sent) = (0, 0);
            let mut w = Wire {
                a,
                b,
                a_to_b,
                b_to_a,
                now,
                drop_a_data,
                a_data_sent,
            };
            w.ship(true, &mut out);
            w
        }

        fn ship(&mut self, from_a: bool, out: &mut Outbox) {
            for s in out.segments() {
                if from_a && !s.payload().is_empty() {
                    self.a_data_sent += 1;
                    if self.drop_a_data == Some(self.a_data_sent - 1) {
                        continue;
                    }
                }
                let mut buf = [0u8; 1600];
                let n = s.encode(&mut buf).unwrap();
                let q = if from_a {
                    &mut self.a_to_b
                } else {
                    &mut self.b_to_a
                };
                q.push_back(buf[..n].to_vec());
            }
            out.clear();
        }

        /// Deliver one queued segment each way, then let 10 ms pass.
        fn step(&mut self) {
            let mut out = Outbox::new();
            if let Some(frame) = self.a_to_b.pop_front() {
                self.b
                    .on_segment(&parse(&frame, A, B).unwrap(), self.now, &mut out);
                self.ship(false, &mut out);
            }
            if let Some(frame) = self.b_to_a.pop_front() {
                self.a
                    .on_segment(&parse(&frame, B, A).unwrap(), self.now, &mut out);
                self.ship(true, &mut out);
            }
            self.now += 10;
            self.a.on_timer(self.now, &mut out);
            self.ship(true, &mut out);
            self.b.on_timer(self.now, &mut out);
            self.ship(false, &mut out);
        }

        /// `send` (with `data`) or `close` (without) on one side.
        fn user(&mut self, from_a: bool, data: Option<&[u8]>) -> usize {
            let mut out = Outbox::new();
            let t = if from_a { &mut self.a } else { &mut self.b };
            let n = match data {
                Some(d) => t.send(d, self.now, &mut out),
                None => {
                    t.close(self.now, &mut out);
                    0
                }
            };
            self.ship(from_a, &mut out);
            n
        }

        fn run_until(&mut self, mut done: impl FnMut(&mut Wire) -> bool) {
            for _ in 0..50_000 {
                if done(self) {
                    return;
                }
                self.step();
            }
            panic!("never settled: {:?} / {:?}", self.a.state(), self.b.state());
        }

        fn established(&mut self) {
            self.run_until(|w| {
                w.a.state() == State::Established && w.b.state() == State::Established
            });
        }
    }

    fn pattern(len: u32, mul: u32) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(mul) >> 3) as u8).collect()
    }

    #[test]
    fn two_stacks_transfer_both_ways_across_the_sequence_wrap() {
        // Both ISSs sit just below 2^32, so every comparison crosses the wrap.
        let mut w = Wire::new(0xFFFF_FF00, 0xFFFF_FFF0, None);
        w.established();
        assert_eq!((w.a.peer_mss(), w.b.peer_mss()), (1460, 1460));
        let (to_b, to_a) = (pattern(10_000, 7), pattern(6_000, 13));
        let (mut sent_a, mut sent_b) = (0, 0);
        let (mut got_a, mut got_b) = (Vec::new(), Vec::new());
        w.run_until(|w| {
            sent_a += w.user(true, Some(&to_b[sent_a..]));
            sent_b += w.user(false, Some(&to_a[sent_b..]));
            let mut buf = [0u8; 700];
            let n = w.b.recv(&mut buf);
            got_b.extend_from_slice(&buf[..n]);
            let n = w.a.recv(&mut buf);
            got_a.extend_from_slice(&buf[..n]);
            got_b.len() == to_b.len() && got_a.len() == to_a.len()
        });
        assert!(got_b == to_b && got_a == to_a, "exact bytes, in order");
        assert!(w.a.snd_nxt() < 0x1_0000, "sequence space wrapped");

        // A closes first and so owns TIME-WAIT; B closes from CLOSE-WAIT.
        w.user(true, None);
        w.run_until(|w| w.b.state() == State::CloseWait);
        w.user(false, None);
        w.run_until(|w| w.a.state() == State::TimeWait && w.b.state() == State::Closed);
        let entered = w.now;
        w.run_until(|w| w.a.state() == State::Closed);
        assert!(w.now - entered >= TIME_WAIT_MS - 50, "held for 2 × MSL");
    }

    #[test]
    fn a_dropped_data_segment_is_recovered_by_retransmission() {
        let mut w = Wire::new(ISS, RCV - 1, Some(0));
        w.established();
        let msg = pattern(3000, 31);
        let start = w.now;
        assert_eq!(w.user(true, Some(&msg)), 3000);
        assert_eq!(w.a_data_sent, 3, "1460 + 1460 + 80, the first one lost");
        for _ in 0..20 {
            w.step(); // 200 ms, inside the first RTO
        }
        assert_eq!(w.b.recv_available(), 0, "nothing past the hole is kept");
        let mut got = Vec::new();
        w.run_until(|w| {
            let mut buf = [0u8; 4096];
            let n = w.b.recv(&mut buf);
            got.extend_from_slice(&buf[..n]);
            got.len() == msg.len()
        });
        assert!(got == msg, "exact bytes, in order");
        assert!(w.a_data_sent > 3, "recovered by retransmission");
        assert!(
            w.now - start >= INITIAL_RTO_MS,
            "only the RTO could fill it"
        );
        w.run_until(|w| w.a.snd_una() == w.a.snd_nxt());
    }
}
