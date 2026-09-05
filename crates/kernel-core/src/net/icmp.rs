//! ICMP echo (RFC 792) — enough to be pinged and to ping, and no more.
//!
//! Only Echo Request and Echo Reply are recognized. The other ICMP types are
//! not "unsupported yet"; acting on them is a decision this stack declines to
//! make. Redirect rewrites routing, Destination Unreachable can be forged to
//! tear down a peer's state, and Timestamp/Address-Mask leak host facts — all
//! from an unauthenticated datagram. Returning [`IcmpError::UnsupportedType`]
//! lets the caller count them without ever having acted.
//!
//! Unlike UDP and TCP, the ICMP checksum covers the ICMP message alone: there
//! is no pseudo-header, so it is not bound to the IP addresses.

use super::checksum;

/// Fixed part of an echo header: type, code, checksum, identifier, sequence.
pub const ECHO_HEADER_LEN: usize = 8;
/// Offset of the checksum field inside the header.
pub const CHECKSUM_OFFSET: usize = 2;

pub mod types {
    pub const ECHO_REPLY: u8 = 0;
    pub const DEST_UNREACHABLE: u8 = 3;
    pub const REDIRECT: u8 = 5;
    pub const ECHO_REQUEST: u8 = 8;
    pub const TIME_EXCEEDED: u8 = 11;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoKind {
    Request,
    Reply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcmpError {
    /// Fewer than [`ECHO_HEADER_LEN`] bytes.
    TooShort,
    /// A type this stack deliberately does not act on.
    UnsupportedType,
    /// Echo messages must carry code 0; anything else is malformed.
    BadCode,
    /// The one's-complement checksum over the whole message did not verify.
    BadChecksum,
    /// Destination buffer too small.
    BufferTooSmall,
    /// Payload longer than the caller's stated maximum.
    PayloadTooLarge,
}

/// A validated echo message borrowing the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Echo<'a> {
    pub kind: EchoKind,
    pub identifier: u16,
    pub sequence: u16,
    pub payload: &'a [u8],
}

/// Parse an ICMP echo message out of an IPv4 payload.
///
/// `bytes` must be exactly the IPv4 payload: ICMP carries no length of its
/// own, so the payload boundary is the only thing that says where the echo
/// data ends. Passing a padded buffer would fold the padding into the
/// checksum and into `payload`.
pub fn parse_echo(bytes: &[u8]) -> Result<Echo<'_>, IcmpError> {
    if bytes.len() < ECHO_HEADER_LEN {
        return Err(IcmpError::TooShort);
    }
    let kind = match bytes[0] {
        types::ECHO_REQUEST => EchoKind::Request,
        types::ECHO_REPLY => EchoKind::Reply,
        _ => return Err(IcmpError::UnsupportedType),
    };
    if bytes[1] != 0 {
        return Err(IcmpError::BadCode);
    }
    if !checksum::is_valid(bytes) {
        return Err(IcmpError::BadChecksum);
    }
    Ok(Echo {
        kind,
        identifier: u16::from_be_bytes([bytes[4], bytes[5]]),
        sequence: u16::from_be_bytes([bytes[6], bytes[7]]),
        payload: &bytes[ECHO_HEADER_LEN..],
    })
}

/// Build an echo message into `buf`, returning its total length.
pub fn build_echo_into(
    buf: &mut [u8],
    kind: EchoKind,
    identifier: u16,
    sequence: u16,
    payload: &[u8],
) -> Result<usize, IcmpError> {
    let total = ECHO_HEADER_LEN
        .checked_add(payload.len())
        .ok_or(IcmpError::PayloadTooLarge)?;
    let out = buf.get_mut(..total).ok_or(IcmpError::BufferTooSmall)?;
    out[0] = match kind {
        EchoKind::Request => types::ECHO_REQUEST,
        EchoKind::Reply => types::ECHO_REPLY,
    };
    out[1] = 0;
    out[2] = 0; // checksum computed over a zeroed field
    out[3] = 0;
    out[4..6].copy_from_slice(&identifier.to_be_bytes());
    out[6..8].copy_from_slice(&sequence.to_be_bytes());
    out[ECHO_HEADER_LEN..total].copy_from_slice(payload);
    let sum = checksum::checksum(out);
    out[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2].copy_from_slice(&sum.to_be_bytes());
    Ok(total)
}

impl Echo<'_> {
    /// Is this the reply to a request we sent with this identifier/sequence?
    ///
    /// Both fields must match: identifier alone would accept a reply to a
    /// different probe in the same session, which is how a naive ping reports
    /// a round-trip time it never measured.
    pub fn answers(&self, identifier: u16, sequence: u16) -> bool {
        self.kind == EchoKind::Reply && self.identifier == identifier && self.sequence == sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(kind: EchoKind, id: u16, seq: u16, payload: &[u8]) -> ([u8; 128], usize) {
        let mut buf = [0u8; 128];
        let n = build_echo_into(&mut buf, kind, id, seq, payload).unwrap();
        (buf, n)
    }

    #[test]
    fn request_round_trips() {
        let (buf, n) = built(EchoKind::Request, 0xABCD, 7, b"itisyou-os");
        let echo = parse_echo(&buf[..n]).unwrap();
        assert_eq!(echo.kind, EchoKind::Request);
        assert_eq!(echo.identifier, 0xABCD);
        assert_eq!(echo.sequence, 7);
        assert_eq!(echo.payload, b"itisyou-os");
    }

    #[test]
    fn reply_round_trips_and_matches_its_request() {
        let (buf, n) = built(EchoKind::Reply, 1, 2, b"x");
        let echo = parse_echo(&buf[..n]).unwrap();
        assert!(echo.answers(1, 2));
        // Right identifier, wrong sequence: not our reply.
        assert!(!echo.answers(1, 3));
        assert!(!echo.answers(9, 2));
    }

    #[test]
    fn a_request_never_answers_a_request() {
        let (buf, n) = built(EchoKind::Request, 1, 2, b"");
        assert!(!parse_echo(&buf[..n]).unwrap().answers(1, 2));
    }

    #[test]
    fn empty_payload_is_legal() {
        let (buf, n) = built(EchoKind::Reply, 0, 0, b"");
        assert_eq!(n, ECHO_HEADER_LEN);
        assert!(parse_echo(&buf[..n]).unwrap().payload.is_empty());
    }

    #[test]
    fn odd_length_payload_checksums_correctly() {
        // Exercises the checksum's odd-byte padding path.
        let (buf, n) = built(EchoKind::Request, 0x1234, 1, b"odd");
        assert_eq!(n, ECHO_HEADER_LEN + 3);
        assert!(parse_echo(&buf[..n]).is_ok());
    }

    #[test]
    fn detects_a_single_bit_flip_anywhere() {
        let (mut buf, n) = built(EchoKind::Request, 0x1234, 1, b"payload!");
        for i in 0..n {
            // Skip the checksum field itself: flipping it is also detected,
            // but the interesting property is that data corruption is.
            if (CHECKSUM_OFFSET..CHECKSUM_OFFSET + 2).contains(&i) {
                continue;
            }
            let mut corrupt = buf;
            corrupt[i] ^= 0x01;
            // A type flip is caught as an unsupported type, not a checksum
            // error; either way it must not parse as a valid echo.
            assert!(parse_echo(&corrupt[..n]).is_err(), "byte {i}");
        }
        buf[CHECKSUM_OFFSET] ^= 0xFF;
        assert_eq!(parse_echo(&buf[..n]), Err(IcmpError::BadChecksum));
    }

    #[test]
    fn refuses_types_it_will_not_act_on() {
        let (mut buf, n) = built(EchoKind::Request, 1, 1, b"data");
        for t in [
            types::DEST_UNREACHABLE,
            types::REDIRECT,
            types::TIME_EXCEEDED,
            13, // timestamp
            17, // address mask request
            255,
        ] {
            buf[0] = t;
            assert_eq!(
                parse_echo(&buf[..n]),
                Err(IcmpError::UnsupportedType),
                "type {t}"
            );
        }
    }

    #[test]
    fn refuses_nonzero_code() {
        let (mut buf, n) = built(EchoKind::Request, 1, 1, b"data");
        buf[1] = 3;
        assert_eq!(parse_echo(&buf[..n]), Err(IcmpError::BadCode));
    }

    #[test]
    fn rejects_truncation_at_every_boundary() {
        let (buf, _) = built(EchoKind::Request, 1, 1, b"data");
        for n in 0..ECHO_HEADER_LEN {
            assert_eq!(parse_echo(&buf[..n]), Err(IcmpError::TooShort), "len {n}");
        }
    }

    #[test]
    fn build_rejects_small_buffers() {
        let mut small = [0u8; 8];
        assert_eq!(
            build_echo_into(&mut small, EchoKind::Request, 1, 1, b"toolong"),
            Err(IcmpError::BufferTooSmall)
        );
        // Exactly the header fits.
        assert_eq!(
            build_echo_into(&mut small, EchoKind::Request, 1, 1, b""),
            Ok(ECHO_HEADER_LEN)
        );
    }

    #[test]
    fn a_trailing_byte_changes_the_message() {
        // Proves the caller must pass exactly the IPv4 payload: Ethernet
        // padding folded into the checksum would be a silent corruption.
        let (buf, n) = built(EchoKind::Request, 1, 1, b"abc");
        let mut padded = [0u8; 128];
        padded[..n].copy_from_slice(&buf[..n]);
        let echoed = parse_echo(&padded[..n + 2]).unwrap();
        assert_eq!(echoed.payload, b"abc\0\0");
    }
}
