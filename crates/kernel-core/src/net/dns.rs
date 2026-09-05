//! A minimal DNS client codec (RFC 1035): build an A query, read the answer.
//!
//! DNS is the most hostile parser in this stack, because a response is a
//! self-referential structure supplied entirely by whoever answered. Two
//! defences shape the code:
//!
//! * **Compression pointers are followed with a hard jump budget.** A pointer
//!   may point backwards to any offset, including one that points back — the
//!   classic decompression bomb. Every pointer costs one of a fixed number of
//!   jumps, so a malicious response terminates instead of looping.
//! * **Nothing is read relative to a length the message itself supplies
//!   without first checking that length against the buffer.** Every rdata
//!   walk is bounds-checked at the point of use, so a truncated or lying
//!   record ends the parse rather than reading past it.
//!
//! Only IN/A is understood. A response is matched on transaction id *and* the
//! echoed question, since an off-path attacker who guesses the id still has to
//! reproduce the question.

pub const HEADER_LEN: usize = 12;
/// Largest query this builder will emit (a name can be 255 bytes on the wire).
pub const MAX_QUERY_LEN: usize = HEADER_LEN + 255 + 4;
/// Standard DNS service port.
pub const PORT: u16 = 53;
/// How many compression pointers one name may traverse before the parser gives
/// up. Real names need none; a handful is generous and bounds the work.
const MAX_JUMPS: u32 = 8;

pub mod rtype {
    pub const A: u16 = 1;
    pub const CNAME: u16 = 5;
}
pub mod class {
    pub const IN: u16 = 1;
}

/// Response codes this client distinguishes.
pub mod rcode {
    pub const NO_ERROR: u8 = 0;
    pub const FORMAT_ERROR: u8 = 1;
    pub const SERVER_FAILURE: u8 = 2;
    pub const NAME_ERROR: u8 = 3;
    pub const REFUSED: u8 = 5;
}

use super::ipv4::Ipv4Addr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsError {
    /// Shorter than a DNS header.
    TooShort,
    /// The transaction id did not match the outstanding query.
    IdMismatch,
    /// The message is a query, not a response.
    NotAResponse,
    /// The server set a non-zero RCODE; the value is carried for reporting.
    ServerError(u8),
    /// Truncation bit set: the answer did not fit in a datagram and this
    /// client does not fall back to TCP, so the answer must not be used.
    Truncated,
    /// A name ran past the end of the message, used a reserved label type, or
    /// exceeded the compression-pointer budget.
    BadName,
    /// A record's rdata length disagrees with the buffer or its type.
    BadRecord,
    /// The response echoed a different question than the one asked.
    QuestionMismatch,
    /// A well-formed response that contains no A record.
    NoAddress,
    /// The name is not encodable (empty, over-long label, or over-long total).
    BadQueryName,
    BufferTooSmall,
}

/// Encode a dotted host name into DNS wire format (length-prefixed labels
/// terminated by a zero byte), returning the number of bytes written.
///
/// Rejects rather than truncates: an over-long label or name that got silently
/// clipped would query a *different* host than the caller asked for.
pub fn encode_name(buf: &mut [u8], name: &str) -> Result<usize, DnsError> {
    if name.is_empty() || name.len() > 253 {
        return Err(DnsError::BadQueryName);
    }
    // A single trailing dot ("example.com.") is the legal absolute form and
    // encodes identically; it is removed here so the split below can treat
    // every remaining empty label ("a..b", ".a", "a..") as the error it is.
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() {
        return Err(DnsError::BadQueryName);
    }
    let mut n = 0usize;
    for label in name.split('.') {
        if label.is_empty() {
            return Err(DnsError::BadQueryName);
        }
        if label.len() > 63 {
            return Err(DnsError::BadQueryName);
        }
        let end = n + 1 + label.len();
        let out = buf.get_mut(n..end).ok_or(DnsError::BufferTooSmall)?;
        out[0] = label.len() as u8;
        out[1..].copy_from_slice(label.as_bytes());
        n = end;
    }
    *buf.get_mut(n).ok_or(DnsError::BufferTooSmall)? = 0;
    Ok(n + 1)
}

/// Build a standard recursive A query for `name` into `buf`.
pub fn build_query(buf: &mut [u8], id: u16, name: &str) -> Result<usize, DnsError> {
    let head = buf.get_mut(..HEADER_LEN).ok_or(DnsError::BufferTooSmall)?;
    head[0..2].copy_from_slice(&id.to_be_bytes());
    head[2] = 0x01; // QR=0 opcode=0 AA=0 TC=0 RD=1
    head[3] = 0x00; // RA=0 Z=0 RCODE=0
    head[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    head[6..8].copy_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    head[8..10].copy_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    head[10..12].copy_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    let n = encode_name(&mut buf[HEADER_LEN..], name)?;
    let end = HEADER_LEN + n + 4;
    let tail = buf
        .get_mut(HEADER_LEN + n..end)
        .ok_or(DnsError::BufferTooSmall)?;
    tail[0..2].copy_from_slice(&rtype::A.to_be_bytes());
    tail[2..4].copy_from_slice(&class::IN.to_be_bytes());
    Ok(end)
}

/// Skip over a (possibly compressed) name starting at `offset`, returning the
/// offset of the byte after it *in the current record stream*.
///
/// A compression pointer ends the name in the stream even though the name
/// continues elsewhere, which is why the return value is the position after
/// the pointer rather than after the expanded name.
fn skip_name(msg: &[u8], mut offset: usize) -> Result<usize, DnsError> {
    let mut jumps = 0u32;
    loop {
        let len = *msg.get(offset).ok_or(DnsError::BadName)?;
        match len & 0xC0 {
            0x00 => {
                if len == 0 {
                    return Ok(offset + 1);
                }
                offset = offset
                    .checked_add(1 + len as usize)
                    .ok_or(DnsError::BadName)?;
                if offset > msg.len() {
                    return Err(DnsError::BadName);
                }
            }
            0xC0 => {
                // Two-byte pointer: the name ends here in this stream.
                if offset + 1 >= msg.len() {
                    return Err(DnsError::BadName);
                }
                jumps += 1;
                if jumps > MAX_JUMPS {
                    return Err(DnsError::BadName);
                }
                return Ok(offset + 2);
            }
            // 0x40/0x80 are reserved label types (EDNS0 extended labels were
            // deprecated). Refusing is safer than guessing a length.
            _ => return Err(DnsError::BadName),
        }
    }
}

/// Compare a name at `offset` (following compression) against `expected`,
/// case-insensitively as DNS requires.
fn name_equals(msg: &[u8], mut offset: usize, expected: &str) -> Result<bool, DnsError> {
    let mut jumps = 0u32;
    let mut want = expected.split('.').filter(|l| !l.is_empty());
    loop {
        let len = *msg.get(offset).ok_or(DnsError::BadName)?;
        match len & 0xC0 {
            0x00 => {
                if len == 0 {
                    return Ok(want.next().is_none());
                }
                let start = offset + 1;
                let end = start.checked_add(len as usize).ok_or(DnsError::BadName)?;
                let label = msg.get(start..end).ok_or(DnsError::BadName)?;
                match want.next() {
                    Some(w) if w.len() == label.len() => {
                        if !w
                            .as_bytes()
                            .iter()
                            .zip(label)
                            .all(|(a, b)| a.eq_ignore_ascii_case(b))
                        {
                            return Ok(false);
                        }
                    }
                    _ => return Ok(false),
                }
                offset = end;
            }
            0xC0 => {
                let hi = *msg.get(offset).ok_or(DnsError::BadName)? as usize;
                let lo = *msg.get(offset + 1).ok_or(DnsError::BadName)? as usize;
                let target = ((hi & 0x3F) << 8) | lo;
                jumps += 1;
                if jumps > MAX_JUMPS || target >= msg.len() {
                    return Err(DnsError::BadName);
                }
                offset = target;
            }
            _ => return Err(DnsError::BadName),
        }
    }
}

/// The outcome of a successful lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub address: Ipv4Addr,
    /// Time-to-live of the A record that supplied the address.
    pub ttl: u32,
}

/// Parse a response to the query `(id, name)` and return the first A record.
///
/// The question section is re-read and compared: matching only on the
/// transaction id would accept an answer to a different question that happened
/// to reuse the id.
pub fn parse_response(msg: &[u8], id: u16, name: &str) -> Result<Answer, DnsError> {
    if msg.len() < HEADER_LEN {
        return Err(DnsError::TooShort);
    }
    if u16::from_be_bytes([msg[0], msg[1]]) != id {
        return Err(DnsError::IdMismatch);
    }
    if msg[2] & 0x80 == 0 {
        return Err(DnsError::NotAResponse);
    }
    if msg[2] & 0x02 != 0 {
        return Err(DnsError::Truncated);
    }
    let rc = msg[3] & 0x0F;
    if rc != rcode::NO_ERROR {
        return Err(DnsError::ServerError(rc));
    }
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]);
    let ancount = u16::from_be_bytes([msg[6], msg[7]]);
    if qdcount != 1 {
        return Err(DnsError::QuestionMismatch);
    }

    // Question section.
    if !name_equals(msg, HEADER_LEN, name)? {
        return Err(DnsError::QuestionMismatch);
    }
    let mut off = skip_name(msg, HEADER_LEN)?;
    let q_end = off.checked_add(4).ok_or(DnsError::BadRecord)?;
    let qfields = msg.get(off..q_end).ok_or(DnsError::BadRecord)?;
    if u16::from_be_bytes([qfields[0], qfields[1]]) != rtype::A
        || u16::from_be_bytes([qfields[2], qfields[3]]) != class::IN
    {
        return Err(DnsError::QuestionMismatch);
    }
    off = q_end;

    // Answer section. CNAMEs are walked over rather than chased: this client
    // wants an address, and a server that returns a CNAME without the
    // corresponding A record has not answered the question.
    for _ in 0..ancount {
        off = skip_name(msg, off)?;
        let head_end = off.checked_add(10).ok_or(DnsError::BadRecord)?;
        let head = msg.get(off..head_end).ok_or(DnsError::BadRecord)?;
        let rtype_v = u16::from_be_bytes([head[0], head[1]]);
        let class_v = u16::from_be_bytes([head[2], head[3]]);
        let ttl = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
        let rdlen = u16::from_be_bytes([head[8], head[9]]) as usize;
        let rd_end = head_end.checked_add(rdlen).ok_or(DnsError::BadRecord)?;
        let rdata = msg.get(head_end..rd_end).ok_or(DnsError::BadRecord)?;
        if rtype_v == rtype::A && class_v == class::IN {
            // An A record is four bytes by definition. A record that claims
            // to be an A but is not four bytes is malformed, not a variant to
            // accommodate.
            if rdlen != 4 {
                return Err(DnsError::BadRecord);
            }
            return Ok(Answer {
                address: Ipv4Addr([rdata[0], rdata[1], rdata[2], rdata[3]]),
                ttl,
            });
        }
        off = rd_end;
    }
    Err(DnsError::NoAddress)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str) -> ([u8; MAX_QUERY_LEN], usize) {
        let mut buf = [0u8; MAX_QUERY_LEN];
        let n = build_query(&mut buf, 0x1234, name).unwrap();
        (buf, n)
    }

    /// Build a response echoing the question and appending `answers`
    /// (already-encoded resource records).
    fn response(name: &str, id: u16, flags1: u8, flags2: u8, answers: &[&[u8]]) -> [u8; 512] {
        let mut buf = [0u8; 512];
        let (q, qn) = query(name);
        buf[..qn].copy_from_slice(&q[..qn]);
        buf[0..2].copy_from_slice(&id.to_be_bytes());
        buf[2] = flags1;
        buf[3] = flags2;
        buf[6..8].copy_from_slice(&(answers.len() as u16).to_be_bytes());
        let mut off = qn;
        for a in answers {
            buf[off..off + a.len()].copy_from_slice(a);
            off += a.len();
        }
        buf
    }

    /// An A record using a compression pointer back to the question's name.
    fn a_record(addr: [u8; 4], ttl: u32) -> [u8; 16] {
        let mut r = [0u8; 16];
        r[0] = 0xC0;
        r[1] = HEADER_LEN as u8;
        r[2..4].copy_from_slice(&rtype::A.to_be_bytes());
        r[4..6].copy_from_slice(&class::IN.to_be_bytes());
        r[6..10].copy_from_slice(&ttl.to_be_bytes());
        r[10..12].copy_from_slice(&4u16.to_be_bytes());
        r[12..16].copy_from_slice(&addr);
        r
    }

    #[test]
    fn query_layout_is_a_standard_recursive_a_query() {
        let (buf, n) = query("os.itisyou.app");
        assert_eq!(u16::from_be_bytes([buf[0], buf[1]]), 0x1234);
        assert_eq!(buf[2], 0x01); // RD set, QR clear
        assert_eq!(u16::from_be_bytes([buf[4], buf[5]]), 1);
        assert_eq!(&buf[HEADER_LEN..HEADER_LEN + 3], b"\x02os");
        assert_eq!(n, HEADER_LEN + 1 + 2 + 1 + 7 + 1 + 3 + 1 + 4);
        assert_eq!(&buf[n - 4..n], &[0x00, 0x01, 0x00, 0x01]);
    }

    #[test]
    fn resolves_an_address() {
        let msg = response(
            "os.itisyou.app",
            0x1234,
            0x81,
            0x80,
            &[&a_record([1, 2, 3, 4], 300)],
        );
        let ans = parse_response(&msg, 0x1234, "os.itisyou.app").unwrap();
        assert_eq!(ans.address, Ipv4Addr::new(1, 2, 3, 4));
        assert_eq!(ans.ttl, 300);
    }

    #[test]
    fn matching_is_case_insensitive() {
        let msg = response(
            "OS.ItIsYou.App",
            0x1234,
            0x81,
            0x80,
            &[&a_record([9, 9, 9, 9], 1)],
        );
        assert!(parse_response(&msg, 0x1234, "os.itisyou.app").is_ok());
    }

    #[test]
    fn rejects_a_response_to_a_different_question() {
        let msg = response(
            "evil.example",
            0x1234,
            0x81,
            0x80,
            &[&a_record([6, 6, 6, 6], 1)],
        );
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::QuestionMismatch)
        );
    }

    #[test]
    fn rejects_a_mismatched_transaction_id() {
        let msg = response(
            "os.itisyou.app",
            0x9999,
            0x81,
            0x80,
            &[&a_record([1, 1, 1, 1], 1)],
        );
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::IdMismatch)
        );
    }

    #[test]
    fn rejects_a_query_masquerading_as_a_response() {
        let msg = response("os.itisyou.app", 0x1234, 0x01, 0x80, &[]);
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::NotAResponse)
        );
    }

    #[test]
    fn refuses_truncated_answers_rather_than_using_them() {
        let msg = response(
            "os.itisyou.app",
            0x1234,
            0x83, // QR + TC
            0x80,
            &[&a_record([1, 2, 3, 4], 1)],
        );
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::Truncated)
        );
    }

    #[test]
    fn surfaces_server_rcodes() {
        for rc in [
            rcode::FORMAT_ERROR,
            rcode::SERVER_FAILURE,
            rcode::NAME_ERROR,
            rcode::REFUSED,
        ] {
            let msg = response("os.itisyou.app", 0x1234, 0x81, 0x80 | rc, &[]);
            assert_eq!(
                parse_response(&msg, 0x1234, "os.itisyou.app"),
                Err(DnsError::ServerError(rc))
            );
        }
    }

    #[test]
    fn a_cname_only_answer_is_no_address() {
        let mut cname = [0u8; 16];
        cname[0] = 0xC0;
        cname[1] = HEADER_LEN as u8;
        cname[2..4].copy_from_slice(&rtype::CNAME.to_be_bytes());
        cname[4..6].copy_from_slice(&class::IN.to_be_bytes());
        cname[10..12].copy_from_slice(&4u16.to_be_bytes());
        let msg = response("os.itisyou.app", 0x1234, 0x81, 0x80, &[&cname]);
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::NoAddress)
        );
    }

    #[test]
    fn skips_a_cname_and_finds_the_following_a_record() {
        let mut cname = [0u8; 16];
        cname[0] = 0xC0;
        cname[1] = HEADER_LEN as u8;
        cname[2..4].copy_from_slice(&rtype::CNAME.to_be_bytes());
        cname[4..6].copy_from_slice(&class::IN.to_be_bytes());
        cname[10..12].copy_from_slice(&4u16.to_be_bytes());
        let msg = response(
            "os.itisyou.app",
            0x1234,
            0x81,
            0x80,
            &[&cname, &a_record([5, 6, 7, 8], 60)],
        );
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app")
                .unwrap()
                .address,
            Ipv4Addr::new(5, 6, 7, 8)
        );
    }

    #[test]
    fn an_a_record_that_is_not_four_bytes_is_malformed() {
        let mut bad = a_record([1, 2, 3, 4], 1);
        bad[10..12].copy_from_slice(&16u16.to_be_bytes());
        let msg = response("os.itisyou.app", 0x1234, 0x81, 0x80, &[&bad]);
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::BadRecord)
        );
    }

    #[test]
    fn a_self_referential_pointer_terminates() {
        // A name whose pointer points at itself: the classic decompression
        // bomb. It must return an error, and above all must not hang.
        let mut msg = [0u8; 64];
        msg[0..2].copy_from_slice(&0x1234u16.to_be_bytes());
        msg[2] = 0x81;
        msg[4..6].copy_from_slice(&1u16.to_be_bytes());
        msg[HEADER_LEN] = 0xC0;
        msg[HEADER_LEN + 1] = HEADER_LEN as u8;
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::BadName)
        );
    }

    #[test]
    fn a_pointer_past_the_message_is_refused() {
        let mut msg = [0u8; 64];
        msg[0..2].copy_from_slice(&0x1234u16.to_be_bytes());
        msg[2] = 0x81;
        msg[4..6].copy_from_slice(&1u16.to_be_bytes());
        msg[HEADER_LEN] = 0xC0;
        msg[HEADER_LEN + 1] = 200;
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::BadName)
        );
    }

    #[test]
    fn reserved_label_types_are_refused() {
        let mut msg = [0u8; 64];
        msg[0..2].copy_from_slice(&0x1234u16.to_be_bytes());
        msg[2] = 0x81;
        msg[4..6].copy_from_slice(&1u16.to_be_bytes());
        msg[HEADER_LEN] = 0x80;
        assert_eq!(
            parse_response(&msg, 0x1234, "os.itisyou.app"),
            Err(DnsError::BadName)
        );
    }

    #[test]
    fn rejects_truncation_at_every_boundary() {
        let msg = response(
            "os.itisyou.app",
            0x1234,
            0x81,
            0x80,
            &[&a_record([1, 2, 3, 4], 1)],
        );
        let full = HEADER_LEN + 1 + 2 + 1 + 7 + 1 + 3 + 1 + 4 + 16;
        for n in 0..HEADER_LEN {
            assert_eq!(
                parse_response(&msg[..n], 0x1234, "os.itisyou.app"),
                Err(DnsError::TooShort),
                "len {n}"
            );
        }
        for n in HEADER_LEN..full {
            assert!(
                parse_response(&msg[..n], 0x1234, "os.itisyou.app").is_err(),
                "len {n} must not parse"
            );
        }
        assert!(parse_response(&msg[..full], 0x1234, "os.itisyou.app").is_ok());
    }

    #[test]
    fn rejects_unencodable_names() {
        let mut buf = [0u8; MAX_QUERY_LEN];
        assert_eq!(build_query(&mut buf, 1, ""), Err(DnsError::BadQueryName));
        assert_eq!(
            build_query(&mut buf, 1, "a..b"),
            Err(DnsError::BadQueryName)
        );
        assert_eq!(build_query(&mut buf, 1, "."), Err(DnsError::BadQueryName));
        assert_eq!(build_query(&mut buf, 1, ".a"), Err(DnsError::BadQueryName));
        assert_eq!(build_query(&mut buf, 1, "a.."), Err(DnsError::BadQueryName));
        let long_label = "a".repeat(64);
        assert_eq!(
            build_query(&mut buf, 1, &long_label),
            Err(DnsError::BadQueryName)
        );
        // 50 six-byte labels = 305 bytes on the wire, past the 253 limit.
        let long_name = "abcde.".repeat(50) + "abcde";
        assert_eq!(
            build_query(&mut buf, 1, &long_name),
            Err(DnsError::BadQueryName)
        );
    }

    #[test]
    fn a_trailing_dot_is_the_legal_absolute_form() {
        let mut buf = [0u8; MAX_QUERY_LEN];
        let with = build_query(&mut buf, 1, "os.itisyou.app.").unwrap();
        let mut buf2 = [0u8; MAX_QUERY_LEN];
        let without = build_query(&mut buf2, 1, "os.itisyou.app").unwrap();
        assert_eq!(with, without);
        assert_eq!(buf[..with], buf2[..without]);
    }

    #[test]
    fn build_rejects_small_buffers() {
        let mut small = [0u8; HEADER_LEN + 4];
        assert_eq!(
            build_query(&mut small, 1, "os.itisyou.app"),
            Err(DnsError::BufferTooSmall)
        );
    }
}
