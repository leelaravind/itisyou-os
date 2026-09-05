//! A hash chain over audit records, so a persisted trail cannot be edited
//! without the edit showing.
//!
//! Each record's hash covers the previous hash as well as its own bytes:
//!
//! ```text
//! head_0 = previous boot's head (or zeros on a fresh system)
//! head_i = SHA-256(head_{i-1} || record_i)
//! ```
//!
//! Storing only the final head means verification is a single pass with one
//! comparison at the end, and every interesting tamper is caught by it:
//! altering a record changes its own hash and therefore every later one;
//! deleting one removes a link; reordering two swaps the inputs. None of those
//! can be repaired without recomputing the whole chain, which is exactly the
//! work an attacker who could also rewrite the stored head would have to do —
//! and that is the honest limit of this scheme, stated in the module docs
//! rather than left implied. It detects *editing*; it does not defend against
//! an attacker who can rewrite the whole file including its head. Signing the
//! head, or writing it somewhere the running system cannot reach, is what
//! would close that gap, and neither exists yet.
//!
//! The chain also spans boots: a new boot starts from the head it recovered,
//! so provenance is continuous across restarts rather than a fresh log each
//! time.

use crate::sha256::Sha256;

/// A chain head: the hash covering every record so far.
pub type Head = [u8; 32];

/// The head of an empty chain on a system that has never persisted a record.
pub const GENESIS: Head = [0u8; 32];

/// Extend a chain by one record.
pub fn extend(head: &Head, record: &[u8]) -> Head {
    let mut h = Sha256::new();
    h.update(head);
    h.update(record);
    h.finalize()
}

/// Recompute a chain over `records` starting from `start`.
pub fn compute(start: &Head, records: &[&[u8]]) -> Head {
    let mut head = *start;
    for record in records {
        head = extend(&head, record);
    }
    head
}

/// Why a persisted trail did not verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// The recomputed head does not match the stored one: a record was
    /// altered, removed, reordered, or inserted.
    HeadMismatch,
    /// The record count in the header disagrees with the records present.
    CountMismatch,
}

/// Verify a persisted trail.
pub fn verify(
    start: &Head,
    records: &[&[u8]],
    claimed_count: usize,
    claimed_head: &Head,
) -> Result<(), VerifyError> {
    // The count is checked separately from the head so a truncated file
    // reports the more specific fault. Both are tampering; only one of them
    // tells you what was done.
    if records.len() != claimed_count {
        return Err(VerifyError::CountMismatch);
    }
    if compute(start, records) != *claimed_head {
        return Err(VerifyError::HeadMismatch);
    }
    Ok(())
}

/// Format a head as lowercase hex into a caller-supplied buffer, returning it
/// as a borrowed `&str`.
pub fn format_head<'a>(head: &Head, out: &'a mut [u8; 64]) -> &'a str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, b) in head.iter().enumerate() {
        out[i * 2] = HEX[(b >> 4) as usize];
        out[i * 2 + 1] = HEX[(b & 0x0F) as usize];
    }
    // Every byte written above is an ASCII hex digit, so this cannot fail.
    core::str::from_utf8(out).unwrap_or("")
}

/// Parse a 64-character lowercase hex head.
pub fn parse_head(text: &str) -> Option<Head> {
    if text.len() != 64 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = hex_value(bytes[i * 2])?;
        let lo = hex_value(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        // Uppercase is rejected rather than accepted: this parser only ever
        // reads what this module wrote, and being liberal about input format
        // is how two encodings of the same head start circulating.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> Vec<&'static [u8]> {
        vec![
            b"1 100 0 pkg_install cap=0x0 ok",
            b"2 140 3 fs_write cap=0x80 ok path=/data/notes",
            b"3 190 3 udp_bind cap=0x100 denied reason=no_handle",
        ]
    }

    #[test]
    fn a_chain_verifies_against_its_own_head() {
        let r = records();
        let head = compute(&GENESIS, &r);
        assert_eq!(verify(&GENESIS, &r, r.len(), &head), Ok(()));
    }

    #[test]
    fn altering_any_byte_of_any_record_breaks_it() {
        let r = records();
        let head = compute(&GENESIS, &r);
        for i in 0..r.len() {
            for byte in 0..r[i].len() {
                let mut owned: Vec<Vec<u8>> = r.iter().map(|x| x.to_vec()).collect();
                owned[i][byte] ^= 0x01;
                let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
                assert_eq!(
                    verify(&GENESIS, &refs, refs.len(), &head),
                    Err(VerifyError::HeadMismatch),
                    "record {i} byte {byte}"
                );
            }
        }
    }

    #[test]
    fn deleting_a_record_breaks_it() {
        let r = records();
        let head = compute(&GENESIS, &r);
        for i in 0..r.len() {
            let mut short = r.clone();
            short.remove(i);
            // The count check fires first; both are failures.
            assert!(
                verify(&GENESIS, &short, r.len(), &head).is_err(),
                "removed {i}"
            );
            // Even with an honestly-updated count, the head still disagrees.
            assert_eq!(
                verify(&GENESIS, &short, short.len(), &head),
                Err(VerifyError::HeadMismatch),
                "removed {i}"
            );
        }
    }

    #[test]
    fn reordering_records_breaks_it() {
        let mut r = records();
        let head = compute(&GENESIS, &r);
        r.swap(0, 2);
        assert_eq!(
            verify(&GENESIS, &r, r.len(), &head),
            Err(VerifyError::HeadMismatch)
        );
    }

    #[test]
    fn inserting_a_record_breaks_it() {
        let r = records();
        let head = compute(&GENESIS, &r);
        let mut more = r.clone();
        more.push(b"4 200 0 forged cap=0x0 ok");
        assert!(verify(&GENESIS, &more, more.len(), &head).is_err());
    }

    #[test]
    fn a_wrong_starting_head_breaks_it() {
        // This is what makes the chain span boots: continuing from the wrong
        // previous head is detected exactly like an altered record.
        let r = records();
        let head = compute(&GENESIS, &r);
        let mut other = GENESIS;
        other[0] = 1;
        assert_eq!(
            verify(&other, &r, r.len(), &head),
            Err(VerifyError::HeadMismatch)
        );
    }

    #[test]
    fn a_truncated_trail_reports_the_count() {
        let r = records();
        let head = compute(&GENESIS, &r);
        assert_eq!(
            verify(&GENESIS, &r[..2], r.len(), &head),
            Err(VerifyError::CountMismatch)
        );
    }

    #[test]
    fn an_empty_chain_is_its_starting_head() {
        assert_eq!(compute(&GENESIS, &[]), GENESIS);
        assert_eq!(verify(&GENESIS, &[], 0, &GENESIS), Ok(()));
    }

    #[test]
    fn heads_round_trip_through_hex() {
        let r = records();
        let head = compute(&GENESIS, &r);
        let mut buf = [0u8; 64];
        let text = format_head(&head, &mut buf);
        assert_eq!(text.len(), 64);
        assert_eq!(parse_head(text), Some(head));
    }

    #[test]
    fn hex_parsing_rejects_malformed_input() {
        assert_eq!(parse_head(""), None);
        assert_eq!(parse_head("00"), None);
        assert_eq!(parse_head(&"0".repeat(63)), None);
        assert_eq!(parse_head(&"0".repeat(65)), None);
        assert_eq!(parse_head(&"g".repeat(64)), None);
        assert_eq!(
            parse_head(&"A".repeat(64)),
            None,
            "uppercase is not accepted"
        );
        assert_eq!(parse_head(&"0".repeat(64)), Some(GENESIS));
    }
}
