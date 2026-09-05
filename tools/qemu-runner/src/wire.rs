//! A host-side Ethernet peer for the guest's NIC.
//!
//! QEMU's `dgram` netdev carries one raw Ethernet frame per UDP datagram, so
//! the runner can be the *entire* network the guest sees: it answers ARP, ICMP
//! echo, a UDP echo service and DNS, and records exactly what it observed.
//!
//! Two properties make this worth more than pointing the guest at QEMU's
//! built-in user-mode networking:
//!
//! * **It is deterministic and offline.** No DHCP lease, no host resolver, no
//!   outbound traffic — the same packets on a laptop and in CI, with nothing
//!   to be flaky about.
//! * **It is an independent implementation.** These bytes are parsed and built
//!   here by hand rather than with `kernel_core::net`, so a bug in the guest's
//!   codec cannot cancel itself out against the same code on the other side.
//!   A test where both ends share a checksum routine proves the two agree, not
//!   that either is right.
//!
//! The peer answers as 10.0.2.2 (gateway/echo) and 10.0.2.3 (DNS), matching
//! QEMU's user-mode address plan so the guest needs no special configuration.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The peer's own MAC. Locally administered, and visibly not the guest's.
pub const PEER_MAC: [u8; 6] = [0x52, 0x55, 0x0A, 0x00, 0x02, 0x02];
/// Gateway/echo address.
pub const PEER_IP: [u8; 4] = [10, 0, 2, 2];
/// DNS server address.
pub const DNS_IP: [u8; 4] = [10, 0, 2, 3];
/// The single name the peer is authoritative for, and its answer.
pub const DNS_NAME: &str = "os.itisyou.app";
pub const DNS_ANSWER: [u8; 4] = [93, 184, 216, 34];
/// Echo service port.
pub const ECHO_PORT: u16 = 7;

const ETHERTYPE_ARP: u16 = 0x0806;
const ETHERTYPE_IPV4: u16 = 0x0800;
const PROTO_ICMP: u8 = 1;
const PROTO_UDP: u8 = 17;

/// What the peer saw, in wire terms. Every field is something the guest had to
/// actually put on the wire correctly for the count to move.
#[derive(Debug, Clone, Default)]
pub struct Observations {
    pub frames_in: u64,
    pub frames_out: u64,
    pub arp_requests: u64,
    pub arp_replies_sent: u64,
    /// ARP requests whose sender fields named the guest's expected address.
    pub arp_from_guest: u64,
    pub icmp_echo_requests: u64,
    pub icmp_echo_replies_sent: u64,
    pub udp_datagrams: u64,
    pub udp_echoed: u64,
    pub dns_queries: u64,
    pub dns_answers_sent: u64,
    /// Frames the peer refused: bad checksum, truncation, unknown ethertype.
    pub rejected: u64,
    /// UDP payloads received on the echo port, in order.
    pub echo_payloads: Vec<Vec<u8>>,
    /// Source MAC of the first frame seen, i.e. the guest's station address.
    pub guest_mac: Option<[u8; 6]>,
    /// Source IPv4 of the first IP packet seen.
    pub guest_ip: Option<[u8; 4]>,
    /// IP checksums that did not verify — must stay zero.
    pub bad_ip_checksums: u64,
    /// UDP checksums that did not verify — must stay zero.
    pub bad_udp_checksums: u64,
    /// ICMP checksums that did not verify — must stay zero.
    pub bad_icmp_checksums: u64,
    /// The peer has probed the guest (ARP request, then ping).
    pub probed_guest: bool,
    /// ARP replies the GUEST sent in answer to the peer's request. Proves the
    /// guest's ARP responder, which its own traffic never exercises.
    pub guest_arp_replies: u64,
    /// ICMP echo replies the GUEST sent. Proves the inbound ICMP path: the
    /// guest answering a ping rather than only sending one.
    pub guest_icmp_replies: u64,
    /// The hostile-frame sequence has been sent.
    pub hostile_sent: bool,
    /// Responses that could only have been produced BY the hostile sequence,
    /// identified by content rather than by timing: an echo reply carrying one
    /// of the probes' identifiers, any ICMP error type, or a datagram back to
    /// the black-holed probe's source port. Counting "everything after the
    /// probe" would have flagged the guest's own unrelated traffic, which is
    /// not evidence of anything.
    pub replies_to_hostile: u64,
}

pub struct WirePeer {
    /// UDP port QEMU must send its frames to (the peer's own socket).
    pub host_port: u16,
    /// UDP port QEMU binds and the peer sends frames to.
    pub guest_port: u16,
    observations: Arc<Mutex<Observations>>,
    stop: Arc<AtomicBool>,
}

impl WirePeer {
    /// Bind both endpoints and start serving. `log` receives one line per
    /// notable event; the runner folds them into the serial log so the
    /// evidence lives in one artifact.
    pub fn start(log: Sender<String>) -> std::io::Result<WirePeer> {
        // The peer's socket. QEMU sends frames here.
        let sock = UdpSocket::bind("127.0.0.1:0")?;
        let host_port = sock.local_addr()?.port();
        // Reserve the guest-side port by binding and immediately dropping it:
        // QEMU binds it a moment later. Binding first means the kernel has
        // told us the port was free rather than us guessing one.
        let guest_port = {
            let probe = UdpSocket::bind("127.0.0.1:0")?;
            let p = probe.local_addr()?.port();
            drop(probe);
            p
        };
        sock.set_read_timeout(Some(Duration::from_millis(100)))?;

        let observations = Arc::new(Mutex::new(Observations::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let obs = Arc::clone(&observations);
        let stop_flag = Arc::clone(&stop);
        let peer_addr = format!("127.0.0.1:{guest_port}");

        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let mut arp_cache: HashMap<[u8; 4], [u8; 6]> = HashMap::new();
            while !stop_flag.load(Ordering::Relaxed) {
                let n = match sock.recv(&mut buf) {
                    Ok(n) => n,
                    // A timeout is the normal idle case; anything else means
                    // the socket is gone and the peer should stop.
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue
                    }
                    Err(_) => break,
                };
                let frame = &buf[..n];
                let mut out: Vec<Vec<u8>> = Vec::new();
                {
                    let mut o = obs.lock().unwrap();
                    o.frames_in += 1;
                    handle_frame(frame, &mut o, &mut arp_cache, &mut out, &log);
                    // Once the guest's address is known, probe it: ARP first,
                    // then (on its reply) a ping. The guest's own traffic only
                    // exercises its client paths; this is what proves it
                    // ANSWERS — that it is a host on the network, not just a
                    // sender.
                    if !o.probed_guest {
                        if let (Some(ip), Some(mac)) = (o.guest_ip, o.guest_mac) {
                            o.probed_guest = true;
                            let _ = log.send(format!(
                                "[HOST:NET] probing_guest ip={}.{}.{}.{}",
                                ip[0], ip[1], ip[2], ip[3]
                            ));
                            out.push(arp_request_frame(mac, ip));
                        }
                    }
                }
                for reply in out {
                    if sock.send_to(&reply, &peer_addr).is_ok() {
                        obs.lock().unwrap().frames_out += 1;
                    }
                }
            }
        });

        Ok(WirePeer {
            host_port,
            guest_port,
            observations,
            stop,
        })
    }

    pub fn observations(&self) -> Observations {
        self.observations.lock().unwrap().clone()
    }

    /// One-line summary suitable for asserting on.
    pub fn summary(&self) -> String {
        let o = self.observations();
        format!(
            "[HOST:NET] peer_summary frames_in={} frames_out={} arp_requests={} arp_replies={} \
icmp_requests={} icmp_replies={} udp={} udp_echoed={} dns_queries={} dns_answers={} \
guest_arp_replies={} guest_icmp_replies={} hostile_sent={} replies_to_hostile={} \
bad_ip_csum={} bad_udp_csum={} bad_icmp_csum={} rejected={}",
            o.frames_in,
            o.frames_out,
            o.arp_requests,
            o.arp_replies_sent,
            o.icmp_echo_requests,
            o.icmp_echo_replies_sent,
            o.udp_datagrams,
            o.udp_echoed,
            o.dns_queries,
            o.dns_answers_sent,
            o.guest_arp_replies,
            o.guest_icmp_replies,
            o.hostile_sent,
            o.replies_to_hostile,
            o.bad_ip_checksums,
            o.bad_udp_checksums,
            o.bad_icmp_checksums,
            o.rejected,
        )
    }
}

impl Drop for WirePeer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// --- an independent, byte-level implementation of the peer side ------------

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// RFC 1071 one's-complement sum, folded.
fn checksum(parts: &[&[u8]]) -> u16 {
    let mut sum: u32 = 0;
    let mut odd: Option<u8> = None;
    for part in parts {
        let mut bytes = *part;
        if let Some(hi) = odd.take() {
            if let Some((&first, rest)) = bytes.split_first() {
                sum += u16::from_be_bytes([hi, first]) as u32;
                bytes = rest;
            } else {
                odd = Some(hi);
                continue;
            }
        }
        let (pairs, remainder) = bytes.as_chunks::<2>();
        for &c in pairs {
            sum += u16::from_be_bytes(c) as u32;
        }
        if let Some(&last) = remainder.first() {
            odd = Some(last);
        }
    }
    if let Some(hi) = odd {
        sum += u16::from_be_bytes([hi, 0]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn eth_frame(dst: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(14 + payload.len().max(46));
    f.extend_from_slice(&dst);
    f.extend_from_slice(&PEER_MAC);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    // Pad to the 60-byte minimum, as a real NIC would.
    while f.len() < 60 {
        f.push(0);
    }
    f
}

fn ipv4_packet(src: [u8; 4], dst: [u8; 4], protocol: u8, payload: &[u8], id: u16) -> Vec<u8> {
    let total = 20 + payload.len();
    let mut h = vec![0u8; 20];
    h[0] = 0x45;
    h[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    h[4..6].copy_from_slice(&id.to_be_bytes());
    h[6] = 0x40; // DF
    h[8] = 64; // TTL
    h[9] = protocol;
    h[12..16].copy_from_slice(&src);
    h[16..20].copy_from_slice(&dst);
    let sum = checksum(&[&h]);
    h[10..12].copy_from_slice(&sum.to_be_bytes());
    h.extend_from_slice(payload);
    h
}

fn udp_datagram(
    src: [u8; 4],
    dst: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let len = 8 + payload.len();
    let mut d = vec![0u8; 8];
    d[0..2].copy_from_slice(&src_port.to_be_bytes());
    d[2..4].copy_from_slice(&dst_port.to_be_bytes());
    d[4..6].copy_from_slice(&(len as u16).to_be_bytes());
    let mut pseudo = Vec::with_capacity(12);
    pseudo.extend_from_slice(&src);
    pseudo.extend_from_slice(&dst);
    pseudo.push(0);
    pseudo.push(PROTO_UDP);
    pseudo.extend_from_slice(&(len as u16).to_be_bytes());
    let mut sum = checksum(&[&pseudo, &d, payload]);
    if sum == 0 {
        sum = 0xFFFF;
    }
    d[6..8].copy_from_slice(&sum.to_be_bytes());
    d.extend_from_slice(payload);
    d
}

/// An ARP request asking the guest for its own address, sent to its unicast
/// MAC (which we already learned) rather than broadcast — a correct responder
/// answers either way, and unicast keeps the log readable.
fn arp_request_frame(guest_mac: [u8; 6], guest_ip: [u8; 4]) -> Vec<u8> {
    let mut arp = vec![0u8; 28];
    arp[0..2].copy_from_slice(&1u16.to_be_bytes());
    arp[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    arp[4] = 6;
    arp[5] = 4;
    arp[6..8].copy_from_slice(&1u16.to_be_bytes());
    arp[8..14].copy_from_slice(&PEER_MAC);
    arp[14..18].copy_from_slice(&PEER_IP);
    arp[24..28].copy_from_slice(&guest_ip);
    eth_frame(guest_mac, ETHERTYPE_ARP, &arp)
}

/// Five frames a correct stack must refuse, each exercising a different
/// rejection path. They are sent to the guest's own MAC so the card's filter
/// cannot be what saves it: the refusal has to come from the stack.
fn hostile_frames(guest_mac: [u8; 6], guest_ip: [u8; 4]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();

    // 1. IPv4 header checksum corrupted. Everything else is a valid ping, so
    //    only the checksum check can catch it.
    let mut echo = vec![8u8, 0, 0, 0, 0xBA, 0xD1, 0, 1];
    let sum = checksum(&[&echo]);
    echo[2..4].copy_from_slice(&sum.to_be_bytes());
    let mut bad_ip = ipv4_packet(PEER_IP, guest_ip, PROTO_ICMP, &echo, 2);
    bad_ip[10] ^= 0xFF;
    frames.push(eth_frame(guest_mac, ETHERTYPE_IPV4, &bad_ip));

    // 2. A valid ping addressed to someone else. Delivered to the guest's MAC,
    //    so only an IP-layer destination check can refuse it. Answering would
    //    make the guest a reflector for traffic it was never sent.
    let elsewhere = [10u8, 0, 2, 99];
    let mut echo2 = vec![8u8, 0, 0, 0, 0xBA, 0xD2, 0, 1];
    let sum2 = checksum(&[&echo2]);
    echo2[2..4].copy_from_slice(&sum2.to_be_bytes());
    let pkt = ipv4_packet(PEER_IP, elsewhere, PROTO_ICMP, &echo2, 3);
    frames.push(eth_frame(guest_mac, ETHERTYPE_IPV4, &pkt));

    // 3. UDP to a port nobody bound. Must be dropped, never answered (an ICMP
    //    port-unreachable would also be a reply, and this stack sends none).
    let datagram = udp_datagram(PEER_IP, guest_ip, 40000, 44444, b"nobody-home");
    let pkt = ipv4_packet(PEER_IP, guest_ip, PROTO_UDP, &datagram, 4);
    frames.push(eth_frame(guest_mac, ETHERTYPE_IPV4, &pkt));

    // 4. An 802.1Q-tagged frame. The parser refuses VLAN tags rather than
    //    reading the TPID as a protocol number.
    let mut vlan = Vec::new();
    vlan.extend_from_slice(&guest_mac);
    vlan.extend_from_slice(&PEER_MAC);
    vlan.extend_from_slice(&0x8100u16.to_be_bytes());
    vlan.extend_from_slice(&0x0064u16.to_be_bytes());
    vlan.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());
    while vlan.len() < 60 {
        vlan.push(0);
    }
    frames.push(vlan);

    // 5. ARP claiming a hardware type that is not Ethernet, with lengths that
    //    contradict it. A liberal parser would answer from the wrong offsets.
    let mut arp = vec![0u8; 28];
    arp[0..2].copy_from_slice(&6u16.to_be_bytes());
    arp[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    arp[4] = 8;
    arp[5] = 4;
    arp[6..8].copy_from_slice(&1u16.to_be_bytes());
    arp[8..14].copy_from_slice(&PEER_MAC);
    arp[14..18].copy_from_slice(&PEER_IP);
    arp[24..28].copy_from_slice(&guest_ip);
    frames.push(eth_frame(guest_mac, ETHERTYPE_ARP, &arp));

    frames
}

fn handle_frame(
    frame: &[u8],
    o: &mut Observations,
    arp_cache: &mut HashMap<[u8; 4], [u8; 6]>,
    out: &mut Vec<Vec<u8>>,
    log: &Sender<String>,
) {
    if frame.len() < 14 {
        o.rejected += 1;
        return;
    }
    let src_mac: [u8; 6] = frame[6..12].try_into().unwrap();
    if o.guest_mac.is_none() {
        o.guest_mac = Some(src_mac);
        let _ = log.send(format!(
            "[HOST:NET] guest_mac={}",
            src_mac
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":")
        ));
    }
    match be16(&frame[12..14]) {
        ETHERTYPE_ARP => handle_arp(&frame[14..], src_mac, o, arp_cache, out, log),
        ETHERTYPE_IPV4 => handle_ipv4(&frame[14..], src_mac, o, out, log),
        _ => o.rejected += 1,
    }
}

fn handle_arp(
    p: &[u8],
    src_mac: [u8; 6],
    o: &mut Observations,
    arp_cache: &mut HashMap<[u8; 4], [u8; 6]>,
    out: &mut Vec<Vec<u8>>,
    log: &Sender<String>,
) {
    if p.len() < 28 || be16(&p[0..2]) != 1 || be16(&p[2..4]) != 0x0800 || p[4] != 6 || p[5] != 4 {
        o.rejected += 1;
        return;
    }
    let op = be16(&p[6..8]);
    let sender_mac: [u8; 6] = p[8..14].try_into().unwrap();
    let sender_ip: [u8; 4] = p[14..18].try_into().unwrap();
    let target_ip: [u8; 4] = p[24..28].try_into().unwrap();
    arp_cache.insert(sender_ip, sender_mac);
    if op == 2 {
        // The guest answered our request. Follow it with a ping so the
        // guest's ICMP responder is exercised too.
        o.guest_arp_replies += 1;
        let _ = log.send(format!(
            "[HOST:NET] guest_arp_reply from={}.{}.{}.{}",
            sender_ip[0], sender_ip[1], sender_ip[2], sender_ip[3]
        ));
        let mut echo = vec![8u8, 0, 0, 0, 0x13, 0x37, 0, 1];
        echo.extend_from_slice(b"host-probe");
        let sum = checksum(&[&echo]);
        echo[2..4].copy_from_slice(&sum.to_be_bytes());
        let packet = ipv4_packet(PEER_IP, sender_ip, PROTO_ICMP, &echo, 1);
        out.push(eth_frame(sender_mac, ETHERTYPE_IPV4, &packet));
        return;
    }
    if op != 1 {
        return;
    }
    o.arp_requests += 1;
    if sender_ip == [10, 0, 2, 15] {
        o.arp_from_guest += 1;
    }
    if target_ip != PEER_IP && target_ip != DNS_IP {
        // Not for us: a correct guest does not ask us about other addresses,
        // so this is worth counting rather than ignoring.
        o.rejected += 1;
        return;
    }
    let _ = log.send(format!(
        "[HOST:NET] arp_request who_has={}.{}.{}.{} tell={}.{}.{}.{}",
        target_ip[0],
        target_ip[1],
        target_ip[2],
        target_ip[3],
        sender_ip[0],
        sender_ip[1],
        sender_ip[2],
        sender_ip[3]
    ));
    let mut reply = vec![0u8; 28];
    reply[0..2].copy_from_slice(&1u16.to_be_bytes());
    reply[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    reply[4] = 6;
    reply[5] = 4;
    reply[6..8].copy_from_slice(&2u16.to_be_bytes());
    reply[8..14].copy_from_slice(&PEER_MAC);
    reply[14..18].copy_from_slice(&target_ip);
    reply[18..24].copy_from_slice(&sender_mac);
    reply[24..28].copy_from_slice(&sender_ip);
    out.push(eth_frame(src_mac, ETHERTYPE_ARP, &reply));
    o.arp_replies_sent += 1;
}

fn handle_ipv4(
    p: &[u8],
    src_mac: [u8; 6],
    o: &mut Observations,
    out: &mut Vec<Vec<u8>>,
    log: &Sender<String>,
) {
    if p.len() < 20 || p[0] >> 4 != 4 {
        o.rejected += 1;
        return;
    }
    let ihl = (p[0] & 0x0F) as usize * 4;
    let total = be16(&p[2..4]) as usize;
    if ihl < 20 || total < ihl || p.len() < total {
        o.rejected += 1;
        return;
    }
    // Verify the guest's header checksum independently.
    if checksum(&[&p[..ihl]]) != 0 {
        o.bad_ip_checksums += 1;
        o.rejected += 1;
        return;
    }
    let src: [u8; 4] = p[12..16].try_into().unwrap();
    let dst: [u8; 4] = p[16..20].try_into().unwrap();
    if o.guest_ip.is_none() {
        o.guest_ip = Some(src);
        let _ = log.send(format!(
            "[HOST:NET] guest_ip={}.{}.{}.{}",
            src[0], src[1], src[2], src[3]
        ));
    }
    if dst != PEER_IP && dst != DNS_IP {
        o.rejected += 1;
        return;
    }
    let payload = &p[ihl..total];
    match p[9] {
        PROTO_ICMP => handle_icmp(payload, src, dst, src_mac, o, out, log),
        PROTO_UDP => handle_udp(payload, src, dst, src_mac, o, out, log),
        _ => o.rejected += 1,
    }
}

fn handle_icmp(
    p: &[u8],
    src: [u8; 4],
    dst: [u8; 4],
    src_mac: [u8; 6],
    o: &mut Observations,
    out: &mut Vec<Vec<u8>>,
    log: &Sender<String>,
) {
    if p.len() < 8 {
        o.rejected += 1;
        return;
    }
    if checksum(&[p]) != 0 {
        o.bad_icmp_checksums += 1;
        o.rejected += 1;
        return;
    }
    let id = be16(&p[4..6]);
    if p[0] == 0 && (id == 0xBAD1 || id == 0xBAD2) {
        // An echo reply carrying a hostile probe's identifier: the guest
        // answered a packet with a corrupt IP checksum, or one addressed to
        // another host.
        o.replies_to_hostile += 1;
        let _ = log.send(format!(
            "[HOST:NET] hostile_answered kind=icmp_echo id={id:#06x}"
        ));
        return;
    }
    if p[0] != 0 && p[0] != 8 {
        // Any ICMP error (unreachable, time exceeded) is also a response this
        // stack should never emit.
        o.replies_to_hostile += 1;
        let _ = log.send(format!(
            "[HOST:NET] hostile_answered kind=icmp_error type={}",
            p[0]
        ));
        return;
    }
    if p[0] == 0 {
        // The guest replied to OUR ping: its inbound ICMP path works. That is
        // also the cue to start the hostile sequence — the guest has proved it
        // answers legitimate traffic, so anything it answers from here on is a
        // failure to refuse.
        o.guest_icmp_replies += 1;
        if !o.hostile_sent {
            if let (Some(ip), Some(mac)) = (o.guest_ip, o.guest_mac) {
                o.hostile_sent = true;
                let _ = log.send("[HOST:NET] hostile_probe_start".to_string());
                out.extend(hostile_frames(mac, ip));
            }
        }
        let _ = log.send(format!(
            "[HOST:NET] guest_icmp_reply id={} seq={}",
            be16(&p[4..6]),
            be16(&p[6..8])
        ));
        return;
    }
    if p[0] != 8 {
        return;
    }

    o.icmp_echo_requests += 1;
    let id = be16(&p[4..6]);
    let seq = be16(&p[6..8]);
    let _ = log.send(format!("[HOST:NET] icmp_echo_request id={id} seq={seq}"));
    let mut reply = p.to_vec();
    reply[0] = 0; // echo reply
    reply[2] = 0;
    reply[3] = 0;
    let sum = checksum(&[&reply]);
    reply[2..4].copy_from_slice(&sum.to_be_bytes());
    let packet = ipv4_packet(dst, src, PROTO_ICMP, &reply, seq);
    out.push(eth_frame(src_mac, ETHERTYPE_IPV4, &packet));
    o.icmp_echo_replies_sent += 1;
}

fn handle_udp(
    p: &[u8],
    src: [u8; 4],
    dst: [u8; 4],
    src_mac: [u8; 6],
    o: &mut Observations,
    out: &mut Vec<Vec<u8>>,
    log: &Sender<String>,
) {
    if p.len() < 8 {
        o.rejected += 1;
        return;
    }
    let len = be16(&p[4..6]) as usize;
    if len < 8 || len > p.len() {
        o.rejected += 1;
        return;
    }
    let stored = be16(&p[6..8]);
    if stored != 0 {
        let mut pseudo = Vec::with_capacity(12);
        pseudo.extend_from_slice(&src);
        pseudo.extend_from_slice(&dst);
        pseudo.push(0);
        pseudo.push(PROTO_UDP);
        pseudo.extend_from_slice(&(len as u16).to_be_bytes());
        let mut header = p[..8].to_vec();
        header[6] = 0;
        header[7] = 0;
        let computed = checksum(&[&pseudo, &header, &p[8..len]]);
        let expect = if computed == 0 { 0xFFFF } else { computed };
        if expect != stored {
            o.bad_udp_checksums += 1;
            o.rejected += 1;
            return;
        }
    }
    let src_port = be16(&p[0..2]);
    let dst_port = be16(&p[2..4]);
    let payload = &p[8..len];
    if o.hostile_sent && dst_port == 40000 {
        // The black-holed probe was sent FROM port 40000; a datagram back to
        // it means the guest answered a port nobody had bound.
        o.replies_to_hostile += 1;
        let _ = log.send("[HOST:NET] hostile_answered kind=udp_blackhole".to_string());
        return;
    }
    o.udp_datagrams += 1;
    let _ = log.send(format!(
        "[HOST:NET] udp src_port={src_port} dst_port={dst_port} bytes={}",
        payload.len()
    ));
    if dst_port == ECHO_PORT && dst == PEER_IP {
        o.echo_payloads.push(payload.to_vec());
        let datagram = udp_datagram(dst, src, ECHO_PORT, src_port, payload);
        let packet = ipv4_packet(dst, src, PROTO_UDP, &datagram, src_port);
        out.push(eth_frame(src_mac, ETHERTYPE_IPV4, &packet));
        o.udp_echoed += 1;
        return;
    }
    if dst_port == 53 && dst == DNS_IP {
        o.dns_queries += 1;
        if let Some(answer) = dns_answer(payload, log) {
            let datagram = udp_datagram(dst, src, 53, src_port, &answer);
            let packet = ipv4_packet(dst, src, PROTO_UDP, &datagram, src_port);
            out.push(eth_frame(src_mac, ETHERTYPE_IPV4, &packet));
            o.dns_answers_sent += 1;
        }
    }
    // Any other port is a black hole on purpose.
}

/// Answer an A query for [`DNS_NAME`]; refuse anything else with NXDOMAIN, so
/// a guest that mis-encodes a name gets a real DNS error rather than silence.
fn dns_answer(query: &[u8], log: &Sender<String>) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let id = be16(&query[0..2]);
    if query[2] & 0x80 != 0 || be16(&query[4..6]) != 1 {
        return None;
    }
    // Decode the question name (no compression is legal in a query).
    let mut off = 12usize;
    let mut name = String::new();
    loop {
        let len = *query.get(off)? as usize;
        if len == 0 {
            off += 1;
            break;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        if !name.is_empty() {
            name.push('.');
        }
        let label = query.get(off + 1..off + 1 + len)?;
        name.push_str(&String::from_utf8_lossy(label));
        off += 1 + len;
    }
    let qtype = be16(query.get(off..off + 2)?);
    let qclass = be16(query.get(off + 2..off + 4)?);
    let question_end = off + 4;
    let _ = log.send(format!(
        "[HOST:NET] dns_query name={name} type={qtype} class={qclass}"
    ));

    let mut msg = Vec::with_capacity(64);
    msg.extend_from_slice(&id.to_be_bytes());
    let matched = name.eq_ignore_ascii_case(DNS_NAME) && qtype == 1 && qclass == 1;
    // QR=1, RD copied from the query, RA=1; RCODE 0 or 3 (NXDOMAIN).
    msg.push(0x80 | (query[2] & 0x01));
    msg.push(if matched { 0x80 } else { 0x83 });
    msg.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    msg.extend_from_slice(&(u16::from(matched)).to_be_bytes()); // ANCOUNT
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&query[12..question_end]);
    if matched {
        // Answer with a compression pointer back to the question name — the
        // form a real server uses, so the guest's pointer handling is
        // exercised rather than bypassed.
        msg.extend_from_slice(&[0xC0, 0x0C]);
        msg.extend_from_slice(&1u16.to_be_bytes()); // A
        msg.extend_from_slice(&1u16.to_be_bytes()); // IN
        msg.extend_from_slice(&60u32.to_be_bytes()); // TTL
        msg.extend_from_slice(&4u16.to_be_bytes());
        msg.extend_from_slice(&DNS_ANSWER);
    }
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_a_known_ipv4_header() {
        // RFC 1071's worked example header, checksum field zeroed.
        let header = [
            0x45u8, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0x00, 0x00, 0xc0, 0xa8,
            0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7,
        ];
        assert_eq!(checksum(&[&header]), 0xb861);
    }

    #[test]
    fn checksum_is_split_insensitive() {
        let data = [1u8, 2, 3, 4, 5, 6, 7];
        let whole = checksum(&[&data]);
        for split in 0..=data.len() {
            let (a, b) = data.split_at(split);
            assert_eq!(checksum(&[a, b]), whole, "split at {split}");
        }
    }

    #[test]
    fn a_built_ipv4_header_verifies() {
        let p = ipv4_packet(PEER_IP, [10, 0, 2, 15], PROTO_UDP, b"hello", 7);
        assert_eq!(checksum(&[&p[..20]]), 0);
    }

    #[test]
    fn a_built_udp_datagram_verifies() {
        let src = PEER_IP;
        let dst = [10u8, 0, 2, 15];
        let d = udp_datagram(src, dst, 7, 40100, b"payload");
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&src);
        pseudo.extend_from_slice(&dst);
        pseudo.push(0);
        pseudo.push(PROTO_UDP);
        pseudo.extend_from_slice(&(d.len() as u16).to_be_bytes());
        assert_eq!(checksum(&[&pseudo, &d]), 0);
    }

    #[test]
    fn dns_answers_only_its_own_name() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut q = vec![0u8; 12];
        q[0..2].copy_from_slice(&0x2020u16.to_be_bytes());
        q[2] = 0x01;
        q[4..6].copy_from_slice(&1u16.to_be_bytes());
        for label in DNS_NAME.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&1u16.to_be_bytes());
        q.extend_from_slice(&1u16.to_be_bytes());
        let answer = dns_answer(&q, &tx).unwrap();
        assert_eq!(&answer[0..2], &0x2020u16.to_be_bytes());
        assert_eq!(answer[3] & 0x0F, 0, "RCODE must be NOERROR");
        assert_eq!(be16(&answer[6..8]), 1, "one answer record");
        assert_eq!(&answer[answer.len() - 4..], &DNS_ANSWER);

        // A different name must come back NXDOMAIN with no answer records.
        let mut other = vec![0u8; 12];
        other[0..2].copy_from_slice(&1u16.to_be_bytes());
        other[2] = 0x01;
        other[4..6].copy_from_slice(&1u16.to_be_bytes());
        for label in "example.invalid".split('.') {
            other.push(label.len() as u8);
            other.extend_from_slice(label.as_bytes());
        }
        other.push(0);
        other.extend_from_slice(&1u16.to_be_bytes());
        other.extend_from_slice(&1u16.to_be_bytes());
        let nx = dns_answer(&other, &tx).unwrap();
        assert_eq!(nx[3] & 0x0F, 3);
        assert_eq!(be16(&nx[6..8]), 0);
    }

    #[test]
    fn arp_request_for_the_peer_is_answered_and_learned() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut o = Observations::default();
        let mut cache = HashMap::new();
        let mut out = Vec::new();
        let guest_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let mut arp = vec![0u8; 28];
        arp[0..2].copy_from_slice(&1u16.to_be_bytes());
        arp[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
        arp[4] = 6;
        arp[5] = 4;
        arp[6..8].copy_from_slice(&1u16.to_be_bytes());
        arp[8..14].copy_from_slice(&guest_mac);
        arp[14..18].copy_from_slice(&[10, 0, 2, 15]);
        arp[24..28].copy_from_slice(&PEER_IP);
        handle_arp(&arp, guest_mac, &mut o, &mut cache, &mut out, &tx);
        assert_eq!(o.arp_requests, 1);
        assert_eq!(o.arp_from_guest, 1);
        assert_eq!(o.arp_replies_sent, 1);
        assert_eq!(cache.get(&[10, 0, 2, 15]), Some(&guest_mac));
        let reply = &out[0];
        assert_eq!(&reply[0..6], &guest_mac);
        assert_eq!(&reply[6..12], &PEER_MAC);
        assert_eq!(be16(&reply[12..14]), ETHERTYPE_ARP);
        assert_eq!(be16(&reply[20..22]), 2, "opcode must be reply");
        assert!(reply.len() >= 60, "frame must be padded to the minimum");
    }

    #[test]
    fn a_bad_ip_checksum_is_counted_not_answered() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut o = Observations::default();
        let mut out = Vec::new();
        let mut p = ipv4_packet(
            [10, 0, 2, 15],
            PEER_IP,
            PROTO_ICMP,
            &[8, 0, 0, 0, 0, 0, 0, 0],
            1,
        );
        p[10] ^= 0xFF;
        handle_ipv4(&p, [0; 6], &mut o, &mut out, &tx);
        assert_eq!(o.bad_ip_checksums, 1);
        assert!(out.is_empty());
    }
}
