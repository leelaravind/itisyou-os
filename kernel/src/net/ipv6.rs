//! IPv6 foundations (V0.9): a link-local address, a SLAAC global address,
//! neighbour discovery and ICMPv6 echo — over the host-tested codec in
//! `kernel_core::net::ipv6`.
//!
//! The same rules as the IPv4 side: nothing unbounded (a fixed neighbour
//! cache that replaces its oldest entry, never grows), receive never blocks,
//! and the stack answers only what it must — neighbour solicitations for its
//! own addresses and echo requests — counting every drop by reason. IPv6 is
//! off until the `ipv6` command brings it up, so a guest that never asks for
//! it sends no IPv6 traffic at all.

use super::{mac, poll, tx_frame};
use kernel_core::net::eth::{EtherType, MacAddr};
use kernel_core::net::icmp::EchoKind;
use kernel_core::net::ipv6::{self as v6, Icmpv6Message, Ipv6Addr, NeighborAdvert};
use spin::Mutex;

const NEIGHBORS: usize = 8;
const MAX_PACKET: usize = super::MAX_FRAME - 14;

#[derive(Debug, Clone, Copy, Default)]
pub struct V6Stats {
    pub rx: u64,
    pub rx_malformed: u64,
    pub rx_unwanted: u64,
    pub router_adverts: u64,
    pub neighbor_adverts_sent: u64,
    pub echo_replies_sent: u64,
}

#[derive(Clone, Copy)]
struct Neighbor {
    addr: Ipv6Addr,
    mac: MacAddr,
    learned: u64,
}

struct State {
    enabled: bool,
    link_local: Ipv6Addr,
    global: Option<Ipv6Addr>,
    router: Option<Ipv6Addr>,
    neighbors: [Option<Neighbor>; NEIGHBORS],
    last_echo_reply: Option<(u16, u16)>,
    stats: V6Stats,
}

static V6: Mutex<State> = Mutex::new(State {
    enabled: false,
    link_local: Ipv6Addr::UNSPECIFIED,
    global: None,
    router: None,
    neighbors: [None; NEIGHBORS],
    last_echo_reply: None,
    stats: V6Stats {
        rx: 0,
        rx_malformed: 0,
        rx_unwanted: 0,
        router_adverts: 0,
        neighbor_adverts_sent: 0,
        echo_replies_sent: 0,
    },
});

impl State {
    fn ours(&self, a: &Ipv6Addr) -> bool {
        *a == self.link_local || Some(*a) == self.global
    }

    /// Multicast groups this host listens on: all-nodes, and the
    /// solicited-node group of its addresses (both share one interface id).
    fn listening(&self, a: &Ipv6Addr) -> bool {
        *a == Ipv6Addr::ALL_NODES || *a == self.link_local.solicited_node()
    }

    fn neighbor(&self, a: &Ipv6Addr) -> Option<MacAddr> {
        self.neighbors
            .iter()
            .flatten()
            .find(|n| n.addr == *a)
            .map(|n| n.mac)
    }

    fn learn(&mut self, addr: Ipv6Addr, mac: MacAddr) {
        if addr.is_unspecified() || addr.is_multicast() || !mac.is_unicast() {
            return;
        }
        let now = crate::interrupts::ticks();
        if let Some(n) = self.neighbors.iter_mut().flatten().find(|n| n.addr == addr) {
            n.mac = mac;
            n.learned = now;
            return;
        }
        let slot = match self.neighbors.iter().position(Option::is_none) {
            Some(i) => i,
            None => (0..NEIGHBORS)
                .min_by_key(|&i| self.neighbors[i].map_or(0, |n| n.learned))
                .unwrap_or(0),
        };
        self.neighbors[slot] = Some(Neighbor {
            addr,
            mac,
            learned: now,
        });
    }
}

/// Bring IPv6 up on the NIC: derive the link-local address from the MAC and
/// join the two multicast groups neighbour discovery needs. Idempotent.
pub fn up() -> Option<Ipv6Addr> {
    if !crate::device::e1000::present() {
        return None;
    }
    let link_local = Ipv6Addr::link_local_from_mac(mac());
    let newly = {
        let mut s = V6.lock();
        let newly = !s.enabled;
        s.enabled = true;
        s.link_local = link_local;
        newly
    };
    if newly {
        crate::device::e1000::with(|nic| {
            nic.join_multicast(Ipv6Addr::ALL_NODES.multicast_mac());
            nic.join_multicast(link_local.solicited_node().multicast_mac());
        });
        let mut buf = [0u8; 39];
        crate::serial_println!(
            "[ITISYOU:NET6] up link_local={}",
            link_local.format(&mut buf)
        );
    }
    Some(link_local)
}

pub fn addresses() -> Option<(Ipv6Addr, Option<Ipv6Addr>, Option<Ipv6Addr>)> {
    let s = V6.lock();
    s.enabled.then_some((s.link_local, s.global, s.router))
}

pub fn stats() -> V6Stats {
    V6.lock().stats
}

/// Source address for talking to `dst`: link-local stays link-local.
fn source_for(dst: &Ipv6Addr) -> Ipv6Addr {
    let s = V6.lock();
    if dst.is_link_local() || dst.is_multicast() {
        s.link_local
    } else {
        s.global.unwrap_or(s.link_local)
    }
}

fn send(dst_mac: MacAddr, src: Ipv6Addr, dst: Ipv6Addr, hop: u8, icmp: &[u8]) -> bool {
    let mut packet = [0u8; MAX_PACKET];
    let Ok(n) = v6::build_packet(&mut packet, src, dst, hop, v6::next_header::ICMPV6, icmp) else {
        return false;
    };
    tx_frame(dst_mac, EtherType::IPV6, &packet[..n])
}

/// Receive path, called for every IPv6 frame the card accepted.
pub fn on_frame(payload: &[u8]) {
    let enabled = {
        let mut s = V6.lock();
        s.stats.rx += 1;
        s.enabled
    };
    if !enabled {
        V6.lock().stats.rx_unwanted += 1;
        return;
    }
    let Ok(packet) = v6::parse(payload) else {
        V6.lock().stats.rx_malformed += 1;
        return;
    };
    {
        let s = V6.lock();
        // The card's multicast filter is a hash and admits colliding groups;
        // the address itself decides.
        if !s.ours(&packet.dst) && !s.listening(&packet.dst) {
            drop(s);
            V6.lock().stats.rx_unwanted += 1;
            return;
        }
    }
    if packet.next_header != v6::next_header::ICMPV6 {
        V6.lock().stats.rx_unwanted += 1;
        return;
    }
    let Ok(message) = v6::parse_icmp(&packet) else {
        V6.lock().stats.rx_malformed += 1;
        return;
    };
    match message {
        Icmpv6Message::Echo(echo) if echo.kind == EchoKind::Reply => {
            V6.lock().last_echo_reply = Some((echo.identifier, echo.sequence));
        }
        Icmpv6Message::Echo(echo) => {
            // Echo requests are answered only when addressed to us directly.
            if !V6.lock().ours(&packet.dst) {
                return;
            }
            let mut msg = [0u8; MAX_PACKET];
            let Ok(n) = v6::build_echo(
                &mut msg,
                packet.dst,
                packet.src,
                EchoKind::Reply,
                echo.identifier,
                echo.sequence,
                echo.payload,
            ) else {
                return;
            };
            // Cache only: this runs inside `poll`, and resolving here would
            // re-enter `poll` from itself. A peer that pings us has just
            // resolved us by neighbour solicitation, which taught us its MAC.
            let dst_mac = V6.lock().neighbor(&packet.src);
            if let Some(m) = dst_mac {
                if send(m, packet.dst, packet.src, v6::DEFAULT_HOP_LIMIT, &msg[..n]) {
                    V6.lock().stats.echo_replies_sent += 1;
                }
            }
        }
        Icmpv6Message::RouterAdvert(ra) => {
            let mut s = V6.lock();
            s.stats.router_adverts += 1;
            s.router = Some(packet.src);
            if let Some(m) = ra.source_ll {
                s.learn(packet.src, m);
            }
            let slaac = ra.prefixes.iter().flatten().find_map(|p| p.slaac_prefix());
            if let Some(prefix) = slaac {
                let global = Ipv6Addr::with_prefix(prefix, mac());
                if s.global != Some(global) {
                    s.global = Some(global);
                    drop(s);
                    let (mut g, mut r) = ([0u8; 39], [0u8; 39]);
                    crate::serial_println!(
                        "[ITISYOU:NET6] slaac global={} router={} router_lifetime={}",
                        global.format(&mut g),
                        packet.src.format(&mut r),
                        ra.router_lifetime
                    );
                }
            }
        }
        Icmpv6Message::NeighborSolicit(ns) => {
            if !V6.lock().ours(&ns.target) {
                return;
            }
            let from_unspecified = packet.src.is_unspecified();
            if let (false, Some(m)) = (from_unspecified, ns.source_ll) {
                V6.lock().learn(packet.src, m);
            }
            // RFC 4861 §7.2.4: answer to the solicitor, or to all-nodes when
            // it has no address yet (duplicate address detection).
            let dst = if from_unspecified {
                Ipv6Addr::ALL_NODES
            } else {
                packet.src
            };
            let na = NeighborAdvert {
                router: false,
                solicited: !from_unspecified,
                overrides: true,
                target: ns.target,
                target_ll: Some(mac()),
            };
            let mut msg = [0u8; 128];
            let Ok(n) = v6::build_neighbor_advert(&mut msg, ns.target, dst, &na) else {
                return;
            };
            let dst_mac = if from_unspecified {
                Some(dst.multicast_mac())
            } else {
                ns.source_ll.or_else(|| V6.lock().neighbor(&packet.src))
            };
            if let Some(m) = dst_mac {
                if send(m, ns.target, dst, v6::NDP_HOP_LIMIT, &msg[..n]) {
                    V6.lock().stats.neighbor_adverts_sent += 1;
                }
            }
        }
        Icmpv6Message::NeighborAdvert(na) => {
            if let Some(m) = na.target_ll {
                V6.lock().learn(na.target, m);
            }
        }
        Icmpv6Message::RouterSolicit(_) => {
            // A host does not answer router solicitations.
            V6.lock().stats.rx_unwanted += 1;
        }
    }
}

/// Resolve `target` to a MAC with neighbour solicitation. Blocking with a real
/// deadline; kernel context only.
pub fn resolve(target: Ipv6Addr, timeout_ms: u64) -> Option<MacAddr> {
    if target.is_multicast() {
        return Some(target.multicast_mac());
    }
    if let Some(m) = V6.lock().neighbor(&target) {
        return Some(m);
    }
    let src = source_for(&target);
    let group = target.solicited_node();
    let mut msg = [0u8; 128];
    let n = v6::build_neighbor_solicit(&mut msg, src, group, target, Some(mac())).ok()?;
    let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
    let mut resend = crate::interrupts::Deadline::after_ms(0);
    loop {
        if !resend.pending() {
            send(
                group.multicast_mac(),
                src,
                group,
                v6::NDP_HOP_LIMIT,
                &msg[..n],
            );
            resend = crate::interrupts::Deadline::after_ms(300);
        }
        poll();
        if let Some(m) = V6.lock().neighbor(&target) {
            return Some(m);
        }
        if !deadline.pending() {
            return None;
        }
    }
}

/// Send router solicitations until a router advertisement gives this host a
/// SLAAC address, or the deadline passes.
pub fn solicit_router(timeout_ms: u64) -> Option<Ipv6Addr> {
    let link_local = up()?;
    let mut msg = [0u8; 64];
    let n =
        v6::build_router_solicit(&mut msg, link_local, Ipv6Addr::ALL_ROUTERS, Some(mac())).ok()?;
    let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
    let mut resend = crate::interrupts::Deadline::after_ms(0);
    loop {
        if !resend.pending() {
            send(
                Ipv6Addr::ALL_ROUTERS.multicast_mac(),
                link_local,
                Ipv6Addr::ALL_ROUTERS,
                v6::NDP_HOP_LIMIT,
                &msg[..n],
            );
            resend = crate::interrupts::Deadline::after_ms(1000);
        }
        poll();
        if let Some(g) = V6.lock().global {
            return Some(g);
        }
        if !deadline.pending() {
            return None;
        }
    }
}

/// One ICMPv6 echo exchange with `dst`. Kernel context only.
pub fn ping(dst: Ipv6Addr, id: u16, seq: u16, timeout_ms: u64) -> bool {
    let Some(dst_mac) = resolve(dst, 1000) else {
        return false;
    };
    let src = source_for(&dst);
    V6.lock().last_echo_reply = None;
    let mut msg = [0u8; 64];
    let Ok(n) = v6::build_echo(
        &mut msg,
        src,
        dst,
        EchoKind::Request,
        id,
        seq,
        b"itisyou-ping6",
    ) else {
        return false;
    };
    if !send(dst_mac, src, dst, v6::DEFAULT_HOP_LIMIT, &msg[..n]) {
        return false;
    }
    let mut deadline = crate::interrupts::Deadline::after_ms(timeout_ms);
    loop {
        poll();
        if V6.lock().last_echo_reply == Some((id, seq)) {
            return true;
        }
        if !deadline.pending() {
            return false;
        }
    }
}
