//! IPv4 header parsing and construction (RFC 791), strict by design.
//!
//! Two decisions shape this module:
//!
//! * **Fragments are refused, not reassembled.** A reassembly engine needs a
//!   hole list, per-flow timers and an eviction policy — i.e. attacker-driven
//!   state, which is exactly the class of bug (teardrop, overlapping
//!   fragments) that has repeatedly broken far more mature stacks. Refusing
//!   with a distinct [`Ipv4Error::Fragmented`] means a caller can count and
//!   log the condition instead of silently handing a truncated first fragment
//!   to UDP as if it were whole.
//! * **Options are parsed, never honoured.** Source routing and friends are
//!   security-relevant; we locate the option bytes so the payload offset is
//!   right and expose them for inspection, but we act on none of them.

use super::checksum;

/// A 32-bit IPv4 address in network byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    /// `0.0.0.0` — "unspecified"; legal as a DHCP source, never as a peer.
    pub const UNSPECIFIED: Ipv4Addr = Ipv4Addr([0, 0, 0, 0]);
    /// `255.255.255.255` — limited broadcast.
    pub const BROADCAST: Ipv4Addr = Ipv4Addr([255, 255, 255, 255]);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr([a, b, c, d])
    }

    pub fn from_slice(bytes: &[u8]) -> Option<Ipv4Addr> {
        let arr: [u8; 4] = bytes.try_into().ok()?;
        Some(Ipv4Addr(arr))
    }

    pub const fn octets(self) -> [u8; 4] {
        self.0
    }

    pub const fn to_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    pub const fn from_u32(v: u32) -> Ipv4Addr {
        Ipv4Addr(v.to_be_bytes())
    }

    pub fn is_unspecified(self) -> bool {
        self.0 == [0, 0, 0, 0]
    }

    pub fn is_broadcast(self) -> bool {
        self.0 == [255, 255, 255, 255]
    }

    /// 224.0.0.0/4.
    pub fn is_multicast(self) -> bool {
        self.0[0] & 0xF0 == 0xE0
    }

    /// 127.0.0.0/8.
    pub fn is_loopback(self) -> bool {
        self.0[0] == 127
    }

    /// True when `self` and `other` share `mask`'s network part.
    pub fn same_subnet(self, other: Ipv4Addr, mask: Ipv4Addr) -> bool {
        self.to_u32() & mask.to_u32() == other.to_u32() & mask.to_u32()
    }

    /// The all-ones directed broadcast for `mask`'s subnet.
    pub fn subnet_broadcast(self, mask: Ipv4Addr) -> Ipv4Addr {
        Ipv4Addr::from_u32((self.to_u32() & mask.to_u32()) | !mask.to_u32())
    }

    /// Render as dotted decimal into a caller-supplied buffer (max
    /// `255.255.255.255` = 15 bytes) and return the borrowed `&str`.
    pub fn format(self, out: &mut [u8; 15]) -> &str {
        let mut n = 0usize;
        for (i, &octet) in self.0.iter().enumerate() {
            if i > 0 {
                out[n] = b'.';
                n += 1;
            }
            let mut value = octet;
            if value >= 100 {
                out[n] = b'0' + value / 100;
                n += 1;
                value %= 100;
                out[n] = b'0' + value / 10;
                n += 1;
            } else if octet >= 10 {
                out[n] = b'0' + value / 10;
                n += 1;
                value %= 10;
            }
            if octet >= 100 {
                value %= 10;
            }
            out[n] = b'0' + value % 10;
            n += 1;
        }
        core::str::from_utf8(&out[..n]).unwrap_or("0.0.0.0")
    }
}

impl core::fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut buf = [0u8; 15];
        f.write_str(self.format(&mut buf))
    }
}

/// IP protocol numbers this stack routes to a handler.
pub mod proto {
    pub const ICMP: u8 = 1;
    pub const TCP: u8 = 6;
    pub const UDP: u8 = 17;
}

/// Minimum (option-free) IPv4 header length in bytes.
pub const MIN_HEADER_LEN: usize = 20;
/// Maximum IPv4 header length: IHL is four bits, in 32-bit words.
pub const MAX_HEADER_LEN: usize = 60;
/// Byte offset of the header checksum inside the header.
pub const CHECKSUM_OFFSET: usize = 10;

/// Fragment-flag bits as they appear in the combined flags/offset word.
const FLAG_DONT_FRAGMENT: u8 = 0b010;
const FLAG_MORE_FRAGMENTS: u8 = 0b001;

/// Every way a received IPv4 packet can be refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv4Error {
    /// Fewer than 20 bytes available.
    TooShort,
    /// Version nibble is not 4.
    BadVersion,
    /// IHL < 5: the header claims to be shorter than the fixed part.
    BadHeaderLength,
    /// IHL is plausible but the buffer does not contain that many bytes.
    HeaderTruncated,
    /// `total_length` is smaller than the header it follows.
    TotalLengthTooSmall,
    /// `total_length` claims more bytes than the frame actually carries. This
    /// is the classic "lie about the length to read past the buffer" input.
    TotalLengthExceedsBuffer,
    /// Header checksum does not verify.
    BadChecksum,
    /// MF set or a non-zero fragment offset. Deliberately not reassembled.
    Fragmented,
    /// TTL reached zero. Refused here rather than at the router layer because
    /// this kernel never forwards, so a zero-TTL packet is malformed input.
    TtlExpired,
    /// Build: destination buffer too small.
    BufferTooSmall,
    /// Build: header + payload would exceed the 16-bit total-length field.
    PayloadTooLarge,
}

/// The fixed fields of a parsed IPv4 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Header {
    /// Header length in *bytes* (IHL × 4), already validated.
    pub header_len: usize,
    /// Combined DSCP/ECN byte; preserved so a reply can mirror it.
    pub dscp_ecn: u8,
    /// Length of header + payload as claimed by the sender, already checked
    /// against the buffer.
    pub total_length: u16,
    pub identification: u16,
    pub dont_fragment: bool,
    pub more_fragments: bool,
    /// Fragment offset in 8-byte units, as encoded on the wire.
    pub fragment_offset: u16,
    pub ttl: u8,
    pub protocol: u8,
    /// The checksum exactly as received (already verified by [`parse`]).
    pub checksum: u16,
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
}

/// A validated packet: header, its option bytes, and the payload, all
/// borrowing the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Packet<'a> {
    pub header: Ipv4Header,
    /// Option bytes between the fixed header and the payload. Empty for the
    /// common IHL=5 case. Present so a caller can log or reject them; this
    /// stack acts on none of them.
    pub options: &'a [u8],
    /// Payload trimmed to `total_length` — trailing Ethernet padding is
    /// removed here, which is why the Ethernet layer can leave it in place.
    pub payload: &'a [u8],
}

/// Parse and fully validate a received IPv4 packet.
///
/// `bytes` may be longer than the packet (Ethernet pads short frames); it may
/// never be shorter than `total_length` claims.
pub fn parse(bytes: &[u8]) -> Result<Ipv4Packet<'_>, Ipv4Error> {
    if bytes.len() < MIN_HEADER_LEN {
        return Err(Ipv4Error::TooShort);
    }
    if bytes[0] >> 4 != 4 {
        return Err(Ipv4Error::BadVersion);
    }
    let ihl = (bytes[0] & 0x0F) as usize;
    if ihl < 5 {
        return Err(Ipv4Error::BadHeaderLength);
    }
    let header_len = ihl * 4; // <= 60, cannot overflow
    if bytes.len() < header_len {
        return Err(Ipv4Error::HeaderTruncated);
    }

    let total_length = u16::from_be_bytes([bytes[2], bytes[3]]);
    if (total_length as usize) < header_len {
        return Err(Ipv4Error::TotalLengthTooSmall);
    }
    if total_length as usize > bytes.len() {
        return Err(Ipv4Error::TotalLengthExceedsBuffer);
    }

    // Checksum before interpreting anything else that matters: a corrupt
    // header's flags are not worth acting on.
    if !checksum::is_valid(&bytes[..header_len]) {
        return Err(Ipv4Error::BadChecksum);
    }

    let flags = bytes[6] >> 5;
    let fragment_offset = u16::from_be_bytes([bytes[6] & 0x1F, bytes[7]]);
    let more_fragments = flags & FLAG_MORE_FRAGMENTS != 0;
    if more_fragments || fragment_offset != 0 {
        return Err(Ipv4Error::Fragmented);
    }

    let ttl = bytes[8];
    if ttl == 0 {
        return Err(Ipv4Error::TtlExpired);
    }

    let header = Ipv4Header {
        header_len,
        dscp_ecn: bytes[1],
        total_length,
        identification: u16::from_be_bytes([bytes[4], bytes[5]]),
        dont_fragment: flags & FLAG_DONT_FRAGMENT != 0,
        more_fragments,
        fragment_offset,
        ttl,
        protocol: bytes[9],
        checksum: u16::from_be_bytes([bytes[CHECKSUM_OFFSET], bytes[CHECKSUM_OFFSET + 1]]),
        // Indices below are all < 20 <= header_len, checked above.
        src: Ipv4Addr([bytes[12], bytes[13], bytes[14], bytes[15]]),
        dst: Ipv4Addr([bytes[16], bytes[17], bytes[18], bytes[19]]),
    };

    Ok(Ipv4Packet {
        header,
        options: &bytes[MIN_HEADER_LEN..header_len],
        payload: &bytes[header_len..total_length as usize],
    })
}

/// Everything a caller must decide to emit a packet. Grouped into a struct
/// because a seven-argument builder is a bug factory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Builder {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub protocol: u8,
    pub ttl: u8,
    /// Only meaningful for reassembly, which nothing here performs; still, a
    /// caller should vary it per packet so intermediate routers behave.
    pub identification: u16,
    /// Sets DF. Recommended: this stack cannot receive fragments, so it should
    /// not invite path-MTU black holes by letting routers fragment its output
    /// either — though with DF set an oversize packet is dropped instead.
    pub dont_fragment: bool,
    pub dscp_ecn: u8,
}

impl Ipv4Builder {
    /// A sensible default: TTL 64, DF set, no traffic class.
    pub const fn new(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8) -> Ipv4Builder {
        Ipv4Builder {
            src,
            dst,
            protocol,
            ttl: 64,
            identification: 0,
            dont_fragment: true,
            dscp_ecn: 0,
        }
    }
}

/// Write a 20-byte option-free header for a packet whose payload is
/// `payload_len` bytes, computing the checksum. Returns [`MIN_HEADER_LEN`].
///
/// Split out from [`build_into`] so a driver can lay Ethernet, IPv4 and
/// transport headers into a single transmit buffer without copying the payload
/// twice.
pub fn build_header_into(
    buf: &mut [u8],
    b: &Ipv4Builder,
    payload_len: usize,
) -> Result<usize, Ipv4Error> {
    let total = MIN_HEADER_LEN
        .checked_add(payload_len)
        .ok_or(Ipv4Error::PayloadTooLarge)?;
    if total > u16::MAX as usize {
        return Err(Ipv4Error::PayloadTooLarge);
    }
    let hdr = buf
        .get_mut(..MIN_HEADER_LEN)
        .ok_or(Ipv4Error::BufferTooSmall)?;
    hdr[0] = 0x45; // version 4, IHL 5 — this builder never emits options
    hdr[1] = b.dscp_ecn;
    hdr[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    hdr[4..6].copy_from_slice(&b.identification.to_be_bytes());
    let flags = if b.dont_fragment {
        FLAG_DONT_FRAGMENT
    } else {
        0
    };
    hdr[6] = flags << 5; // fragment offset is always zero here
    hdr[7] = 0;
    hdr[8] = b.ttl;
    hdr[9] = b.protocol;
    hdr[10] = 0; // checksum computed over a zeroed field
    hdr[11] = 0;
    hdr[12..16].copy_from_slice(&b.src.0);
    hdr[16..20].copy_from_slice(&b.dst.0);
    let sum = checksum::checksum(hdr);
    hdr[10..12].copy_from_slice(&sum.to_be_bytes());
    Ok(MIN_HEADER_LEN)
}

/// Build a complete packet (header + payload) into `buf`.
pub fn build_into(buf: &mut [u8], b: &Ipv4Builder, payload: &[u8]) -> Result<usize, Ipv4Error> {
    let total = MIN_HEADER_LEN
        .checked_add(payload.len())
        .ok_or(Ipv4Error::PayloadTooLarge)?;
    if buf.len() < total {
        return Err(Ipv4Error::BufferTooSmall);
    }
    build_header_into(buf, b, payload.len())?;
    buf[MIN_HEADER_LEN..total].copy_from_slice(payload);
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 1);
    const DST: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 199);

    /// A 20-byte header + `payload` with a correct checksum.
    fn packet(protocol: u8, payload: &[u8]) -> [u8; 128] {
        let mut buf = [0u8; 128];
        let b = Ipv4Builder {
            identification: 0x1234,
            ..Ipv4Builder::new(SRC, DST, protocol)
        };
        build_into(&mut buf, &b, payload).unwrap();
        buf
    }

    fn fix_checksum(buf: &mut [u8]) {
        let ihl = (buf[0] & 0x0F) as usize * 4;
        buf[10] = 0;
        buf[11] = 0;
        let sum = checksum::checksum(&buf[..ihl]);
        buf[10..12].copy_from_slice(&sum.to_be_bytes());
    }

    #[test]
    fn round_trips_a_built_packet() {
        let payload = [1u8, 2, 3, 4, 5];
        let raw = packet(proto::UDP, &payload);
        let p = parse(&raw[..25]).unwrap();
        assert_eq!(p.header.src, SRC);
        assert_eq!(p.header.dst, DST);
        assert_eq!(p.header.protocol, proto::UDP);
        assert_eq!(p.header.ttl, 64);
        assert!(p.header.dont_fragment);
        assert_eq!(p.header.identification, 0x1234);
        assert_eq!(p.header.total_length, 25);
        assert_eq!(p.header.header_len, 20);
        assert_eq!(p.options, &[] as &[u8]);
        assert_eq!(p.payload, &payload[..]);
    }

    /// Ethernet pads short frames; the parser must trim to total_length rather
    /// than handing the pad to the transport layer.
    #[test]
    fn trims_trailing_ethernet_padding() {
        let payload = [9u8, 9, 9];
        let raw = packet(proto::UDP, &payload);
        let p = parse(&raw[..64]).unwrap();
        assert_eq!(p.payload, &payload[..], "pad bytes must not reach UDP");
    }

    #[test]
    fn rejects_truncation_at_every_boundary() {
        let raw = packet(proto::UDP, &[1, 2, 3, 4]);
        for n in 0..MIN_HEADER_LEN {
            assert_eq!(parse(&raw[..n]), Err(Ipv4Error::TooShort), "len {n}");
        }
        // 20 bytes of header present, but total_length claims 24.
        assert_eq!(
            parse(&raw[..MIN_HEADER_LEN]),
            Err(Ipv4Error::TotalLengthExceedsBuffer)
        );
        for n in MIN_HEADER_LEN..24 {
            assert_eq!(
                parse(&raw[..n]),
                Err(Ipv4Error::TotalLengthExceedsBuffer),
                "len {n}"
            );
        }
        assert!(parse(&raw[..24]).is_ok());
    }

    #[test]
    fn rejects_bad_version_and_ihl() {
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[0] = 0x65; // version 6
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::BadVersion));

        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[0] = 0x44; // IHL 4 — shorter than the fixed header
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::BadHeaderLength));

        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[0] = 0x40; // IHL 0
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::BadHeaderLength));
    }

    /// IHL claims options that the buffer does not contain.
    #[test]
    fn rejects_ihl_past_the_buffer() {
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[0] = 0x4F; // IHL 15 -> 60-byte header
        raw[2..4].copy_from_slice(&60u16.to_be_bytes());
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::HeaderTruncated));
    }

    #[test]
    fn parses_headers_that_carry_options() {
        // IHL 6: 20 fixed bytes + one 4-byte option (NOP, NOP, NOP, EOL).
        let mut raw = [0u8; 64];
        raw[0] = 0x46;
        raw[2..4].copy_from_slice(&28u16.to_be_bytes());
        raw[8] = 64;
        raw[9] = proto::UDP;
        raw[12..16].copy_from_slice(&SRC.0);
        raw[16..20].copy_from_slice(&DST.0);
        raw[20..24].copy_from_slice(&[0x01, 0x01, 0x01, 0x00]);
        raw[24..28].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        fix_checksum(&mut raw);
        let p = parse(&raw[..28]).unwrap();
        assert_eq!(p.header.header_len, 24);
        assert_eq!(p.options, &[0x01, 0x01, 0x01, 0x00]);
        assert_eq!(p.payload, &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn rejects_total_length_shorter_than_the_header() {
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[2..4].copy_from_slice(&19u16.to_be_bytes());
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::TotalLengthTooSmall));
        raw[2..4].copy_from_slice(&0u16.to_be_bytes());
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::TotalLengthTooSmall));
    }

    #[test]
    fn rejects_oversized_total_length() {
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[2..4].copy_from_slice(&65535u16.to_be_bytes());
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::TotalLengthExceedsBuffer));
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[10] ^= 0xFF; // corrupt the checksum itself
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::BadChecksum));

        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[15] ^= 0x01; // corrupt an address, leaving the checksum stale
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::BadChecksum));
    }

    /// Both fragment indicators must be refused with the same distinct error,
    /// so a caller can never mistake a first fragment for a whole datagram.
    #[test]
    fn refuses_fragments_explicitly() {
        // More-fragments set, offset 0 (the first fragment — the dangerous one,
        // because it looks like a complete packet).
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[6] |= 0x20;
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::Fragmented));

        // Non-zero offset, MF clear (a last fragment).
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[6] = 0x00;
        raw[7] = 0x01;
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::Fragmented));

        // Offset in the high bits of byte 6 must be seen too.
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[6] = 0x1F;
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::Fragmented));

        // DF alone is not a fragment.
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[6] = 0x40;
        fix_checksum(&mut raw);
        assert!(parse(&raw[..24]).is_ok());
    }

    #[test]
    fn rejects_zero_ttl() {
        let mut raw = packet(proto::UDP, &[0; 4]);
        raw[8] = 0;
        fix_checksum(&mut raw);
        assert_eq!(parse(&raw[..24]), Err(Ipv4Error::TtlExpired));
        raw[8] = 1;
        fix_checksum(&mut raw);
        assert!(parse(&raw[..24]).is_ok());
    }

    #[test]
    fn build_rejects_small_buffers_and_oversize_payloads() {
        let b = Ipv4Builder::new(SRC, DST, proto::UDP);
        let mut small = [0u8; 19];
        assert_eq!(
            build_into(&mut small, &b, &[]),
            Err(Ipv4Error::BufferTooSmall)
        );
        let mut buf = [0u8; 24];
        assert_eq!(
            build_into(&mut buf, &b, &[0; 8]),
            Err(Ipv4Error::BufferTooSmall)
        );
        assert_eq!(
            build_header_into(&mut buf, &b, 65516),
            Err(Ipv4Error::PayloadTooLarge)
        );
        assert!(build_header_into(&mut buf, &b, 65515).is_ok());
    }

    #[test]
    fn build_without_df_clears_the_flag() {
        let b = Ipv4Builder {
            dont_fragment: false,
            ..Ipv4Builder::new(SRC, DST, proto::ICMP)
        };
        let mut buf = [0u8; 32];
        let n = build_into(&mut buf, &b, &[7, 7]).unwrap();
        let p = parse(&buf[..n]).unwrap();
        assert!(!p.header.dont_fragment);
        assert_eq!(p.header.protocol, proto::ICMP);
    }

    #[test]
    fn address_helpers() {
        assert!(Ipv4Addr::UNSPECIFIED.is_unspecified());
        assert!(Ipv4Addr::BROADCAST.is_broadcast());
        assert!(Ipv4Addr::new(224, 0, 0, 251).is_multicast());
        assert!(!Ipv4Addr::new(223, 255, 255, 255).is_multicast());
        assert!(Ipv4Addr::new(127, 0, 0, 1).is_loopback());
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        assert!(SRC.same_subnet(DST, mask));
        assert!(!SRC.same_subnet(Ipv4Addr::new(10, 0, 0, 1), mask));
        assert_eq!(SRC.subnet_broadcast(mask), Ipv4Addr::new(192, 168, 0, 255));
        assert_eq!(SRC.to_u32(), 0xC0A8_0001);
        assert_eq!(Ipv4Addr::from_u32(0xC0A8_0001), SRC);
        assert_eq!(
            Ipv4Addr::from_slice(&[1, 2, 3, 4]),
            Some(Ipv4Addr::new(1, 2, 3, 4))
        );
        assert!(Ipv4Addr::from_slice(&[1, 2, 3]).is_none());
        assert!(Ipv4Addr::from_slice(&[1, 2, 3, 4, 5]).is_none());
    }

    #[test]
    fn address_formatting_covers_every_digit_count() {
        let cases: [([u8; 4], &str); 5] = [
            ([0, 0, 0, 0], "0.0.0.0"),
            ([255, 255, 255, 255], "255.255.255.255"),
            ([192, 168, 0, 1], "192.168.0.1"),
            ([10, 99, 100, 209], "10.99.100.209"),
            ([1, 20, 30, 4], "1.20.30.4"),
        ];
        for (octets, want) in cases {
            let mut buf = [0u8; 15];
            assert_eq!(Ipv4Addr(octets).format(&mut buf), want);
        }
    }
}
