//! The network interface: one NIC, one IPv4 address, and the dispatch that
//! turns received frames into protocol events.
//!
//! Everything that *parses* lives in `kernel_core::net` and is host-tested
//! against hostile input. What lives here is the part that cannot be tested
//! without hardware: driving the card, deciding what to answer, and holding
//! the small amount of state a stack needs (an ARP cache, a socket table).
//!
//! Three properties are deliberate:
//!
//! * **Nothing is unbounded.** The ARP cache, the socket table and each
//!   socket's receive queue are fixed-size arrays. A remote peer can cause
//!   entries to be *replaced*, never allocated, so no amount of traffic grows
//!   kernel memory.
//! * **Receive never blocks.** `poll` drains whatever the card has and
//!   returns; a caller waiting for a reply loops with a real deadline. A
//!   blocking receive in a polled stack is a hang waiting for a peer that may
//!   never answer.
//! * **The stack answers only what it must.** ARP requests for our own
//!   address and ICMP echo requests are answered; everything else is counted
//!   and dropped. Each drop has a reason, so "the network is broken" is always
//!   a diagnosable statement.

pub mod ipv6;
pub mod socket;
pub mod tcp;

use crate::device::e1000;
use crate::sync::Mutex;
use kernel_core::net::{arp, eth, icmp, ipv4, udp};

use eth::MacAddr;
use ipv4::Ipv4Addr;

/// Default address plan. It matches QEMU's user-mode networking so a guest
/// image behaves the same whether it is talking to slirp or to the test
/// harness's own wire peer.
pub const DEFAULT_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
pub const DEFAULT_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
pub const DEFAULT_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
pub const DEFAULT_DNS: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);

/// Largest frame this stack sends or accepts (standard Ethernet MTU + header).
pub const MAX_FRAME: usize = 1514;

const ARP_CACHE_LEN: usize = 8;

/// One learned IP→MAC binding.
#[derive(Clone, Copy)]
struct ArpEntry {
    ip: Ipv4Addr,
    mac: MacAddr,
    /// Tick at which this binding was learned; the oldest is evicted first, so
    /// a peer that floods us with gratuitous replies can at worst churn the
    /// cache, never grow it.
    learned: u64,
    valid: bool,
}

/// Counters that make a failure diagnosable instead of a silence.
#[derive(Debug, Clone, Copy, Default)]
pub struct NetStats {
    pub rx_frames: u64,
    pub rx_arp: u64,
    pub rx_ipv4: u64,
    pub rx_icmp: u64,
    pub rx_udp: u64,
    /// Frames refused by a parser (bad checksum, truncation, VLAN tag, ...).
    pub rx_malformed: u64,
    /// Well-formed frames this host is not interested in (not addressed to
    /// us, a protocol we do not implement, a port nobody bound).
    pub rx_unwanted: u64,
    pub tx_frames: u64,
    pub arp_requests_sent: u64,
    pub arp_replies_sent: u64,
    pub icmp_replies_sent: u64,
    /// Sends abandoned because ARP never resolved the next hop.
    pub tx_unresolved: u64,
}

struct Interface {
    ip: Ipv4Addr,
    netmask: Ipv4Addr,
    gateway: Ipv4Addr,
    dns: Ipv4Addr,
    arp: [ArpEntry; ARP_CACHE_LEN],
    stats: NetStats,
    /// Monotonically increasing IPv4 identification field.
    ip_id: u16,
    /// Set once the NIC is bound and the address plan applied.
    up: bool,
    /// The most recent ICMP echo reply seen, for the `ping` command: (id, seq).
    last_echo_reply: Option<(u16, u16)>,
    /// Target of the most recent ARP request and the TSC it was sent at.
    ///
    /// A caller retrying a non-blocking send in a tight yield loop would
    /// otherwise emit one ARP request per attempt — hundreds of broadcasts
    /// while waiting for the first reply. Rate limiting lives here rather than
    /// in the caller because every caller would need the same logic.
    arp_pending: Option<(Ipv4Addr, u64)>,
}

static IFACE: Mutex<Interface> = Mutex::new(Interface {
    ip: DEFAULT_IP,
    netmask: DEFAULT_NETMASK,
    gateway: DEFAULT_GATEWAY,
    dns: DEFAULT_DNS,
    arp: [ArpEntry {
        ip: Ipv4Addr::UNSPECIFIED,
        mac: MacAddr::ZERO,
        learned: 0,
        valid: false,
    }; ARP_CACHE_LEN],
    stats: NetStats {
        rx_frames: 0,
        rx_arp: 0,
        rx_ipv4: 0,
        rx_icmp: 0,
        rx_udp: 0,
        rx_malformed: 0,
        rx_unwanted: 0,
        tx_frames: 0,
        arp_requests_sent: 0,
        arp_replies_sent: 0,
        icmp_replies_sent: 0,
        tx_unresolved: 0,
    },
    ip_id: 1,
    up: false,
    last_echo_reply: None,
    arp_pending: None,
});

/// Bring the interface up on the bound NIC. Idempotent.
pub fn init() {
    if !e1000::present() {
        return;
    }
    let mut iface = IFACE.lock();
    if iface.up {
        return;
    }
    iface.up = true;
    let ip = iface.ip;
    let gw = iface.gateway;
    drop(iface);
    let mut a = [0u8; 15];
    let mut b = [0u8; 15];
    crate::serial_println!(
        "[ITISYOU:NET] iface_up ip={} gateway={} mtu={}",
        ip.format(&mut a),
        gw.format(&mut b),
        MAX_FRAME - eth::HEADER_LEN,
    );
}

pub fn is_up() -> bool {
    IFACE.lock().up
}

pub fn stats() -> NetStats {
    IFACE.lock().stats
}

pub fn address() -> (Ipv4Addr, Ipv4Addr, Ipv4Addr, Ipv4Addr) {
    let i = IFACE.lock();
    (i.ip, i.netmask, i.gateway, i.dns)
}

pub fn mac() -> MacAddr {
    e1000::with(|n| n.mac()).unwrap_or(MacAddr::ZERO)
}

// --- ARP cache -------------------------------------------------------------

impl Interface {
    fn arp_lookup(&self, ip: Ipv4Addr) -> Option<MacAddr> {
        self.arp
            .iter()
            .find(|e| e.valid && e.ip == ip)
            .map(|e| e.mac)
    }

    /// Insert or refresh a binding, evicting the oldest entry when full.
    fn arp_learn(&mut self, ip: Ipv4Addr, mac: MacAddr, now: u64) {
        if let Some(e) = self.arp.iter_mut().find(|e| e.valid && e.ip == ip) {
            e.mac = mac;
            e.learned = now;
            return;
        }
        let slot = match self.arp.iter().position(|e| !e.valid) {
            Some(i) => i,
            None => {
                // Oldest wins eviction: a flood of new peers rotates the cache
                // rather than being able to pin a chosen entry in it.
                let mut oldest = 0;
                for (i, e) in self.arp.iter().enumerate() {
                    if e.learned < self.arp[oldest].learned {
                        oldest = i;
                    }
                }
                oldest
            }
        };
        self.arp[slot] = ArpEntry {
            ip,
            mac,
            learned: now,
            valid: true,
        };
    }

    /// The address ARP must resolve to reach `dst`: the destination itself on
    /// our own subnet, otherwise the gateway.
    fn next_hop(&self, dst: Ipv4Addr) -> Ipv4Addr {
        if dst.same_subnet(self.ip, self.netmask) || dst.is_broadcast() {
            dst
        } else {
            self.gateway
        }
    }
}

// --- transmit --------------------------------------------------------------

fn tx_frame(dst: MacAddr, ethertype: u16, payload: &[u8]) -> bool {
    let mut buf = [0u8; MAX_FRAME];
    let src = mac();
    let Ok(n) = eth::build_into(
        &mut buf,
        dst,
        src,
        eth::EtherType::from_u16(ethertype),
        payload,
        true,
    ) else {
        return false;
    };
    let sent = e1000::with(|nic| nic.transmit(&buf[..n])).unwrap_or(false);
    if sent {
        IFACE.lock().stats.tx_frames += 1;
    }
    sent
}

/// Send an ARP request for `target` at most once per [`ARP_RETRY_MS`].
fn request_arp_rate_limited(target: Ipv4Addr) {
    let now = crate::interrupts::tsc();
    let budget = crate::interrupts::cycles_for_ms(ARP_RETRY_MS);
    {
        let mut iface = IFACE.lock();
        match iface.arp_pending {
            Some((pending, at)) if pending == target && now.wrapping_sub(at) < budget => return,
            _ => iface.arp_pending = Some((target, now)),
        }
    }
    send_arp_request(target);
}

/// How long to wait before repeating an unanswered ARP request.
const ARP_RETRY_MS: u64 = 200;

fn send_arp_request(target: Ipv4Addr) {
    let (src_ip, src_mac) = { (IFACE.lock().ip, mac()) };
    let mut payload = [0u8; arp::PACKET_LEN];
    let packet = arp::request(src_mac, src_ip, target);
    if arp::build_into(&mut payload, &packet).is_err() {
        return;
    }
    if tx_frame(MacAddr::BROADCAST, eth::EtherType::ARP, &payload) {
        IFACE.lock().stats.arp_requests_sent += 1;
    }
}

/// Resolve `ip` to a MAC, sending requests and polling until `timeout_ms`.
pub fn resolve(ip: Ipv4Addr, timeout_ms: u64) -> Option<MacAddr> {
    let target = {
        let iface = IFACE.lock();
        let hop = iface.next_hop(ip);
        if let Some(mac) = iface.arp_lookup(hop) {
            return Some(mac);
        }
        hop
    };
    // Broadcast needs no resolution.
    if target.is_broadcast() {
        return Some(MacAddr::BROADCAST);
    }
    let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
    let mut retry = crate::interrupts::Deadline::after_ms(0);
    loop {
        if !retry.pending() {
            send_arp_request(target);
            retry = crate::interrupts::Deadline::after_ms(200);
        }
        poll();
        if let Some(mac) = IFACE.lock().arp_lookup(target) {
            return Some(mac);
        }
        if !deadline.pending() {
            IFACE.lock().stats.tx_unresolved += 1;
            return None;
        }
    }
}

/// Why a non-blocking send could not be completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The next hop's MAC is not known yet. An ARP request has been sent; the
    /// caller should poll and retry rather than wait here.
    Unresolved,
    /// The packet could not be built (payload too large) or the ring is full.
    Failed,
}

/// Send one IPv4 packet WITHOUT waiting for ARP.
///
/// This is the form a syscall must use. Inside a syscall, `SFMASK` has cleared
/// IF, so no interrupt can arrive: a blocking wait there freezes the whole
/// machine for its duration and cannot be preempted. Returning
/// [`SendError::Unresolved`] hands the waiting back to the caller's own
/// scheduling slice, where yielding is free and the rest of the system keeps
/// running.
pub fn try_send_ipv4(dst: Ipv4Addr, protocol: u8, payload: &[u8]) -> Result<(), SendError> {
    let hop = {
        let iface = IFACE.lock();
        iface.next_hop(dst)
    };
    // The lookup result is bound to a local BEFORE it is matched. Writing
    // `match IFACE.lock().arp_lookup(hop)` would keep the guard alive for the
    // whole match, and the `None` arm re-locks to send the request — a
    // self-deadlock on a spin lock, with interrupts off, i.e. a dead machine.
    let cached = IFACE.lock().arp_lookup(hop);
    let dst_mac = if hop.is_broadcast() {
        MacAddr::BROADCAST
    } else {
        match cached {
            Some(mac) => mac,
            None => {
                request_arp_rate_limited(hop);
                return Err(SendError::Unresolved);
            }
        }
    };
    if build_and_send_ipv4(dst, dst_mac, protocol, payload) {
        Ok(())
    } else {
        Err(SendError::Failed)
    }
}

/// Send one IPv4 packet, resolving the next hop first. Blocking; only for
/// kernel-side callers that run with interrupts enabled (the shell).
pub fn send_ipv4(dst: Ipv4Addr, protocol: u8, payload: &[u8]) -> bool {
    let Some(dst_mac) = resolve(dst, 1000) else {
        return false;
    };
    build_and_send_ipv4(dst, dst_mac, protocol, payload)
}

fn build_and_send_ipv4(dst: Ipv4Addr, dst_mac: MacAddr, protocol: u8, payload: &[u8]) -> bool {
    let (src, id) = {
        let mut iface = IFACE.lock();
        iface.ip_id = iface.ip_id.wrapping_add(1);
        (iface.ip, iface.ip_id)
    };
    let mut builder = ipv4::Ipv4Builder::new(src, dst, protocol);
    builder.identification = id;
    let mut packet = [0u8; MAX_FRAME - eth::HEADER_LEN];
    let Ok(n) = ipv4::build_into(&mut packet, &builder, payload) else {
        return false;
    };
    tx_frame(dst_mac, eth::EtherType::IPV4, &packet[..n])
}

/// Build a UDP datagram into `out`, returning its length.
fn build_udp<'a>(
    out: &'a mut [u8],
    dst: Ipv4Addr,
    dst_port: u16,
    src_port: u16,
    payload: &[u8],
) -> Option<&'a [u8]> {
    let src = IFACE.lock().ip;
    let n = udp::build_into(out, src, dst, src_port, dst_port, payload).ok()?;
    Some(&out[..n])
}

/// Send a UDP datagram, blocking on ARP if necessary (shell/kernel callers).
pub fn send_udp(dst: Ipv4Addr, dst_port: u16, src_port: u16, payload: &[u8]) -> bool {
    let mut datagram = [0u8; udp::MAX_PAYLOAD_LEN + udp::HEADER_LEN];
    let Some(bytes) = build_udp(&mut datagram, dst, dst_port, src_port, payload) else {
        return false;
    };
    // Copy out of the borrow so the interface lock is not held across the send.
    let mut owned = [0u8; udp::MAX_PAYLOAD_LEN + udp::HEADER_LEN];
    let n = bytes.len();
    owned[..n].copy_from_slice(bytes);
    send_ipv4(dst, ipv4::proto::UDP, &owned[..n])
}

/// Non-blocking UDP send, for syscall context.
pub fn try_send_udp(
    dst: Ipv4Addr,
    dst_port: u16,
    src_port: u16,
    payload: &[u8],
) -> Result<(), SendError> {
    let mut datagram = [0u8; udp::MAX_PAYLOAD_LEN + udp::HEADER_LEN];
    let n = {
        let Some(bytes) = build_udp(&mut datagram, dst, dst_port, src_port, payload) else {
            return Err(SendError::Failed);
        };
        bytes.len()
    };
    try_send_ipv4(dst, ipv4::proto::UDP, &datagram[..n])
}

/// Send an ICMP echo request. The reply is observed through [`poll`].
pub fn send_ping(dst: Ipv4Addr, id: u16, seq: u16, payload: &[u8]) -> bool {
    IFACE.lock().last_echo_reply = None;
    let mut msg = [0u8; 128];
    let Ok(n) = icmp::build_echo_into(&mut msg, icmp::EchoKind::Request, id, seq, payload) else {
        return false;
    };
    send_ipv4(dst, ipv4::proto::ICMP, &msg[..n])
}

/// Wait for the echo reply matching `(id, seq)`.
pub fn await_ping_reply(id: u16, seq: u16, timeout_ms: u64) -> bool {
    let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
    loop {
        poll();
        if IFACE.lock().last_echo_reply == Some((id, seq)) {
            return true;
        }
        if !deadline.pending() {
            return false;
        }
    }
}

// --- receive ---------------------------------------------------------------

/// Drain every frame the card has, dispatching each. Returns how many were
/// processed. Never blocks.
///
/// Every poll is also a scheduling safe point (V0.10, rule R8): ping,
/// resolve, dhcp, ipv6, `net poll`, `tcp serve` and the audit anchor all
/// wait by polling, so the background keeps running while they wait. No
/// caller may hold a lock across `poll` (a held lock makes the safe point
/// refuse and is counted as `lock_skips`). Slices never poll the NIC
/// themselves; inside a syscall the safe point refuses (IF is clear).
pub fn poll() -> usize {
    crate::sched::safe_point();
    poll_inner()
}

/// Frames one `poll` drains before it returns, even if the card has more.
/// V1.0 (NET1-004, the V1-SEC-002 review): the drain used to run until the
/// ring was empty, so a sustained frame flood from a remote peer kept the
/// single CPU inside one syscall — interrupts masked, no scheduling, no TCP
/// timers — for as long as it lasted. A bound turns a flood into many short
/// polls (the caller polls again from its own slice) instead of one that
/// never returns. Two ring-fulls per poll keeps normal bursts single-pass.
const DRAIN_MAX: usize = 2 * e1000::RING_LEN;

fn poll_inner() -> usize {
    let mut processed = 0;
    let mut buf = [0u8; MAX_FRAME];
    // Clear whatever the card has raised. The MSI handler counts messages and
    // touches no device state, so this is where the cause register is retired
    // — without it the card would raise one message and then go quiet.
    e1000::with(|nic| nic.clear_interrupt_cause());
    while processed < DRAIN_MAX {
        let Some(Some(n)) = e1000::with(|nic| nic.receive(&mut buf)) else {
            break;
        };
        processed += 1;
        dispatch(&buf[..n]);
    }
    // Every drain also runs the TCP timers: this polled stack has no
    // background timer, so whoever polls is what retransmits.
    tcp::tick();
    processed
}

fn dispatch(frame: &[u8]) {
    IFACE.lock().stats.rx_frames += 1;
    // The card already filtered to our address and broadcast, so a runt here
    // is a damaged frame rather than something addressed elsewhere.
    let Ok(parsed) = eth::parse(frame, false) else {
        IFACE.lock().stats.rx_malformed += 1;
        return;
    };
    match parsed.ethertype {
        eth::EtherType::Arp => on_arp(parsed.payload),
        eth::EtherType::Ipv4 => on_ipv4(parsed.payload),
        eth::EtherType::Ipv6 => ipv6::on_frame(parsed.payload),
        _ => IFACE.lock().stats.rx_unwanted += 1,
    }
}

fn on_arp(payload: &[u8]) {
    let now = crate::interrupts::ticks();
    let Ok(packet) = arp::parse(payload) else {
        IFACE.lock().stats.rx_malformed += 1;
        return;
    };
    let (our_ip, reply) = {
        let mut iface = IFACE.lock();
        iface.stats.rx_arp += 1;
        // Learn from any ARP traffic that names a sender: the sender fields of
        // a request are as good a binding as those of a reply, and learning
        // from requests is what keeps a ping exchange from needing two round
        // trips.
        if !packet.sender_ip.is_unspecified() && packet.sender_mac.is_unicast() {
            iface.arp_learn(packet.sender_ip, packet.sender_mac, now);
        }
        let our_ip = iface.ip;
        (our_ip, packet.is_request_for(our_ip))
    };
    if !reply {
        return;
    }
    let mut out = [0u8; arp::PACKET_LEN];
    let response = packet.reply_with(our_ip, mac());
    if arp::build_into(&mut out, &response).is_ok()
        && tx_frame(packet.sender_mac, eth::EtherType::ARP, &out)
    {
        IFACE.lock().stats.arp_replies_sent += 1;
    }
}

fn on_ipv4(payload: &[u8]) {
    let Ok(packet) = ipv4::parse(payload) else {
        IFACE.lock().stats.rx_malformed += 1;
        return;
    };
    let our_ip = {
        let mut iface = IFACE.lock();
        iface.stats.rx_ipv4 += 1;
        iface.ip
    };
    // Accept only traffic addressed to this host or broadcast. The card's
    // filter already does most of this; re-checking at the IP layer means a
    // promiscuous card (or a future bridged setup) cannot smuggle a packet
    // meant for someone else into a socket.
    if packet.header.dst != our_ip && !packet.header.dst.is_broadcast() {
        IFACE.lock().stats.rx_unwanted += 1;
        return;
    }
    // A source that is broadcast or multicast is spoofed: no host sends from
    // one, and a reply to it (a TCP RST or SYN-ACK, an ICMP echo reply) would
    // go to the whole link as a reflection amplifier (V1.0, NET1-002).
    if packet.header.src.is_broadcast() || packet.header.src.is_multicast() {
        IFACE.lock().stats.rx_unwanted += 1;
        return;
    }
    // An echo request must be addressed to this host, not broadcast: a ping
    // to the link broadcast that we answered would make the guest a reflector
    // with a spoofable source (V1.0, NET1-002, the V1-SEC-002 review). UDP
    // still accepts broadcast — DHCP's reply is a broadcast datagram.
    let to_us = packet.header.dst == our_ip;
    match packet.header.protocol {
        ipv4::proto::ICMP if to_us => on_icmp(packet.header.src, packet.payload),
        ipv4::proto::ICMP => IFACE.lock().stats.rx_unwanted += 1,
        // The UDP checksum covers the destination address actually in the
        // header. Passing our own address here (as V0.8 did) made every
        // BROADCAST datagram fail its checksum and be dropped as malformed —
        // invisible until V0.9's DHCP client depended on a broadcast reply.
        ipv4::proto::UDP => on_udp(packet.header.src, packet.header.dst, packet.payload),
        // TCP is unicast only: a segment to the broadcast address is noise,
        // and answering it with a RST would be a reflection amplifier.
        ipv4::proto::TCP if packet.header.dst == our_ip => {
            tcp::on_segment(packet.header.src, packet.header.dst, packet.payload)
        }
        _ => IFACE.lock().stats.rx_unwanted += 1,
    }
}

fn on_icmp(src: Ipv4Addr, payload: &[u8]) {
    let Ok(echo) = icmp::parse_echo(payload) else {
        IFACE.lock().stats.rx_malformed += 1;
        return;
    };
    IFACE.lock().stats.rx_icmp += 1;
    match echo.kind {
        icmp::EchoKind::Reply => {
            IFACE.lock().last_echo_reply = Some((echo.identifier, echo.sequence));
        }
        icmp::EchoKind::Request => {
            // Echo the payload back verbatim, as RFC 792 requires; the
            // requester uses it to check the path did not mangle data.
            let mut msg = [0u8; MAX_FRAME];
            let Ok(n) = icmp::build_echo_into(
                &mut msg,
                icmp::EchoKind::Reply,
                echo.identifier,
                echo.sequence,
                echo.payload,
            ) else {
                return;
            };
            // Non-blocking (V1.0, NET1-001, the V1-SEC-002 review): this runs
            // inside `poll`, and the blocking `send_ipv4` would resolve ARP by
            // calling `poll` again — one stack frame deeper per unanswered
            // ping, which a flood of pings from an unresolved on-subnet source
            // drove into the guard page and a kernel double fault. The next
            // hop is answered from the cache, as the IPv6 responder does; a
            // peer that pinged us has just ARP-resolved us, so it is usually
            // warm. If it is not, the reply is dropped rather than waited for.
            if try_send_ipv4(src, ipv4::proto::ICMP, &msg[..n]).is_ok() {
                IFACE.lock().stats.icmp_replies_sent += 1;
            }
        }
    }
}

fn on_udp(src: Ipv4Addr, dst: Ipv4Addr, payload: &[u8]) {
    let Ok(datagram) = udp::parse(payload, src, dst) else {
        IFACE.lock().stats.rx_malformed += 1;
        return;
    };
    IFACE.lock().stats.rx_udp += 1;
    if !socket::deliver(datagram.dst_port, src, datagram.src_port, datagram.payload) {
        IFACE.lock().stats.rx_unwanted += 1;
    }
}

/// Resolve `name` to an IPv4 address using the configured DNS server.
///
/// Returns `None` on timeout or on any malformed or mismatched response — a
/// resolver that falls back to "some address" on a bad answer is worse than
/// one that fails.
pub fn resolve_name(name: &str, timeout_ms: u64) -> Option<Ipv4Addr> {
    use kernel_core::net::dns;
    let (server, local_port) = {
        let iface = IFACE.lock();
        // The query id doubles as the ephemeral source port's low bits; both
        // vary per lookup so a blind off-path spoof has to guess both.
        (
            iface.dns,
            40000u16 + (crate::interrupts::ticks() as u16 & 0x0FFF),
        )
    };
    let id = (crate::interrupts::ticks() as u16) ^ 0x5A5A;
    let mut query = [0u8; dns::MAX_QUERY_LEN];
    let n = dns::build_query(&mut query, id, name).ok()?;

    let sock = socket::bind_kernel(local_port)?;
    let result = (|| {
        if !send_udp(server, dns::PORT, local_port, &query[..n]) {
            return None;
        }
        let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
        let mut buf = [0u8; 512];
        loop {
            poll();
            if let Some((len, from, _port)) = socket::take(sock, &mut buf) {
                // Only the server we asked may answer.
                if from == server {
                    if let Ok(answer) = dns::parse_response(&buf[..len], id, name) {
                        return Some(answer.address);
                    }
                }
            }
            if !deadline.pending() {
                return None;
            }
        }
    })();
    socket::close_kernel(sock);
    result
}

// --- DHCP (V0.9) -------------------------------------------------------------

/// Why DHCP configuration did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpFailure {
    NoNic,
    /// Port 68 is already bound (another configuration attempt, or an app).
    PortBusy,
    NoOffer,
    NoAck,
    /// The server declined the request (NAK).
    Declined,
}

/// Broadcast a UDP datagram from the unconfigured address `0.0.0.0` — the
/// only source a DHCP client may use before it holds a lease (RFC 2131 §4.1).
fn send_unconfigured_broadcast(src_port: u16, dst_port: u16, payload: &[u8]) -> bool {
    let mut datagram = [0u8; udp::MAX_PAYLOAD_LEN + udp::HEADER_LEN];
    let Ok(n) = udp::build_into(
        &mut datagram,
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::BROADCAST,
        src_port,
        dst_port,
        payload,
    ) else {
        return false;
    };
    let id = {
        let mut iface = IFACE.lock();
        iface.ip_id = iface.ip_id.wrapping_add(1);
        iface.ip_id
    };
    let mut builder =
        ipv4::Ipv4Builder::new(Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, ipv4::proto::UDP);
    builder.identification = id;
    let mut packet = [0u8; MAX_FRAME - eth::HEADER_LEN];
    let Ok(len) = ipv4::build_into(&mut packet, &builder, &datagram[..n]) else {
        return false;
    };
    tx_frame(MacAddr::BROADCAST, eth::EtherType::IPV4, &packet[..len])
}

/// Wait up to `ms` for a reply to transaction `xid` on socket `sock`.
fn await_dhcp(sock: usize, xid: u32, ms: u64) -> Option<kernel_core::net::dhcp::Lease> {
    use kernel_core::net::dhcp;
    let mut deadline = crate::interrupts::Deadline::after_ms(ms);
    let mut buf = [0u8; 600];
    loop {
        poll();
        while let Some((len, _from, port)) = socket::take(sock, &mut buf) {
            // Only a server may answer, and only this transaction for this
            // card; everything else is someone else's conversation.
            if port != dhcp::SERVER_PORT {
                continue;
            }
            match dhcp::parse_reply(&buf[..len], xid, mac()) {
                Ok(lease) => return Some(lease),
                Err(e) => crate::serial_println!("[ITISYOU:NET] dhcp_reply refused={e:?}"),
            }
        }
        if !deadline.pending() {
            return None;
        }
    }
}

/// Obtain and apply a lease: DISCOVER → OFFER → REQUEST → ACK (RFC 2131).
///
/// Blocking with real deadlines, so kernel context only (the shell), never a
/// syscall. Each message is retransmitted once per second until the overall
/// budget runs out. On failure the interface keeps the address plan it had,
/// so a network without a DHCP server still gets the static configuration.
pub fn dhcp_configure(timeout_ms: u64) -> Result<kernel_core::net::dhcp::Lease, DhcpFailure> {
    use kernel_core::net::dhcp;
    if !e1000::present() {
        return Err(DhcpFailure::NoNic);
    }
    let sock = socket::bind_kernel(dhcp::CLIENT_PORT).ok_or(DhcpFailure::PortBusy)?;
    let xid = (crate::interrupts::tsc() as u32) ^ 0x4954_5953; // "ITYS"
    let result = (|| {
        let mut msg = [0u8; dhcp::MAX_MESSAGE_LEN];
        let mut budget = timeout_ms;
        // DISCOVER until an OFFER arrives.
        let offer = loop {
            let n = dhcp::build_discover(&mut msg, xid, mac()).map_err(|_| DhcpFailure::NoOffer)?;
            send_unconfigured_broadcast(dhcp::CLIENT_PORT, dhcp::SERVER_PORT, &msg[..n]);
            let wait = budget.min(1000);
            match await_dhcp(sock, xid, wait) {
                Some(l) if l.kind == dhcp::MessageType::Offer => break l,
                _ if budget <= wait => return Err(DhcpFailure::NoOffer),
                _ => budget -= wait,
            }
        };
        let mut a = [0u8; 15];
        let mut b = [0u8; 15];
        crate::serial_println!(
            "[ITISYOU:NET] dhcp_offer ip={} server={}",
            offer.address.format(&mut a),
            offer.server.format(&mut b)
        );
        // REQUEST the offered address until the server answers.
        let mut budget = timeout_ms;
        let ack = loop {
            let n = dhcp::build_request(&mut msg, xid, mac(), &offer)
                .map_err(|_| DhcpFailure::NoAck)?;
            send_unconfigured_broadcast(dhcp::CLIENT_PORT, dhcp::SERVER_PORT, &msg[..n]);
            let wait = budget.min(1000);
            match await_dhcp(sock, xid, wait) {
                Some(l) if l.kind == dhcp::MessageType::Ack => break l,
                Some(l) if l.kind == dhcp::MessageType::Nak => return Err(DhcpFailure::Declined),
                _ if budget <= wait => return Err(DhcpFailure::NoAck),
                _ => budget -= wait,
            }
        };
        Ok(ack)
    })();
    socket::close_kernel(sock);
    let lease = result?;

    // Apply. A lease without a mask falls back to the /24 the static plan
    // used; without a router the server itself is the only known next hop.
    let netmask = lease.netmask.unwrap_or(DEFAULT_NETMASK);
    let gateway = lease.router.unwrap_or(lease.server);
    let dns = lease.dns.unwrap_or(gateway);
    {
        let mut iface = IFACE.lock();
        iface.ip = lease.address;
        iface.netmask = netmask;
        iface.gateway = gateway;
        iface.dns = dns;
        // Bindings learned under the old address plan may point at hosts that
        // are no longer on-link.
        for e in iface.arp.iter_mut() {
            e.valid = false;
        }
    }
    let (mut a, mut m, mut g, mut d, mut s) =
        ([0u8; 15], [0u8; 15], [0u8; 15], [0u8; 15], [0u8; 15]);
    crate::serial_println!(
        "[ITISYOU:NET] dhcp_lease ip={} mask={} router={} dns={} server={} lease_secs={} renew_secs={}",
        lease.address.format(&mut a),
        netmask.format(&mut m),
        gateway.format(&mut g),
        dns.format(&mut d),
        lease.server.format(&mut s),
        lease.lease_secs.unwrap_or(0),
        lease.renew_after().unwrap_or(0),
    );
    crate::audit::allowed("dhcp_lease", 0, None);
    Ok(lease)
}

/// One UDP request/response exchange from kernel context (the shell): bind an
/// ephemeral kernel socket, send `payload` to `dst:dst_port`, and wait up to
/// `timeout_ms` for a reply from exactly that endpoint. Returns the reply's
/// length in `reply`. Retransmits every 500 ms, since UDP carries no promise.
pub fn udp_request(
    dst: Ipv4Addr,
    dst_port: u16,
    payload: &[u8],
    reply: &mut [u8],
    timeout_ms: u64,
) -> Option<usize> {
    let local_port = 41000u16 + (crate::interrupts::ticks() as u16 & 0x0FFF);
    let sock = socket::bind_kernel(local_port)?;
    let result = (|| {
        let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
        let mut resend = crate::interrupts::Deadline::after_ms(0);
        loop {
            if !resend.pending() {
                send_udp(dst, dst_port, local_port, payload);
                resend = crate::interrupts::Deadline::after_ms(500);
            }
            poll();
            if let Some((len, from, port)) = socket::take(sock, reply) {
                if from == dst && port == dst_port {
                    return Some(len);
                }
            }
            if !deadline.pending() {
                return None;
            }
        }
    })();
    socket::close_kernel(sock);
    result
}
