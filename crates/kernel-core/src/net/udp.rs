//! UDP (RFC 768) over IPv4.
//!
//! Two things here are easy to get wrong and are therefore handled explicitly:
//!
//! * **The length field is authoritative, not the buffer.** A datagram's
//!   header claims a length; IPv4 may hand over more bytes than that (it does
//!   not, after `ipv4::parse` trims, but a caller could pass a raw frame). A
//!   parser that returns "the rest of the buffer" as the payload lets a sender
//!   smuggle bytes past the length a receiver thinks it read. So the length is
//!   validated against the buffer and then used to bound the payload.
//! * **The checksum is optional in IPv4 UDP, and zero means "not computed".**
//!   A received zero must be accepted without verification; a *computed*
//!   checksum of zero must be transmitted as `0xFFFF`, because the two have
//!   the same one's-complement value and only that encoding distinguishes
//!   "checksummed to zero" from "no checksum". This stack always computes one.

use super::checksum;
use super::ipv4::{proto, Ipv4Addr};

pub const HEADER_LEN: usize = 8;
pub const CHECKSUM_OFFSET: usize = 6;
/// Largest payload that still fits an IPv4 packet inside a 1500-byte MTU.
pub const MAX_PAYLOAD_LEN: usize = 1500 - super::ipv4::MIN_HEADER_LEN - HEADER_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpError {
    /// Fewer than [`HEADER_LEN`] bytes.
    TooShort,
    /// The length field is smaller than the header itself.
    BadLength,
    /// The length field claims more bytes than the buffer holds.
    LengthPastBuffer,
    /// A non-zero checksum that did not verify against the pseudo-header.
    BadChecksum,
    /// Port 0 is reserved and never a valid endpoint.
    ZeroPort,
    BufferTooSmall,
    PayloadTooLarge,
}

/// A validated datagram borrowing the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Datagram<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    /// True when the sender left the checksum field zero (legal in IPv4 UDP,
    /// meaning "not computed"). Surfaced rather than hidden so a caller can
    /// refuse unchecksummed data if it wants to.
    pub checksum_omitted: bool,
    pub payload: &'a [u8],
}

/// Parse a datagram. `src`/`dst` are the IPv4 addresses it arrived between,
/// needed because the UDP checksum covers a pseudo-header built from them —
/// which is what binds a datagram to its addresses and stops a checksum being
/// replayed across a rewritten header.
pub fn parse<'a>(bytes: &'a [u8], src: Ipv4Addr, dst: Ipv4Addr) -> Result<Datagram<'a>, UdpError> {
    if bytes.len() < HEADER_LEN {
        return Err(UdpError::TooShort);
    }
    let src_port = u16::from_be_bytes([bytes[0], bytes[1]]);
    let dst_port = u16::from_be_bytes([bytes[2], bytes[3]]);
    if dst_port == 0 {
        return Err(UdpError::ZeroPort);
    }
    let length = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    if length < HEADER_LEN {
        return Err(UdpError::BadLength);
    }
    if length > bytes.len() {
        return Err(UdpError::LengthPastBuffer);
    }
    let wire = &bytes[..length];
    let stored = u16::from_be_bytes([bytes[6], bytes[7]]);
    let checksum_omitted = stored == 0;
    if !checksum_omitted {
        let computed = checksum::transport_checksum(
            src.octets(),
            dst.octets(),
            proto::UDP,
            &wire[..HEADER_LEN],
            CHECKSUM_OFFSET,
            &wire[HEADER_LEN..],
        )
        .ok_or(UdpError::BadLength)?;
        // Compare against the *wire* encoding, so a datagram whose sum folds
        // to zero — legitimately transmitted as 0xFFFF — verifies instead of
        // being rejected as corrupt.
        if wire_checksum(computed) != stored {
            return Err(UdpError::BadChecksum);
        }
    }
    Ok(Datagram {
        src_port,
        dst_port,
        checksum_omitted,
        payload: &wire[HEADER_LEN..],
    })
}

/// Build a datagram (header + payload) into `buf`, always checksummed.
pub fn build_into(
    buf: &mut [u8],
    src: Ipv4Addr,
    dst: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Result<usize, UdpError> {
    if src_port == 0 || dst_port == 0 {
        return Err(UdpError::ZeroPort);
    }
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(UdpError::PayloadTooLarge);
    }
    let total = HEADER_LEN + payload.len();
    let out = buf.get_mut(..total).ok_or(UdpError::BufferTooSmall)?;
    out[0..2].copy_from_slice(&src_port.to_be_bytes());
    out[2..4].copy_from_slice(&dst_port.to_be_bytes());
    out[4..6].copy_from_slice(&(total as u16).to_be_bytes());
    out[6] = 0;
    out[7] = 0;
    out[HEADER_LEN..total].copy_from_slice(payload);
    let sum = checksum::transport_checksum(
        src.octets(),
        dst.octets(),
        proto::UDP,
        &out[..HEADER_LEN],
        CHECKSUM_OFFSET,
        payload,
    )
    .ok_or(UdpError::PayloadTooLarge)?;
    out[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2].copy_from_slice(&wire_checksum(sum).to_be_bytes());
    Ok(total)
}

/// Encode a computed checksum for transmission.
///
/// Zero is not a legal transmitted UDP checksum: it is the reserved encoding
/// for "no checksum was computed". A sum that folds to zero is sent as
/// `0xFFFF`, which has the same value in one's-complement arithmetic, so the
/// receiver's verification is unaffected while the "omitted" signal stays
/// unambiguous.
const fn wire_checksum(sum: u16) -> u16 {
    if sum == 0 {
        0xFFFF
    } else {
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
    const DST: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);

    fn built(payload: &[u8]) -> ([u8; 256], usize) {
        let mut buf = [0u8; 256];
        let n = build_into(&mut buf, SRC, DST, 40000, 7, payload).unwrap();
        (buf, n)
    }

    #[test]
    fn round_trips() {
        let (buf, n) = built(b"hello");
        let d = parse(&buf[..n], SRC, DST).unwrap();
        assert_eq!(d.src_port, 40000);
        assert_eq!(d.dst_port, 7);
        assert_eq!(d.payload, b"hello");
        assert!(!d.checksum_omitted);
    }

    #[test]
    fn empty_payload_round_trips() {
        let (buf, n) = built(b"");
        assert_eq!(n, HEADER_LEN);
        assert!(parse(&buf[..n], SRC, DST).unwrap().payload.is_empty());
    }

    #[test]
    fn odd_length_payload_round_trips() {
        let (buf, n) = built(b"odd");
        assert_eq!(parse(&buf[..n], SRC, DST).unwrap().payload, b"odd");
    }

    #[test]
    fn the_checksum_is_bound_to_the_addresses() {
        let (buf, n) = built(b"hello");
        // Same bytes, different source address: the pseudo-header changes, so
        // the checksum must no longer verify.
        let other = Ipv4Addr::new(10, 0, 2, 16);
        assert_eq!(parse(&buf[..n], other, DST), Err(UdpError::BadChecksum));
        assert_eq!(parse(&buf[..n], SRC, other), Err(UdpError::BadChecksum));
    }

    #[test]
    fn detects_payload_corruption() {
        let (buf, n) = built(b"hello");
        for i in HEADER_LEN..n {
            let mut corrupt = buf;
            corrupt[i] ^= 0x20;
            assert_eq!(
                parse(&corrupt[..n], SRC, DST),
                Err(UdpError::BadChecksum),
                "byte {i}"
            );
        }
    }

    #[test]
    fn a_zero_checksum_is_accepted_without_verification() {
        let (mut buf, n) = built(b"hello");
        buf[CHECKSUM_OFFSET] = 0;
        buf[CHECKSUM_OFFSET + 1] = 0;
        let d = parse(&buf[..n], SRC, DST).unwrap();
        assert!(d.checksum_omitted);
        assert_eq!(d.payload, b"hello");
    }

    #[test]
    fn the_length_field_bounds_the_payload_not_the_buffer() {
        // A datagram claiming 8+3 bytes inside a 16-byte buffer must yield a
        // 3-byte payload; the extra bytes are not smuggled through.
        let (mut buf, _) = built(b"abc");
        let claimed = HEADER_LEN + 3;
        let mut padded = [0xEEu8; 32];
        padded[..claimed].copy_from_slice(&buf[..claimed]);
        let d = parse(&padded, SRC, DST).unwrap();
        assert_eq!(d.payload, b"abc");
        // And a length that lies about being longer than the buffer is caught.
        buf[4..6].copy_from_slice(&(claimed as u16 + 1).to_be_bytes());
        assert_eq!(
            parse(&buf[..claimed], SRC, DST),
            Err(UdpError::LengthPastBuffer)
        );
    }

    #[test]
    fn rejects_length_shorter_than_the_header() {
        let (mut buf, n) = built(b"abc");
        for bad in [0u16, 1, 7] {
            buf[4..6].copy_from_slice(&bad.to_be_bytes());
            assert_eq!(parse(&buf[..n], SRC, DST), Err(UdpError::BadLength));
        }
    }

    #[test]
    fn rejects_truncation_at_every_boundary() {
        let (buf, _) = built(b"abc");
        for n in 0..HEADER_LEN {
            assert_eq!(
                parse(&buf[..n], SRC, DST),
                Err(UdpError::TooShort),
                "len {n}"
            );
        }
    }

    #[test]
    fn rejects_port_zero_in_both_directions() {
        let mut buf = [0u8; 64];
        assert_eq!(
            build_into(&mut buf, SRC, DST, 0, 7, b""),
            Err(UdpError::ZeroPort)
        );
        assert_eq!(
            build_into(&mut buf, SRC, DST, 40000, 0, b""),
            Err(UdpError::ZeroPort)
        );
        let (mut wire, n) = built(b"x");
        wire[2..4].copy_from_slice(&0u16.to_be_bytes());
        assert_eq!(parse(&wire[..n], SRC, DST), Err(UdpError::ZeroPort));
    }

    #[test]
    fn build_rejects_small_buffers_and_oversize_payloads() {
        let mut small = [0u8; HEADER_LEN + 2];
        assert_eq!(
            build_into(&mut small, SRC, DST, 1, 1, b"abc"),
            Err(UdpError::BufferTooSmall)
        );
        let mut big = [0u8; 2048];
        let payload = [0u8; MAX_PAYLOAD_LEN + 1];
        assert_eq!(
            build_into(&mut big, SRC, DST, 1, 1, &payload),
            Err(UdpError::PayloadTooLarge)
        );
        assert!(build_into(&mut big, SRC, DST, 1, 1, &payload[..MAX_PAYLOAD_LEN]).is_ok());
    }

    #[test]
    fn a_transmitted_checksum_is_never_zero() {
        // Search for a payload whose one's-complement sum folds to zero; the
        // builder must encode it as 0xFFFF so the receiver does not read it as
        // "no checksum". Rather than hunt for such a payload, assert the
        // invariant across a wide sweep — no built datagram may carry a zero.
        let mut buf = [0u8; 256];
        for i in 0..=255u8 {
            for len in [0usize, 1, 2, 3, 17] {
                let payload = [i; 17];
                let n = build_into(&mut buf, SRC, DST, 40000, 7, &payload[..len]).unwrap();
                assert_ne!(
                    u16::from_be_bytes([buf[CHECKSUM_OFFSET], buf[CHECKSUM_OFFSET + 1]]),
                    0,
                    "byte {i} len {len}"
                );
                assert!(parse(&buf[..n], SRC, DST).is_ok());
            }
        }
    }
}
