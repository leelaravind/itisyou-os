//! IPv6 foundations (RFC 8200, 4291, 4443, 4861, 5952): addresses, the fixed
//! header, ICMPv6 echo and Neighbor Discovery. Strict by design:
//!
//! * **Extension headers are refused, not walked.** A chain of them puts
//!   attacker-chosen length and ordering in front of the payload, and the
//!   routing and fragment headers are where mature stacks have broken (RH0
//!   amplification, overlapping fragments). A next header other than ICMPv6,
//!   UDP or TCP is [`Ipv6Error::Unsupported`], carrying the value to count.
//! * **NDP is validated exactly as RFC 4861 §6.1/§7.1 say.** It rewrites the
//!   neighbor cache or default route from an unauthenticated datagram, and hop
//!   limit 255 is the only evidence it came from on-link. Every rule has its
//!   own error; the ICMPv6 checksum's pseudo-header binds it to its addresses.
//! * **Unknown options are skipped; known ones are checked to the byte.**
//!   Skipping is a MUST (RFC 4861 §4.6) — QEMU's router appends an RDNSS
//!   option — but every length is bounds-checked before anything is skipped.

use super::checksum::Checksum;
use super::eth::MacAddr;
use super::icmp::EchoKind;

/// Fixed IPv6 header length; options live in (refused) extension headers.
pub const HEADER_LEN: usize = 40;
/// Hop limit for ordinary traffic, matching this stack's IPv4 TTL.
pub const DEFAULT_HOP_LIMIT: u8 = 64;
/// Every NDP message is sent with, and must arrive with, this hop limit (RFC
/// 4861 §3.1): routers decrement it, so 255 proves the sender is on-link.
pub const NDP_HOP_LIMIT: u8 = 255;
/// Prefix options a [`RouterAdvert`] stores; any beyond are counted.
pub const MAX_PREFIXES: usize = 4;
/// Offset of the checksum in every ICMPv6 message.
pub const ICMP_CHECKSUM_OFFSET: usize = 2;
/// Fixed part of an echo message: type, code, checksum, identifier, sequence.
pub const ECHO_HEADER_LEN: usize = 8;

/// Next-header values handed to an upper layer. Anything else, every
/// extension header included, is [`Ipv6Error::Unsupported`].
pub mod next_header {
    /// Transmission Control Protocol.
    pub const TCP: u8 = 6;
    /// User Datagram Protocol.
    pub const UDP: u8 = 17;
    /// ICMP for IPv6 (RFC 4443).
    pub const ICMPV6: u8 = 58;
}

/// ICMPv6 types parsed or built here. Everything else — the error messages,
/// MLD, and Redirect, which would rewrite routing from an unauthenticated
/// datagram — is [`Icmpv6Error::UnsupportedType`].
pub mod icmp_types {
    /// Echo Request (RFC 4443 §4.1).
    pub const ECHO_REQUEST: u8 = 128;
    /// Echo Reply (RFC 4443 §4.2).
    pub const ECHO_REPLY: u8 = 129;
    /// Router Solicitation (RFC 4861 §4.1).
    pub const ROUTER_SOLICIT: u8 = 133;
    /// Router Advertisement (RFC 4861 §4.2).
    pub const ROUTER_ADVERT: u8 = 134;
    /// Neighbor Solicitation (RFC 4861 §4.3).
    pub const NEIGHBOR_SOLICIT: u8 = 135;
    /// Neighbor Advertisement (RFC 4861 §4.4).
    pub const NEIGHBOR_ADVERT: u8 = 136;
}

/// NDP option types consumed here (RFC 4861 §4.6); others are skipped.
pub mod nd_option {
    /// Source Link-Layer Address.
    pub const SOURCE_LL: u8 = 1;
    /// Target Link-Layer Address.
    pub const TARGET_LL: u8 = 2;
    /// Prefix Information.
    pub const PREFIX_INFO: u8 = 3;
    /// MTU.
    pub const MTU: u8 = 5;
}

/// A 128-bit IPv6 address in network byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ipv6Addr(pub [u8; 16]);

impl Ipv6Addr {
    /// `::` — no address yet; the source of a DAD probe, never a peer.
    pub const UNSPECIFIED: Ipv6Addr = Ipv6Addr([0; 16]);
    /// `ff02::1`, every node on the link: where unsolicited RAs are sent.
    pub const ALL_NODES: Ipv6Addr = Ipv6Addr([0xFF, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    /// `ff02::2`, every router on the link: where a host sends its RS.
    pub const ALL_ROUTERS: Ipv6Addr = Ipv6Addr([0xFF, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);

    /// Build from a slice, rejecting any length other than sixteen.
    pub fn from_slice(bytes: &[u8]) -> Option<Ipv6Addr> {
        Some(Ipv6Addr(bytes.try_into().ok()?))
    }

    /// The eight big-endian groups the text form is written in.
    pub fn segments(&self) -> [u16; 8] {
        let mut out = [0u16; 8];
        for (seg, pair) in out.iter_mut().zip(self.0.as_chunks::<2>().0) {
            *seg = u16::from_be_bytes(*pair);
        }
        out
    }

    /// `::`.
    pub fn is_unspecified(&self) -> bool {
        self.0 == [0; 16]
    }

    /// `ff00::/8`; never a legal source address (RFC 4291 §2.7).
    pub fn is_multicast(&self) -> bool {
        self.0[0] == 0xFF
    }

    /// Link-local unicast, `fe80::/10`: the only source an RA may come from.
    pub fn is_link_local(&self) -> bool {
        self.0[0] == 0xFE && self.0[1] & 0xC0 == 0x80
    }

    /// `fe80::/64` plus the modified EUI-64 of `mac` — the address a host
    /// has before any router speaks.
    pub fn link_local_from_mac(mac: MacAddr) -> Ipv6Addr {
        Ipv6Addr::with_prefix([0xFE, 0x80, 0, 0, 0, 0, 0, 0], mac)
    }

    /// A SLAAC address: a /64 `prefix` plus the modified EUI-64 of `mac`
    /// (RFC 4291 appendix A: `ff:fe` inserted, universal/local bit flipped).
    /// Only a /64 leaves room for it; see [`PrefixInfo::slaac_prefix`].
    pub fn with_prefix(prefix: [u8; 8], mac: MacAddr) -> Ipv6Addr {
        let [m0, m1, m2, m3, m4, m5] = mac.0;
        let mut a = [0u8; 16];
        a[..8].copy_from_slice(&prefix);
        a[8..].copy_from_slice(&[m0 ^ 0x02, m1, m2, 0xFF, 0xFE, m3, m4, m5]);
        Ipv6Addr(a)
    }

    /// The solicited-node group `ff02::1:ffXX:XXXX` (RFC 4291 §2.7.1): where
    /// an NS for `self` is sent, so a host joins it for each own address.
    pub fn solicited_node(&self) -> Ipv6Addr {
        let [.., a13, a14, a15] = self.0;
        Ipv6Addr([0xFF, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xFF, a13, a14, a15])
    }

    /// The Ethernet destination for a multicast `self`: `33:33` plus its low
    /// 32 bits (RFC 2464 §7). A unicast peer's MAC comes from NDP instead.
    pub fn multicast_mac(&self) -> MacAddr {
        let [.., a12, a13, a14, a15] = self.0;
        MacAddr([0x33, 0x33, a12, a13, a14, a15])
    }

    /// RFC 5952 canonical text into a caller buffer (39 bytes is the longest
    /// form) — lowercase, no leading zeros, the longest run of two or more
    /// zero groups (the first, on a tie) as `::`, a lone zero group as `0`.
    /// The mixed `::ffff:1.2.3.4` form is not produced: this stack never
    /// carries IPv4-mapped addresses.
    pub fn format<'a>(&self, out: &'a mut [u8; 39]) -> &'a str {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let seg = self.segments();
        // Strict `>` keeps the first run on a tie (§4.2.3).
        let (mut best, mut run) = ((0, 0), 0);
        for (i, &group) in seg.iter().enumerate() {
            run = if group == 0 { run + 1 } else { 0 };
            if run > best.1 {
                best = (i + 1 - run, run);
            }
        }
        // `8` is a sentinel no index reaches: nothing to compress.
        let (start, end) = if best.1 >= 2 {
            (best.0, best.0 + best.1)
        } else {
            (8, 8)
        };
        // Eight 4-digit groups and seven colons is 39; compression only ever
        // shortens that, so every index below is in bounds.
        let (mut n, mut i) = (0, 0);
        while i < seg.len() {
            if i == start {
                out[n..n + 2].copy_from_slice(b"::");
                (n, i) = (n + 2, end);
                continue;
            }
            if i > 0 && i != end {
                out[n] = b':';
                n += 1;
            }
            for shift in [12u32, 8, 4, 0] {
                if seg[i] >> shift != 0 || shift == 0 {
                    out[n] = HEX[usize::from((seg[i] >> shift) & 0xF)];
                    n += 1;
                }
            }
            i += 1;
        }
        core::str::from_utf8(&out[..n]).unwrap_or("::")
    }
}

impl core::fmt::Display for Ipv6Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.format(&mut [0u8; 39]))
    }
}

/// Every way a received IPv6 header is refused, plus the build errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv6Error {
    /// Fewer than [`HEADER_LEN`] bytes.
    TooShort,
    /// Version nibble is not 6.
    BadVersion,
    /// Payload length claims more bytes than follow the header.
    PayloadLengthExceedsBuffer,
    /// Multicast source: answering one would turn a single forged echo request
    /// into a reply to every member of the group.
    MulticastSource,
    /// A next header this stack does not parse; carried so the caller can
    /// count extension headers separately from unknown protocols.
    Unsupported(u8),
    /// Build: destination buffer too small.
    BufferTooSmall,
    /// Build: payload does not fit the 16-bit payload-length field.
    PayloadTooLarge,
}

/// A validated packet borrowing the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv6Packet<'a> {
    /// DSCP/ECN byte; preserved so a reply can mirror it.
    pub traffic_class: u8,
    /// The 20-bit flow label, in the low bits.
    pub flow_label: u32,
    /// One of [`next_header`]'s values, or [`parse`] would have refused it.
    pub next_header: u8,
    /// Reported, not judged: this host never forwards, and the one rule that
    /// depends on it (NDP's 255) is enforced by [`parse_icmp`].
    pub hop_limit: u8,
    /// Sender; never multicast.
    pub src: Ipv6Addr,
    /// Destination; the caller decides whether it is one of ours.
    pub dst: Ipv6Addr,
    /// Exactly payload-length bytes: trailing Ethernet padding is trimmed.
    pub payload: &'a [u8],
}

/// The 16 address bytes at constant offset `at` of a fixed-size array; the
/// array type guarantees the range whatever the input bytes are.
fn addr_at<const N: usize>(bytes: &[u8; N], at: usize) -> Ipv6Addr {
    let mut a = [0u8; 16];
    a.copy_from_slice(&bytes[at..at + 16]);
    Ipv6Addr(a)
}

fn check_next_header(next: u8) -> Result<(), Ipv6Error> {
    match next {
        next_header::ICMPV6 | next_header::UDP | next_header::TCP => Ok(()),
        other => Err(Ipv6Error::Unsupported(other)),
    }
}

/// Parse and validate a received packet. `bytes` may be longer than the
/// packet (Ethernet pads short frames), never shorter than it claims.
pub fn parse(bytes: &[u8]) -> Result<Ipv6Packet<'_>, Ipv6Error> {
    let Some((h, rest)) = bytes.split_first_chunk::<HEADER_LEN>() else {
        return Err(Ipv6Error::TooShort);
    };
    if h[0] >> 4 != 6 {
        return Err(Ipv6Error::BadVersion);
    }
    let len = usize::from(u16::from_be_bytes([h[4], h[5]]));
    let payload = rest
        .get(..len)
        .ok_or(Ipv6Error::PayloadLengthExceedsBuffer)?;
    let src = addr_at(h, 8);
    if src.is_multicast() {
        return Err(Ipv6Error::MulticastSource);
    }
    // Last, so `Unsupported` is only reported for a well-formed header.
    check_next_header(h[6])?;
    Ok(Ipv6Packet {
        traffic_class: (h[0] << 4) | (h[1] >> 4),
        flow_label: u32::from_be_bytes([0, h[1] & 0x0F, h[2], h[3]]),
        next_header: h[6],
        hop_limit: h[7],
        src,
        dst: addr_at(h, 24),
        payload,
    })
}

/// Build a packet — the 40-byte header (traffic class and flow label zero),
/// then `payload` — into `buf`, returning its length. Refuses what [`parse`]
/// would: an unsupported next header or a multicast source is never emitted.
pub fn build_packet(
    buf: &mut [u8],
    src: Ipv6Addr,
    dst: Ipv6Addr,
    hop_limit: u8,
    next_header: u8,
    payload: &[u8],
) -> Result<usize, Ipv6Error> {
    let len = u16::try_from(payload.len()).map_err(|_| Ipv6Error::PayloadTooLarge)?;
    check_next_header(next_header)?;
    if src.is_multicast() {
        return Err(Ipv6Error::MulticastSource);
    }
    let total = HEADER_LEN + payload.len();
    let out = buf.get_mut(..total).ok_or(Ipv6Error::BufferTooSmall)?;
    out[..4].copy_from_slice(&[0x60, 0, 0, 0]); // version 6, class 0, flow 0
    out[4..6].copy_from_slice(&len.to_be_bytes());
    out[6] = next_header;
    out[7] = hop_limit;
    out[8..24].copy_from_slice(&src.0);
    out[24..40].copy_from_slice(&dst.0);
    out[HEADER_LEN..].copy_from_slice(payload);
    Ok(total)
}

/// The pseudo-header prefixed to every upper-layer checksum over IPv6 (RFC
/// 8200 §8.1): source, destination, 32-bit upper-layer length, three zero
/// bytes, next header. Never sent; it makes a re-addressed message fail.
pub fn pseudo_header(src: Ipv6Addr, dst: Ipv6Addr, next_header: u8, upper_len: u32) -> [u8; 40] {
    let mut ph = [0u8; 40];
    ph[..16].copy_from_slice(&src.0);
    ph[16..32].copy_from_slice(&dst.0);
    ph[32..36].copy_from_slice(&upper_len.to_be_bytes());
    ph[39] = next_header;
    ph
}

/// The IPv6 form of [`super::checksum::transport_checksum`]: pseudo-header +
/// `header` (the two bytes at `checksum_offset` read as zero) + `payload`.
///
/// Returns the plain complement: ICMPv6 sends it as-is, while UDP must still
/// send a zero result as `0xFFFF`. `None` if the length overflows 32 bits.
pub fn transport_checksum(
    src: Ipv6Addr,
    dst: Ipv6Addr,
    next_header: u8,
    header: &[u8],
    checksum_offset: usize,
    payload: &[u8],
) -> Option<u16> {
    let total = u32::try_from(header.len().checked_add(payload.len())?).ok()?;
    let mut c = Checksum::new();
    c.push(&pseudo_header(src, dst, next_header, total));
    let after = checksum_offset.saturating_add(2);
    match (header.get(..checksum_offset), header.get(after..)) {
        (Some(head), Some(tail)) => {
            c.push(head);
            c.push_zeros(2);
            c.push(tail);
        }
        _ => c.push(header),
    }
    c.push(payload);
    Some(c.finish())
}

/// An intact ICMPv6 message, checksum included, sums to zero over the
/// pseudo-header. The checksum is mandatory: zero never means "omitted".
fn icmp_checksum_ok(src: Ipv6Addr, dst: Ipv6Addr, message: &[u8]) -> bool {
    let Ok(len) = u32::try_from(message.len()) else {
        return false;
    };
    let mut c = Checksum::new();
    c.push(&pseudo_header(src, dst, next_header::ICMPV6, len));
    c.push(message);
    c.finish() == 0
}

/// Every way an ICMPv6 message is refused, plus the build errors. The NDP
/// variants map one-to-one onto RFC 4861's §6.1/§7.1 validity rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icmpv6Error {
    /// The packet's next header is not ICMPv6.
    NotIcmpv6,
    /// Shorter than four bytes, or than its type's fixed part.
    TooShort,
    /// The checksum over pseudo-header + message did not verify.
    BadChecksum,
    /// A type this module deliberately does not act on (see [`icmp_types`]).
    UnsupportedType(u8),
    /// A non-zero code; every type parsed here defines only code 0.
    BadCode,
    /// An NDP message whose hop limit is not 255: it was forwarded.
    NdpHopLimit,
    /// An option with length zero — skipping it would never advance.
    OptionZeroLength,
    /// An option, or a stray trailing byte, running past the message end.
    OptionOverrun,
    /// A consumed option of the wrong size for its type, or a prefix longer
    /// than 128 bits.
    BadOption,
    /// A second link-layer or MTU option: which to believe would be a guess,
    /// and "last one wins" is a cache-poisoning primitive.
    DuplicateOption,
    /// A link-layer option carrying a multicast or all-zero MAC.
    BadLinkLayerAddress,
    /// NS/NA target is multicast.
    MulticastTarget,
    /// RA whose source is not link-local.
    RouterNotLinkLocal,
    /// RS or NS from `::` carrying a Source Link-Layer Address option.
    UnspecifiedSourceWithLinkLayer,
    /// NS from `::` (a DAD probe) not sent to the target's solicited-node group.
    DadNotSolicitedNode,
    /// NA with the Solicited flag sent to a multicast destination.
    SolicitedToMulticast,
    /// Build: destination buffer too small.
    BufferTooSmall,
    /// Build: the message would not fit an IPv6 payload.
    PayloadTooLarge,
}

/// A validated echo message borrowing the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Echo<'a> {
    /// Request or reply.
    pub kind: EchoKind,
    /// Chosen by the requester, mirrored by the replier.
    pub identifier: u16,
    /// Chosen by the requester, mirrored by the replier.
    pub sequence: u16,
    /// Echo data, mirrored by the replier.
    pub payload: &'a [u8],
}

impl Echo<'_> {
    /// Is this the reply to our request with this identifier and sequence?
    /// Both must match, as for [`super::icmp::Echo::answers`].
    pub fn answers(&self, identifier: u16, sequence: u16) -> bool {
        self.kind == EchoKind::Reply && self.identifier == identifier && self.sequence == sequence
    }
}

/// A Router Solicitation. Hosts ignore these (RFC 4861 §6.2.6); it is parsed
/// so it can be counted and so the builder can be verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterSolicit {
    /// The soliciting host's MAC, when it has an address to send from.
    pub source_ll: Option<MacAddr>,
}

/// One Prefix Information option (RFC 4861 §4.6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixInfo {
    /// Significant leading bits of `prefix`, at most 128.
    pub prefix_len: u8,
    /// L flag: addresses under the prefix are on-link.
    pub on_link: bool,
    /// A flag: the prefix may be used for SLAAC.
    pub autonomous: bool,
    /// Seconds the prefix stays valid; `u32::MAX` is infinity.
    pub valid_lifetime: u32,
    /// Seconds addresses from it stay preferred; `u32::MAX` is infinity.
    pub preferred_lifetime: u32,
    /// The prefix, as sent: bits past `prefix_len` are not masked.
    pub prefix: Ipv6Addr,
}

impl PrefixInfo {
    /// The upper 64 bits for [`Ipv6Addr::with_prefix`] when RFC 4862 §5.5.3
    /// lets a host form an address here: A flag set, not link-local, valid
    /// lifetime non-zero and no shorter than the preferred one, and exactly
    /// /64 (the only length that fits an EUI-64). A multicast prefix is
    /// refused too — the result would not be unicast. `None` means "forms no
    /// address", not "malformed".
    pub fn slaac_prefix(&self) -> Option<[u8; 8]> {
        if !self.autonomous
            || self.prefix_len != 64
            || self.prefix.is_link_local()
            || self.prefix.is_multicast()
            || self.valid_lifetime == 0
            || self.preferred_lifetime > self.valid_lifetime
        {
            return None;
        }
        self.prefix.0.first_chunk::<8>().copied()
    }
}

/// A Router Advertisement (RFC 4861 §4.2). Zero `cur_hop_limit`,
/// `reachable_time` or `retrans_timer` mean "unspecified"; a zero
/// `router_lifetime` means "not a default router".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterAdvert {
    /// Hop limit the router suggests for outgoing packets.
    pub cur_hop_limit: u8,
    /// M flag: addresses are available via DHCPv6.
    pub managed: bool,
    /// O flag: other configuration is available via DHCPv6.
    pub other_config: bool,
    /// Seconds this router may serve as a default router.
    pub router_lifetime: u16,
    /// Milliseconds a neighbor stays reachable after confirmation.
    pub reachable_time: u32,
    /// Milliseconds between retransmitted Neighbor Solicitations.
    pub retrans_timer: u32,
    /// The router's MAC, from a Source Link-Layer Address option.
    pub source_ll: Option<MacAddr>,
    /// Advertised link MTU, raw: the caller ignores one below IPv6's 1280 or
    /// above what its NIC supports (RFC 4861 §6.3.4).
    pub mtu: Option<u32>,
    /// The first [`MAX_PREFIXES`] Prefix Information options, in order.
    pub prefixes: [Option<PrefixInfo>; MAX_PREFIXES],
    /// Prefix options beyond [`MAX_PREFIXES`], counted rather than lost.
    pub excess_prefixes: u16,
}

/// A Neighbor Solicitation (RFC 4861 §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeighborSolicit {
    /// The address being resolved or probed; never multicast.
    pub target: Ipv6Addr,
    /// The solicitor's MAC; always `None` for a DAD probe from `::`.
    pub source_ll: Option<MacAddr>,
}

/// A Neighbor Advertisement (RFC 4861 §4.4); also the builder's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeighborAdvert {
    /// R flag: the sender is a router.
    pub router: bool,
    /// S flag: answers a solicitation. Never set towards a multicast group.
    pub solicited: bool,
    /// O flag: override an existing neighbor-cache entry.
    pub overrides: bool,
    /// The address whose link-layer address is advertised; never multicast.
    pub target: Ipv6Addr,
    /// The target's MAC, from a Target Link-Layer Address option.
    pub target_ll: Option<MacAddr>,
}

/// A validated ICMPv6 message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icmpv6Message<'a> {
    /// Echo Request or Reply.
    Echo(Echo<'a>),
    /// Router Solicitation.
    RouterSolicit(RouterSolicit),
    /// Router Advertisement.
    RouterAdvert(RouterAdvert),
    /// Neighbor Solicitation.
    NeighborSolicit(NeighborSolicit),
    /// Neighbor Advertisement.
    NeighborAdvert(NeighborAdvert),
}

/// Walk an option list, handing `visit` each option's type and bytes. Every
/// length is validated before `visit` sees the option — even ones it will
/// ignore — so a zero length cannot loop and an overrun cannot be read.
fn for_each_option<'a>(
    mut rest: &'a [u8],
    mut visit: impl FnMut(u8, &'a [u8]) -> Result<(), Icmpv6Error>,
) -> Result<(), Icmpv6Error> {
    while let Some(&[kind, units]) = rest.first_chunk::<2>() {
        if units == 0 {
            return Err(Icmpv6Error::OptionZeroLength);
        }
        let split = rest.split_at_checked(usize::from(units) * 8);
        let (opt, tail) = split.ok_or(Icmpv6Error::OptionOverrun)?;
        visit(kind, opt)?;
        rest = tail;
    }
    // One stray byte cannot even hold a type and a length.
    match rest {
        [] => Ok(()),
        _ => Err(Icmpv6Error::OptionOverrun),
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), Icmpv6Error> {
    match slot.replace(value) {
        Some(_) => Err(Icmpv6Error::DuplicateOption),
        None => Ok(()),
    }
}

fn check_mac(mac: MacAddr) -> Result<MacAddr, Icmpv6Error> {
    if mac.is_multicast() || mac.is_zero() {
        return Err(Icmpv6Error::BadLinkLayerAddress);
    }
    Ok(mac)
}

/// A link-layer address option: on Ethernet exactly one 8-byte unit of type,
/// length and MAC (RFC 4861 §4.6.1, RFC 2464 §8).
fn ll_option(opt: &[u8]) -> Result<MacAddr, Icmpv6Error> {
    let o: &[u8; 8] = opt.try_into().map_err(|_| Icmpv6Error::BadOption)?;
    check_mac(MacAddr([o[2], o[3], o[4], o[5], o[6], o[7]]))
}

/// The one link-layer option of type `want` in `options`, if present.
fn ll_from_options(options: &[u8], want: u8) -> Result<Option<MacAddr>, Icmpv6Error> {
    let mut found = None;
    for_each_option(options, |t, opt| {
        if t == want {
            set_once(&mut found, ll_option(opt)?)
        } else {
            Ok(())
        }
    })?;
    Ok(found)
}

fn prefix_option(opt: &[u8]) -> Result<PrefixInfo, Icmpv6Error> {
    let p: &[u8; 32] = opt.try_into().map_err(|_| Icmpv6Error::BadOption)?;
    if p[2] > 128 {
        return Err(Icmpv6Error::BadOption);
    }
    Ok(PrefixInfo {
        prefix_len: p[2],
        on_link: p[3] & 0x80 != 0,
        autonomous: p[3] & 0x40 != 0,
        valid_lifetime: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
        preferred_lifetime: u32::from_be_bytes([p[8], p[9], p[10], p[11]]),
        prefix: addr_at(p, 16), // p[12..16] is reserved and ignored
    })
}

/// RFC 4861 §7.1.1's NS rules, shared by parser and builder.
fn check_ns(src: Ipv6Addr, dst: Ipv6Addr, target: Ipv6Addr, sll: bool) -> Result<(), Icmpv6Error> {
    if target.is_multicast() {
        return Err(Icmpv6Error::MulticastTarget);
    }
    if src.is_unspecified() && sll {
        return Err(Icmpv6Error::UnspecifiedSourceWithLinkLayer);
    }
    if src.is_unspecified() && dst != target.solicited_node() {
        return Err(Icmpv6Error::DadNotSolicitedNode);
    }
    Ok(())
}

/// RFC 4861 §7.1.2's NA rules, shared by parser and builder.
fn check_na(dst: Ipv6Addr, target: Ipv6Addr, solicited: bool) -> Result<(), Icmpv6Error> {
    if target.is_multicast() {
        return Err(Icmpv6Error::MulticastTarget);
    }
    if solicited && dst.is_multicast() {
        return Err(Icmpv6Error::SolicitedToMulticast);
    }
    Ok(())
}

/// The checks every NDP type shares — hop limit, length, code — returning
/// the `N`-byte fixed part and the option bytes after it.
fn ndp_split<'a, const N: usize>(
    p: &Ipv6Packet<'a>,
) -> Result<(&'a [u8; N], &'a [u8]), Icmpv6Error> {
    if p.hop_limit != NDP_HOP_LIMIT {
        return Err(Icmpv6Error::NdpHopLimit);
    }
    let (fixed, options) = p
        .payload
        .split_first_chunk::<N>()
        .ok_or(Icmpv6Error::TooShort)?;
    if fixed.get(1) != Some(&0) {
        return Err(Icmpv6Error::BadCode);
    }
    Ok((fixed, options))
}

/// Parse and validate the ICMPv6 message `packet` carries. It takes the
/// packet, not its payload, because the checksum needs the addresses and NDP
/// validity depends on hop limit, source and destination — passed separately
/// they could be mismatched.
pub fn parse_icmp<'a>(packet: &Ipv6Packet<'a>) -> Result<Icmpv6Message<'a>, Icmpv6Error> {
    if packet.next_header != next_header::ICMPV6 {
        return Err(Icmpv6Error::NotIcmpv6);
    }
    let msg = packet.payload;
    let Some(&[kind, code, _, _]) = msg.first_chunk::<4>() else {
        return Err(Icmpv6Error::TooShort);
    };
    // First: a corrupt message's type is not worth interpreting, and the
    // pseudo-header proves the addresses were not rewritten.
    if !icmp_checksum_ok(packet.src, packet.dst, msg) {
        return Err(Icmpv6Error::BadChecksum);
    }
    let (src, dst) = (packet.src, packet.dst);
    match kind {
        icmp_types::ECHO_REQUEST | icmp_types::ECHO_REPLY => {
            let (h, payload) = msg.split_first_chunk::<8>().ok_or(Icmpv6Error::TooShort)?;
            if code != 0 {
                return Err(Icmpv6Error::BadCode);
            }
            let kind = match kind {
                icmp_types::ECHO_REQUEST => EchoKind::Request,
                _ => EchoKind::Reply,
            };
            let identifier = u16::from_be_bytes([h[4], h[5]]);
            let sequence = u16::from_be_bytes([h[6], h[7]]);
            Ok(Icmpv6Message::Echo(Echo {
                kind,
                identifier,
                sequence,
                payload,
            }))
        }
        icmp_types::ROUTER_SOLICIT => {
            let (_, options) = ndp_split::<8>(packet)?;
            let source_ll = ll_from_options(options, nd_option::SOURCE_LL)?;
            if src.is_unspecified() && source_ll.is_some() {
                return Err(Icmpv6Error::UnspecifiedSourceWithLinkLayer);
            }
            Ok(Icmpv6Message::RouterSolicit(RouterSolicit { source_ll }))
        }
        icmp_types::ROUTER_ADVERT => parse_router_advert(packet),
        icmp_types::NEIGHBOR_SOLICIT => {
            let (h, options) = ndp_split::<24>(packet)?;
            let target = addr_at(h, 8);
            let source_ll = ll_from_options(options, nd_option::SOURCE_LL)?;
            check_ns(src, dst, target, source_ll.is_some())?;
            let ns = NeighborSolicit { target, source_ll };
            Ok(Icmpv6Message::NeighborSolicit(ns))
        }
        icmp_types::NEIGHBOR_ADVERT => {
            let (h, options) = ndp_split::<24>(packet)?;
            let na = NeighborAdvert {
                router: h[4] & 0x80 != 0,
                solicited: h[4] & 0x40 != 0,
                overrides: h[4] & 0x20 != 0,
                target: addr_at(h, 8),
                target_ll: ll_from_options(options, nd_option::TARGET_LL)?,
            };
            check_na(dst, na.target, na.solicited)?;
            Ok(Icmpv6Message::NeighborAdvert(na))
        }
        other => Err(Icmpv6Error::UnsupportedType(other)),
    }
}

fn parse_router_advert<'a>(packet: &Ipv6Packet<'a>) -> Result<Icmpv6Message<'a>, Icmpv6Error> {
    let (h, options) = ndp_split::<16>(packet)?;
    if !packet.src.is_link_local() {
        return Err(Icmpv6Error::RouterNotLinkLocal);
    }
    let mut ra = RouterAdvert {
        cur_hop_limit: h[4],
        managed: h[5] & 0x80 != 0,
        other_config: h[5] & 0x40 != 0,
        router_lifetime: u16::from_be_bytes([h[6], h[7]]),
        reachable_time: u32::from_be_bytes([h[8], h[9], h[10], h[11]]),
        retrans_timer: u32::from_be_bytes([h[12], h[13], h[14], h[15]]),
        source_ll: None,
        mtu: None,
        prefixes: [None; MAX_PREFIXES],
        excess_prefixes: 0,
    };
    for_each_option(options, |t, opt| match t {
        nd_option::SOURCE_LL => set_once(&mut ra.source_ll, ll_option(opt)?),
        nd_option::MTU => {
            let o: &[u8; 8] = opt.try_into().map_err(|_| Icmpv6Error::BadOption)?;
            set_once(&mut ra.mtu, u32::from_be_bytes([o[4], o[5], o[6], o[7]]))
        }
        nd_option::PREFIX_INFO => {
            let info = prefix_option(opt)?;
            match ra.prefixes.iter_mut().find(|slot| slot.is_none()) {
                Some(slot) => *slot = Some(info),
                None => ra.excess_prefixes = ra.excess_prefixes.saturating_add(1),
            }
            Ok(())
        }
        _ => Ok(()),
    })?;
    Ok(Icmpv6Message::RouterAdvert(ra))
}

/// Checksum a laid-out ICMPv6 message in place and return its length.
fn seal(out: &mut [u8], src: Ipv6Addr, dst: Ipv6Addr) -> Result<usize, Icmpv6Error> {
    let at = ICMP_CHECKSUM_OFFSET;
    let sum = transport_checksum(src, dst, next_header::ICMPV6, out, at, &[]);
    let sum = sum.ok_or(Icmpv6Error::PayloadTooLarge)?;
    out[at..at + 2].copy_from_slice(&sum.to_be_bytes());
    Ok(out.len())
}

/// Lay out an NDP message: type `kind`, a `fixed`-byte part whose bytes 4..
/// `body` fills, then an optional link-layer option `(type, mac)`.
fn build_ndp(
    buf: &mut [u8],
    (src, dst): (Ipv6Addr, Ipv6Addr),
    kind: u8,
    fixed: usize,
    ll: Option<(u8, MacAddr)>,
    body: impl FnOnce(&mut [u8]),
) -> Result<usize, Icmpv6Error> {
    let ll = ll
        .map(|(t, mac)| check_mac(mac).map(|m| (t, m)))
        .transpose()?;
    let total = fixed + if ll.is_some() { 8 } else { 0 };
    let out = buf.get_mut(..total).ok_or(Icmpv6Error::BufferTooSmall)?;
    out.fill(0);
    out[0] = kind;
    body(&mut out[4..fixed]);
    if let Some((t, mac)) = ll {
        out[fixed..fixed + 2].copy_from_slice(&[t, 1]);
        out[fixed + 2..].copy_from_slice(&mac.0);
    }
    seal(out, src, dst)
}

/// Build an Echo Request or Reply into `buf`, returning its length; wrap it
/// with [`build_packet`] using [`next_header::ICMPV6`].
pub fn build_echo(
    buf: &mut [u8],
    src: Ipv6Addr,
    dst: Ipv6Addr,
    kind: EchoKind,
    identifier: u16,
    sequence: u16,
    payload: &[u8],
) -> Result<usize, Icmpv6Error> {
    if payload.len() > usize::from(u16::MAX) - ECHO_HEADER_LEN {
        return Err(Icmpv6Error::PayloadTooLarge);
    }
    let total = ECHO_HEADER_LEN + payload.len();
    let out = buf.get_mut(..total).ok_or(Icmpv6Error::BufferTooSmall)?;
    out[0] = match kind {
        EchoKind::Request => icmp_types::ECHO_REQUEST,
        EchoKind::Reply => icmp_types::ECHO_REPLY,
    };
    out[1..4].fill(0); // code 0; checksum computed over a zeroed field
    out[4..6].copy_from_slice(&identifier.to_be_bytes());
    out[6..8].copy_from_slice(&sequence.to_be_bytes());
    out[ECHO_HEADER_LEN..].copy_from_slice(payload);
    seal(out, src, dst)
}

/// Build a Router Solicitation, normally to [`Ipv6Addr::ALL_ROUTERS`]. From
/// `::` (no address yet) `source_ll` must be `None`. Send at hop limit
/// [`NDP_HOP_LIMIT`], as for every NDP builder.
pub fn build_router_solicit(
    buf: &mut [u8],
    src: Ipv6Addr,
    dst: Ipv6Addr,
    source_ll: Option<MacAddr>,
) -> Result<usize, Icmpv6Error> {
    if src.is_unspecified() && source_ll.is_some() {
        return Err(Icmpv6Error::UnspecifiedSourceWithLinkLayer);
    }
    let ll = source_ll.map(|mac| (nd_option::SOURCE_LL, mac));
    build_ndp(buf, (src, dst), icmp_types::ROUTER_SOLICIT, 8, ll, |_| {})
}

/// Build a Neighbor Solicitation for `target` — to its solicited-node group
/// to resolve it, to `target` itself to confirm reachability, or from `::`
/// without `source_ll` for duplicate address detection. Refuses exactly what
/// [`parse_icmp`] would.
pub fn build_neighbor_solicit(
    buf: &mut [u8],
    src: Ipv6Addr,
    dst: Ipv6Addr,
    target: Ipv6Addr,
    source_ll: Option<MacAddr>,
) -> Result<usize, Icmpv6Error> {
    check_ns(src, dst, target, source_ll.is_some())?;
    let ll = source_ll.map(|mac| (nd_option::SOURCE_LL, mac));
    build_ndp(buf, (src, dst), icmp_types::NEIGHBOR_SOLICIT, 24, ll, |b| {
        b[4..20].copy_from_slice(&target.0)
    })
}

/// Build a Neighbor Advertisement from `na`. Refuses exactly what
/// [`parse_icmp`] would.
pub fn build_neighbor_advert(
    buf: &mut [u8],
    src: Ipv6Addr,
    dst: Ipv6Addr,
    na: &NeighborAdvert,
) -> Result<usize, Icmpv6Error> {
    check_na(dst, na.target, na.solicited)?;
    let ll = na.target_ll.map(|mac| (nd_option::TARGET_LL, mac));
    build_ndp(buf, (src, dst), icmp_types::NEIGHBOR_ADVERT, 24, ll, |b| {
        b[0] = (u8::from(na.router) << 7)
            | (u8::from(na.solicited) << 6)
            | (u8::from(na.overrides) << 5);
        b[4..20].copy_from_slice(&na.target.0);
    })
}

#[cfg(test)]
mod tests {
    use super::{Icmpv6Error as E, Icmpv6Message as M, *};

    const MAC: MacAddr = MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    /// libslirp derives its router's MAC from `fe80::2`: `52:56` + low 32 bits.
    const ROUTER_MAC: MacAddr = MacAddr([0x52, 0x56, 0, 0, 0, 2]);
    const UNSPEC: Ipv6Addr = Ipv6Addr::UNSPECIFIED;
    const ALL_NODES: Ipv6Addr = Ipv6Addr::ALL_NODES;
    const ALL_ROUTERS: Ipv6Addr = Ipv6Addr::ALL_ROUTERS;

    fn addr(s: &str) -> Ipv6Addr {
        Ipv6Addr(s.parse::<std::net::Ipv6Addr>().unwrap().octets())
    }

    fn text(a: Ipv6Addr) -> String {
        a.format(&mut [0u8; 39]).to_string()
    }

    fn rng(mut x: u64) -> impl FnMut() -> u64 {
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        }
    }

    fn wrap(src: Ipv6Addr, dst: Ipv6Addr, hop: u8, msg: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; HEADER_LEN + msg.len()];
        build_packet(&mut buf, src, dst, hop, 58, msg).unwrap();
        buf
    }

    /// `msg` with a correct checksum at hop limit 255, so the fault under
    /// test is the only one — for inputs the builders refuse to produce.
    fn sealed(src: Ipv6Addr, dst: Ipv6Addr, mut msg: Vec<u8>) -> Vec<u8> {
        if msg.len() >= 4 {
            msg[2..4].fill(0);
            let sum = transport_checksum(src, dst, 58, &msg, 2, &[]).unwrap();
            msg[2..4].copy_from_slice(&sum.to_be_bytes());
        }
        wrap(src, dst, NDP_HOP_LIMIT, &msg)
    }

    /// Parse all the way up. The buffer is leaked so the borrowed result can
    /// outlive the statement — harmless in a test.
    fn icmp(raw: Vec<u8>) -> Result<Icmpv6Message<'static>, E> {
        parse_icmp(&parse(raw.leak()).unwrap())
    }

    /// An NS/NA-shaped message: type, flags byte, target, options.
    fn nd(kind: u8, flags: u8, target: Ipv6Addr, opts: &[u8]) -> Vec<u8> {
        [&[kind, 0, 0, 0, flags, 0, 0, 0][..], &target.0[..], opts].concat()
    }

    /// Independent RFC 1071 arithmetic — a plain wide sum folded at the end —
    /// to check the incremental implementation against. Checksum field zero.
    fn reference_checksum(src: Ipv6Addr, dst: Ipv6Addr, msg: &[u8]) -> u16 {
        let len = (msg.len() as u32).to_be_bytes();
        let mut b = [&src.0[..], &dst.0[..], &len[..], &[0, 0, 0, 58], msg].concat();
        if !b.len().is_multiple_of(2) {
            b.push(0);
        }
        let mut sum: u64 = b
            .chunks(2)
            .map(|w| (u64::from(w[0]) << 8) + u64::from(w[1]))
            .sum();
        while sum > 0xFFFF {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }

    #[test]
    fn formats_rfc5952_text_and_agrees_with_std() {
        for (input, want) in [
            ("::", "::"),
            ("::1", "::1"),
            ("fe80::1", "fe80::1"),
            ("2001:db8:0:0:1:0:0:1", "2001:db8::1:0:0:1"), // tie: first run
            ("2001:0:0:1:0:0:0:1", "2001:0:0:1::1"),       // longest run
            ("2001:db8:0:1:1:1:1:1", "2001:db8:0:1:1:1:1:1"), // lone zero
            ("2001:0DB8::0001", "2001:db8::1"),
        ] {
            assert_eq!(text(addr(input)), want, "{input}");
        }
        let full = "1111:2222:3333:4444:5555:6666:7777:8888";
        assert_eq!(text(addr(full)), full);
        assert_eq!(ALL_ROUTERS.to_string(), "ff02::2");
        // Every zero/non-zero pattern of the eight groups at every digit
        // count, then random addresses, against std's RFC 5952 formatter —
        // skipping IPv4-mapped ones, which std writes in mixed notation.
        let mut next = rng(0x9E37_79B9_7F4A_7C15);
        let patterns = (0u32..1024).map(|m| -> [u16; 8] {
            let fill = [0x1, 0xab, 0xabc, 0xffff][(m >> 8) as usize];
            core::array::from_fn(|g| if (m >> g) & 1 == 1 { fill } else { 0 })
        });
        let random = (0..5000).map(|_| {
            core::array::from_fn(|_| match next() % 5 {
                0 | 4 => 0,
                n => (next() as u16) >> (4 * (n - 1)),
            })
        });
        for groups in patterns.chain(random) {
            let std_addr = std::net::Ipv6Addr::from(groups);
            if std_addr.to_ipv4_mapped().is_none() {
                assert_eq!(text(Ipv6Addr(std_addr.octets())), std_addr.to_string());
            }
        }
    }

    #[test]
    fn eui64_solicited_node_and_multicast_mac() {
        let ll = Ipv6Addr::link_local_from_mac(MAC);
        assert_eq!(ll, addr("fe80::5054:ff:fe12:3456"));
        assert_eq!(text(ll), "fe80::5054:ff:fe12:3456");
        // The U/L flip goes both ways: a universal MAC gains the bit.
        let universal = Ipv6Addr::link_local_from_mac(MacAddr([0, 0x1B, 0x21, 1, 2, 3]));
        assert_eq!(text(universal), "fe80::21b:21ff:fe01:203");
        let slaac = Ipv6Addr::with_prefix([0x20, 1, 0xd, 0xb8, 0, 0, 0, 1], MAC);
        assert_eq!(slaac, addr("2001:db8:0:1:5054:ff:fe12:3456"));
        assert_eq!(ll.solicited_node(), addr("ff02::1:ff12:3456"));
        let group_mac = ll.solicited_node().multicast_mac();
        assert_eq!(group_mac, MacAddr([0x33, 0x33, 0xFF, 0x12, 0x34, 0x56]));
        assert_eq!(ALL_NODES.multicast_mac(), MacAddr([0x33, 0x33, 0, 0, 0, 1]));
        assert!(ll.is_link_local() && addr("febf::1").is_link_local());
        assert!(!addr("fec0::1").is_link_local() && !addr("fe7f::1").is_link_local());
        assert!(ALL_ROUTERS.is_multicast() && !ll.is_multicast());
        assert!(UNSPEC.is_unspecified() && !addr("::1").is_unspecified());
        assert_eq!(Ipv6Addr::from_slice(&ll.0), Some(ll));
        assert_eq!(Ipv6Addr::from_slice(&[0; 15]), None);
        assert_eq!(
            ll.segments(),
            [0xfe80, 0, 0, 0, 0x5054, 0xff, 0xfe12, 0x3456]
        );
    }

    #[test]
    fn header_round_trip_and_refusals() {
        let (src, dst) = (addr("2001:db8::1"), addr("2001:db8::2"));
        let mut raw = [0u8; 64];
        let n = build_packet(&mut raw, src, dst, 64, 17, &[1, 2, 3, 4, 5]).unwrap();
        // 64 bytes in, as Ethernet padding delivers them: the pad is trimmed.
        let p = parse(&raw).unwrap();
        assert_eq!(
            (p.src, p.dst, p.next_header, p.hop_limit),
            (src, dst, 17, 64)
        );
        assert_eq!(
            (p.traffic_class, p.flow_label, p.payload),
            (0, 0, &raw[40..n])
        );
        raw[..4].copy_from_slice(&[0x6A, 0xB1, 0x23, 0x45]);
        let p = parse(&raw[..n]).unwrap();
        assert_eq!((p.traffic_class, p.flow_label), (0xAB, 0x1_2345));
        for len in 0..n {
            let want = match len {
                0..HEADER_LEN => Ipv6Error::TooShort,
                _ => Ipv6Error::PayloadLengthExceedsBuffer,
            };
            assert_eq!(parse(&raw[..len]), Err(want), "len {len}");
        }
        let refused = |at: usize, v: u8| {
            let mut r = raw;
            r[at] = v;
            parse(&r[..n]).map(|_| ())
        };
        for v in [0x00, 0x40, 0x70, 0xF0] {
            assert_eq!(refused(0, v), Err(Ipv6Error::BadVersion));
        }
        assert_eq!(refused(4, 1), Err(Ipv6Error::PayloadLengthExceedsBuffer));
        assert_eq!(refused(8, 0xFF), Err(Ipv6Error::MulticastSource));
        // Extension headers and unknown protocols: reported, never walked,
        // and never emitted either.
        for nh in [0u8, 43, 44, 50, 51, 59, 60, 1, 4, 41, 255] {
            assert_eq!(refused(6, nh), Err(Ipv6Error::Unsupported(nh)));
            let built = build_packet(&mut [0; 64], src, dst, 64, nh, &[]);
            assert_eq!(built, Err(Ipv6Error::Unsupported(nh)));
        }
        let e = build_packet(&mut [0; 64], ALL_NODES, dst, 64, 17, &[]);
        assert_eq!(e, Err(Ipv6Error::MulticastSource));
        let e = build_packet(&mut [0; 44], src, dst, 64, 17, &[0; 5]);
        assert_eq!(e, Err(Ipv6Error::BufferTooSmall));
        let (mut out, big) = (vec![0u8; 70_000], vec![0u8; 65_536]);
        let e = build_packet(&mut out, src, dst, 64, 17, &big);
        assert_eq!(e, Err(Ipv6Error::PayloadTooLarge));
        assert!(build_packet(&mut out, src, dst, 64, 17, &big[1..]).is_ok());
    }

    #[test]
    fn echo_round_trip_checksum_and_corruption() {
        let (me, peer) = (addr("fec0::5054:ff:fe12:3456"), addr("fec0::2"));
        let ph = pseudo_header(me, peer, 58, 0x0102_0304);
        assert_eq!(
            ph[..],
            [&me.0[..], &peer.0[..], &[1, 2, 3, 4, 0, 0, 0, 58]].concat()
        );
        // Odd length, a stale checksum field to be read as zero, and a split
        // between header and payload, against the independent reference.
        let msg = [128u8, 0, 0xAA, 0xBB, 1, 2, 3, 4, 5];
        let sum = transport_checksum(me, peer, 58, &msg, 2, &[]);
        let zeroed = [128u8, 0, 0, 0, 1, 2, 3, 4, 5];
        assert_eq!(sum, Some(reference_checksum(me, peer, &zeroed)));
        assert_eq!(
            transport_checksum(me, peer, 58, &msg[..4], 2, &msg[4..]),
            sum
        );
        assert_ne!(transport_checksum(me, ALL_NODES, 58, &msg, 2, &[]), sum);
        let mut buf = [0u8; 64];
        let n = build_echo(&mut buf, me, peer, EchoKind::Request, 0xBEEF, 7, b"hi!").unwrap();
        let raw = wrap(me, peer, 64, &buf[..n]);
        let Ok(M::Echo(req)) = icmp(raw.clone()) else {
            panic!("request did not parse");
        };
        let fields = (req.kind, req.identifier, req.sequence, req.payload);
        assert_eq!(fields, (EchoKind::Request, 0xBEEF, 7, &b"hi!"[..]));
        let n = build_echo(&mut buf, peer, me, EchoKind::Reply, 0xBEEF, 7, b"hi!").unwrap();
        let Ok(M::Echo(reply)) = icmp(wrap(peer, me, 64, &buf[..n])) else {
            panic!("reply did not parse");
        };
        assert!(reply.answers(0xBEEF, 7) && !reply.answers(0xBEEF, 8));
        assert!(!req.answers(0xBEEF, 7), "a request never answers");
        // The checksum is verified before anything is interpreted, so every
        // single-bit flip of either address or of the message is refused as
        // exactly that (a source flipped to multicast fails a layer earlier).
        for i in 8..raw.len() {
            for bit in 0..8 {
                let mut c = raw.clone();
                c[i] ^= 1 << bit;
                if parse(&c).is_ok() {
                    assert_eq!(icmp(c), Err(E::BadChecksum), "byte {i} bit {bit}");
                }
            }
        }
        let mut m = buf[..n].to_vec();
        m[1] = 1;
        assert_eq!(icmp(sealed(peer, me, m)), Err(E::BadCode));
        assert_eq!(
            icmp(sealed(peer, me, vec![129, 0, 0, 0, 0])),
            Err(E::TooShort)
        );
        assert_eq!(icmp(wrap(peer, me, 64, &[129, 0, 0])), Err(E::TooShort));
        let e = build_echo(&mut [0u8; 8], me, peer, EchoKind::Request, 1, 1, b"x");
        assert_eq!(e, Err(E::BufferTooSmall));
        let (mut out, big) = (vec![0u8; 70_000], vec![0u8; 65_528]);
        let e = build_echo(&mut out, me, peer, EchoKind::Reply, 1, 1, &big);
        assert_eq!(e, Err(E::PayloadTooLarge));
        assert!(build_echo(&mut out, me, peer, EchoKind::Reply, 1, 1, &big[1..]).is_ok());
    }

    /// Shaped like QEMU user-mode networking's RA (libslirp `ndp_send_ra`):
    /// router `fe80::2` to all-nodes, hop limit 64, lifetime 1800 s, M/O
    /// clear, its link-layer address, prefix `fec0::/64` with L+A (valid
    /// 86400 s, preferred 14400 s), and an RDNSS option to be skipped.
    fn qemu_ra() -> Vec<u8> {
        let mut m = vec![134, 0, 0, 0, 64, 0, 0x07, 0x08, 0, 0, 0, 0, 0, 0, 0, 0];
        m.extend([1, 1, 0x52, 0x56, 0, 0, 0, 2]);
        m.extend([
            3, 4, 64, 0xC0, 0, 1, 0x51, 0x80, 0, 0, 0x38, 0x40, 0, 0, 0, 0,
        ]);
        m.extend(addr("fec0::").0);
        m.extend([25, 3, 0, 0, 0, 0, 0x04, 0xB0]);
        m.extend(addr("fec0::3").0);
        m
    }

    #[test]
    fn parses_a_qemu_style_router_advert_exactly() {
        let (router, mut msg) = (addr("fe80::2"), qemu_ra());
        let sum = reference_checksum(router, ALL_NODES, &msg);
        assert_eq!(
            transport_checksum(router, ALL_NODES, 58, &msg, 2, &[]),
            Some(sum)
        );
        msg[2..4].copy_from_slice(&sum.to_be_bytes());
        let fec0 = PrefixInfo {
            prefix_len: 64,
            on_link: true,
            autonomous: true,
            valid_lifetime: 86_400,
            preferred_lifetime: 14_400,
            prefix: addr("fec0::"),
        };
        let want = RouterAdvert {
            cur_hop_limit: 64,
            managed: false,
            other_config: false,
            router_lifetime: 1800,
            reachable_time: 0,
            retrans_timer: 0,
            source_ll: Some(ROUTER_MAC),
            mtu: None,
            prefixes: [Some(fec0), None, None, None],
            excess_prefixes: 0,
        };
        let raw = wrap(router, ALL_NODES, 255, &msg);
        assert_eq!(icmp(raw), Ok(M::RouterAdvert(want)));
        let slaac = Ipv6Addr::with_prefix(fec0.slaac_prefix().unwrap(), MAC);
        assert_eq!(text(slaac), "fec0::5054:ff:fe12:3456");
        // Every truncation, re-checksummed so length is the only fault: short
        // of the fixed part, a clean option boundary, or a cut option.
        for n in 0..msg.len() {
            let got = icmp(sealed(router, ALL_NODES, msg[..n].to_vec()));
            match n {
                0..16 => assert_eq!(got, Err(E::TooShort), "len {n}"),
                16 | 24 | 56 => assert!(got.is_ok(), "len {n}"),
                _ => assert_eq!(got, Err(E::OptionOverrun), "len {n}"),
            }
        }
    }

    #[test]
    fn router_advert_limits_and_refusals() {
        let (r, fixed) = (addr("fe80::2"), qemu_ra()[..16].to_vec());
        let ra = |opts: &[u8]| icmp(sealed(r, ALL_NODES, [&fixed[..], opts].concat()));
        let prefix = |len: u8| {
            let head = [3, 4, len, 0xC0, 0, 0, 0, 9, 0, 0, 0, 9, 0, 0, 0, 0];
            [&head[..], &addr("2001:db8::").0[..]].concat()
        };
        let mut opts = vec![5, 1, 0, 0, 0, 0, 0x05, 0xDC];
        for _ in 0..=MAX_PREFIXES {
            opts.extend(prefix(64));
        }
        let Ok(M::RouterAdvert(got)) = ra(&opts) else {
            panic!("RA with an MTU and five prefixes did not parse");
        };
        assert_eq!(got.mtu, Some(1500));
        assert_eq!(got.prefixes.iter().flatten().count(), MAX_PREFIXES);
        assert_eq!(got.excess_prefixes, 1, "the fifth is counted, not lost");
        let sll = [1, 1, 0x52, 0x56, 0, 0, 0, 2];
        #[rustfmt::skip]
        let cases = [
            ([&sll[..], &sll[..]].concat(), E::DuplicateOption),
            (vec![5, 1, 0, 0, 0, 0, 5, 0xDC, 5, 1, 0, 0, 0, 0, 5, 0xDC], E::DuplicateOption),
            (vec![5, 2, 0, 0, 0, 0, 5, 0xDC, 0, 0, 0, 0, 0, 0, 0, 0], E::BadOption),
            (vec![1, 2, 0x52, 0x56, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0], E::BadOption),
            (prefix(129), E::BadOption),
            (vec![1, 1, 0x33, 0x33, 0, 0, 0, 1], E::BadLinkLayerAddress),
            (vec![1, 1, 0, 0, 0, 0, 0, 0], E::BadLinkLayerAddress),
            (vec![25, 0, 0, 0, 0, 0, 0, 0], E::OptionZeroLength),
            (vec![25, 3, 0, 0, 0, 0, 0, 0], E::OptionOverrun),
        ];
        for (opts, err) in cases {
            assert_eq!(ra(&opts), Err(err), "{opts:?}");
        }
        let global = addr("2001:db8::1");
        assert_eq!(
            icmp(sealed(global, ALL_NODES, fixed.clone())),
            Err(E::RouterNotLinkLocal)
        );
    }

    #[test]
    fn neighbor_discovery_round_trips() {
        let (me, peer) = (Ipv6Addr::link_local_from_mac(MAC), addr("fe80::2"));
        let (snm, my_snm) = (peer.solicited_node(), me.solicited_node());
        let mut buf = [0u8; 64];
        // Address resolution: to the target's solicited-node group, our MAC.
        let n = build_neighbor_solicit(&mut buf, me, snm, peer, Some(MAC)).unwrap();
        assert_eq!(
            (buf[0], buf[1], &buf[4..8], &buf[8..24]),
            (135, 0, &[0; 4][..], &peer.0[..])
        );
        assert_eq!(buf[24..n], [1, 1, 0x52, 0x54, 0, 0x12, 0x34, 0x56]);
        let ns = NeighborSolicit {
            target: peer,
            source_ll: Some(MAC),
        };
        assert_eq!(
            icmp(wrap(me, snm, 255, &buf[..n])),
            Ok(M::NeighborSolicit(ns))
        );
        // Duplicate address detection: from `::`, no link-layer option.
        let n = build_neighbor_solicit(&mut buf, UNSPEC, my_snm, me, None).unwrap();
        let dad = NeighborSolicit {
            target: me,
            source_ll: None,
        };
        assert_eq!(
            icmp(wrap(UNSPEC, my_snm, 255, &buf[..n])),
            Ok(M::NeighborSolicit(dad))
        );
        // A solicited unicast answer, and an unsolicited override to all-nodes.
        let answer = NeighborAdvert {
            router: false,
            solicited: true,
            overrides: true,
            target: me,
            target_ll: Some(MAC),
        };
        let announce = NeighborAdvert {
            router: true,
            solicited: false,
            ..answer
        };
        for (dst, na, flags) in [(peer, answer, 0x60), (ALL_NODES, announce, 0xA0)] {
            let n = build_neighbor_advert(&mut buf, me, dst, &na).unwrap();
            assert_eq!(buf[4], flags, "R, S, O are the top three bits");
            assert_eq!(
                icmp(wrap(me, dst, 255, &buf[..n])),
                Ok(M::NeighborAdvert(na))
            );
        }
        for (src, source_ll) in [(me, Some(MAC)), (UNSPEC, None)] {
            let n = build_router_solicit(&mut buf, src, ALL_ROUTERS, source_ll).unwrap();
            let rs = M::RouterSolicit(RouterSolicit { source_ll });
            assert_eq!(icmp(wrap(src, ALL_ROUTERS, 255, &buf[..n])), Ok(rs));
        }
    }

    #[test]
    fn ndp_validity_refusals() {
        let (me, peer) = (Ipv6Addr::link_local_from_mac(MAC), addr("fe80::2"));
        let (snm, my_snm) = (peer.solicited_node(), me.solicited_node());
        let sll = [1u8, 1, 0x52, 0x54, 0, 0x12, 0x34, 0x56];
        let rs_sll = [&[133u8, 0, 0, 0, 0, 0, 0, 0][..], &sll[..]].concat();
        let mut bad_code = nd(135, 0, peer, &[]);
        bad_code[1] = 1;
        #[rustfmt::skip]
        let cases = [
            (me, snm, nd(135, 0, peer, &[1, 0, 0, 0, 0, 0, 0, 0]), E::OptionZeroLength),
            (me, snm, nd(135, 0, peer, &[1, 2, 0x52, 0x54, 0, 0x12, 0x34, 0x56]), E::OptionOverrun),
            (me, snm, nd(135, 0, peer, &[1]), E::OptionOverrun),
            (me, snm, nd(135, 0, ALL_NODES, &[]), E::MulticastTarget),
            (me, peer, nd(136, 0, ALL_NODES, &[]), E::MulticastTarget),
            (UNSPEC, my_snm, nd(135, 0, me, &sll), E::UnspecifiedSourceWithLinkLayer),
            (UNSPEC, ALL_NODES, nd(135, 0, me, &[]), E::DadNotSolicitedNode),
            (UNSPEC, snm, nd(135, 0, me, &[]), E::DadNotSolicitedNode),
            (me, ALL_NODES, nd(136, 0x40, me, &[]), E::SolicitedToMulticast),
            (UNSPEC, ALL_ROUTERS, rs_sll.clone(), E::UnspecifiedSourceWithLinkLayer),
            (me, snm, bad_code, E::BadCode),
            (me, snm, nd(135, 0, peer, &[])[..20].to_vec(), E::TooShort),
            (me, peer, vec![137, 0, 0, 0, 0, 0, 0, 0], E::UnsupportedType(137)),
            (me, peer, vec![1, 0, 0, 0, 0, 0, 0, 0], E::UnsupportedType(1)),
            (me, peer, vec![130, 0, 0, 0, 0, 0, 0, 0], E::UnsupportedType(130)),
        ];
        for (src, dst, msg, err) in cases {
            assert_eq!(icmp(sealed(src, dst, msg.clone())), Err(err), "{msg:?}");
        }
        // Each NDP type, otherwise valid, is refused once forwarded. The hop
        // limit is outside the pseudo-header, so the checksum still holds.
        for (src, dst, msg) in [
            (me, snm, nd(135, 0, peer, &sll)),
            (me, peer, nd(136, 0x60, me, &[])),
            (peer, ALL_NODES, qemu_ra()),
            (me, ALL_ROUTERS, rs_sll),
        ] {
            let mut raw = sealed(src, dst, msg);
            assert!(icmp(raw.clone()).is_ok());
            for hop in [0, 1, 64, 254] {
                raw[7] = hop;
                assert_eq!(icmp(raw.clone()), Err(E::NdpHopLimit), "hop {hop}");
            }
        }
        // The builders run the parser's own checks, so refuse the same.
        let mut buf = [0u8; 64];
        let e = build_neighbor_solicit(&mut buf, me, snm, ALL_NODES, None);
        assert_eq!(e, Err(E::MulticastTarget));
        let e = build_neighbor_solicit(&mut buf, UNSPEC, ALL_NODES, me, None);
        assert_eq!(e, Err(E::DadNotSolicitedNode));
        let na = NeighborAdvert {
            router: false,
            solicited: true,
            overrides: false,
            target: me,
            target_ll: None,
        };
        let e = build_neighbor_advert(&mut buf, me, ALL_NODES, &na);
        assert_eq!(e, Err(E::SolicitedToMulticast));
        let to_group = NeighborAdvert {
            target: ALL_NODES,
            ..na
        };
        let e = build_neighbor_advert(&mut buf, me, peer, &to_group);
        assert_eq!(e, Err(E::MulticastTarget));
        let e = build_router_solicit(&mut buf, UNSPEC, ALL_ROUTERS, Some(MAC));
        assert_eq!(e, Err(E::UnspecifiedSourceWithLinkLayer));
        let e = build_router_solicit(&mut buf, me, ALL_ROUTERS, Some(MacAddr::BROADCAST));
        assert_eq!(e, Err(E::BadLinkLayerAddress));
        let e = build_neighbor_solicit(&mut [0u8; 31], me, snm, peer, Some(MAC));
        assert_eq!(e, Err(E::BufferTooSmall));
        let mut udp = [0u8; 48];
        build_packet(&mut udp, me, peer, 64, next_header::UDP, &[0; 8]).unwrap();
        assert_eq!(icmp(udp.to_vec()), Err(E::NotIcmpv6));
    }

    #[test]
    fn slaac_prefix_follows_rfc4862() {
        let base = PrefixInfo {
            prefix_len: 64,
            on_link: true,
            autonomous: true,
            valid_lifetime: 600,
            preferred_lifetime: 300,
            prefix: addr("2001:db8:1:2::"),
        };
        assert_eq!(base.slaac_prefix(), Some([0x20, 1, 0xd, 0xb8, 0, 1, 0, 2]));
        let spoilers: [fn(&mut PrefixInfo); 6] = [
            |p| p.autonomous = false,
            |p| p.prefix_len = 48,
            |p| p.prefix = addr("fe80::"),
            |p| p.prefix = addr("ff02::"),
            |p| (p.valid_lifetime, p.preferred_lifetime) = (0, 0),
            |p| p.preferred_lifetime = 601,
        ];
        for spoil in spoilers {
            let mut p = base;
            spoil(&mut p);
            assert_eq!(p.slaac_prefix(), None, "{p:?}");
        }
    }

    /// Random bytes shaped just enough to reach every parser path (valid
    /// checksum, mostly code 0, option headers with small lengths) must yield
    /// an error or a value — never a panic or an out-of-bounds read.
    #[test]
    fn hostile_input_never_panics() {
        let mut next = rng(0x0123_4567_89AB_CDEF);
        let types = [128, 129, 133, 134, 135, 136, 1, 137];
        let (ll, snm) = (addr("fe80::1"), addr("ff02::1:ff00:1"));
        for _ in 0..20_000 {
            let len = (next() % 96) as usize;
            let mut msg: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if len >= 2 {
                msg[0] = types[(next() % 8) as usize];
                msg[1] = u8::from(next().is_multiple_of(8));
            }
            for i in (8..len.saturating_sub(1)).step_by(8) {
                msg[i] = [1, 2, 3, 5, 25][(next() % 5) as usize];
                msg[i + 1] = (next() % 5) as u8;
            }
            let src = [ll, UNSPEC, addr("2001:db8::1")][(next() % 3) as usize];
            let dst = [ALL_NODES, snm, ll][(next() % 3) as usize];
            let mut raw = sealed(src, dst, msg);
            let _ = parse_icmp(&parse(&raw).unwrap());
            raw[4..6].copy_from_slice(&(next() as u16).to_be_bytes());
            if let Ok(p) = parse(&raw) {
                let _ = parse_icmp(&p);
            }
        }
    }
}
