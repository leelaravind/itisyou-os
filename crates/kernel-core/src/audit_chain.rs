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
//! an attacker who can rewrite the whole file including its head. Writing the
//! head somewhere the running system cannot reach closes that gap: that is
//! the V0.9 witness anchor (`kernel::audit::anchor`).
//!
//! The chain also spans boots: a new boot starts from the head it recovered,
//! so provenance is continuous across restarts rather than a fresh log each
//! time.
//!
//! # The stored trail is a window (V0.11)
//!
//! A trail holds the NEWEST records of that continuous history, not all of
//! it, so its header names the head the first stored record extends:
//!
//! ```text
//! itisyou-audit v2 boot=<n> count=<records> base=<head> head=<head>
//! ```
//!
//! and verification is `compute(base, records) == head`. Before V0.11 the
//! format had no `base` (it is `v1`, verified from [`GENESIS`]), yet the
//! kernel wrote only the in-memory ring's records under a head covering every
//! record ever made: any trail saved after the ring had dropped a record, or
//! in a later boot than the first, then failed verification although nobody
//! had touched it (AUDIT11-001). `v1` trails are still read, as `base =
//! GENESIS`. Moving `base` forward over dropped records is honest about what
//! the file can prove: the records before `base` are gone, and the chain
//! vouches only for the ones still present. It is not a new way to erase
//! history unseen — dropping records from the front changes `count`, which
//! the witness anchor compares, and forging a different `base` for the same
//! `head` needs a SHA-256 preimage.

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

/// Header marker of a trail written by V0.8–V0.10: no `base`, verified from
/// [`GENESIS`].
pub const MAGIC_V1: &str = "itisyou-audit v1";
/// Header marker of a trail written since V0.11: `base` names the head the
/// first stored record extends.
pub const MAGIC_V2: &str = "itisyou-audit v2";

/// A stored trail's header line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrailHeader {
    /// Boot number of the boot that wrote the trail.
    pub boot: u64,
    /// Records stored after the header.
    pub count: usize,
    /// The head the first stored record extends ([`GENESIS`] when the stored
    /// records are the whole history).
    pub base: Head,
    /// The head after the last stored record.
    pub head: Head,
}

impl TrailHeader {
    /// Check `records` — the lines after the header — against this header.
    pub fn verify(&self, records: &[&[u8]]) -> Result<(), VerifyError> {
        verify(&self.base, records, self.count, &self.head)
    }
}

/// Parse a trail's header line, strictly: the fields in their fixed order,
/// separated by single spaces, and nothing after them.
///
/// ```text
/// itisyou-audit v2 boot=<n> count=<n> base=<hex> head=<hex>
/// itisyou-audit v1 boot=<n> count=<n> head=<hex>            (base = GENESIS)
/// ```
///
/// The header is outside the chain, so a lenient parser would let two
/// different lines mean the same trail (a repeated field, a `+1`), and the
/// one-byte difference between them would carry no evidence either way.
pub fn parse_header(line: &str) -> Option<TrailHeader> {
    let (v2, rest) = match line.strip_prefix(MAGIC_V2) {
        Some(rest) => (true, rest),
        None => (false, line.strip_prefix(MAGIC_V1)?),
    };
    let mut fields = rest.strip_prefix(' ')?.split(' ');
    let boot = decimal(fields.next()?.strip_prefix("boot=")?)?;
    let count = usize::try_from(decimal(fields.next()?.strip_prefix("count=")?)?).ok()?;
    let base = if v2 {
        parse_head(fields.next()?.strip_prefix("base=")?)?
    } else {
        GENESIS
    };
    let head = parse_head(fields.next()?.strip_prefix("head=")?)?;
    if fields.next().is_some() {
        return None;
    }
    Some(TrailHeader {
        boot,
        count,
        base,
        head,
    })
}

/// Write `header` as a V2 header line, without a terminator.
pub fn write_header(header: &TrailHeader, out: &mut impl core::fmt::Write) -> core::fmt::Result {
    let mut base = [0u8; 64];
    let mut head = [0u8; 64];
    write!(
        out,
        "{MAGIC_V2} boot={} count={} base={} head={}",
        header.boot,
        header.count,
        format_head(&header.base, &mut base),
        format_head(&header.head, &mut head)
    )
}

/// A plain decimal: ASCII digits only (no sign, no spaces), at most 20 of
/// them, and within `u64`.
fn decimal(text: &str) -> Option<u64> {
    if text.is_empty() || text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Write an audit record's detail with every control character (C0, DEL and
/// C1) escaped, plus the backslash and the double quote, and the kernel's
/// marker prefix neutralized (V0.11, AUDIT11-002).
///
/// This is for any text the kernel echoes that a program or a disk chose: an
/// audit detail (a file name), a store file name, an application name read
/// back from the package store, a file's contents. A line break in it would
/// split one record into two lines, both in a stored trail (so an untouched
/// trail would fail verification) and on the serial console, where the text
/// after it would pose as a kernel marker line. A marker prefix inside a line
/// is caught too: the harness takes the FIRST `[ITISYOU:` on a line, so
/// `store: x = [ITISYOU:B210]` would read as a boot stage; it becomes
/// `[RING3-U:`, exactly as in process output ([`crate::linebuf`]). A quote
/// would end the serial marker's quoted `detail="…"` early. Escaping the
/// backslash keeps the escapes unambiguous: `\x0a` in the output always came
/// from a line break, never from the four characters `\x0a`.
pub fn escape_untrusted(text: &str, out: &mut impl core::fmt::Write) -> core::fmt::Result {
    let marker = core::str::from_utf8(crate::linebuf::KERNEL_MARKER).unwrap_or("[ITISYOU:");
    let neutral = core::str::from_utf8(crate::linebuf::RING3_MARKER).unwrap_or("[RING3-U:");
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix(marker) {
            out.write_str(neutral)?;
            rest = after;
            continue;
        }
        match c {
            '\\' => out.write_str("\\\\")?,
            '"' => out.write_str("\\\"")?,
            c if c.is_ascii_control() => write!(out, "\\x{:02x}", c as u32)?,
            c if c.is_control() => write!(out, "\\u{{{:x}}}", c as u32)?,
            c => out.write_char(c)?,
        }
        rest = &rest[c.len_utf8()..];
    }
    Ok(())
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

    fn hex(head: &Head) -> String {
        let mut buf = [0u8; 64];
        format_head(head, &mut buf).to_string()
    }

    /// The trail a writer keeps: the newest `cap` records of `all`, and the
    /// head the first of them extends — what `kernel::audit` maintains
    /// incrementally, done here in one go.
    fn window<'a>(all: &[&'a [u8]], cap: usize) -> (Head, Vec<&'a [u8]>) {
        let dropped = all.len().saturating_sub(cap);
        (compute(&GENESIS, &all[..dropped]), all[dropped..].to_vec())
    }

    #[test]
    fn a_window_of_a_longer_history_verifies_from_its_base() {
        // AUDIT11-001: the history is longer than what is stored. From
        // GENESIS the stored records cannot reach the head (the V0.8-V0.10
        // verification, which reported an untouched trail as tampered); from
        // the base the writer recorded, they do.
        let all: Vec<Vec<u8>> = (0..200)
            .map(|i| format!("{i} {} 0 fs_write 0x80 ok path=/data/f{i}", i * 7).into_bytes())
            .collect();
        let all: Vec<&[u8]> = all.iter().map(|r| r.as_slice()).collect();
        let head = compute(&GENESIS, &all);
        let (base, kept) = window(&all, 128);
        assert_eq!(kept.len(), 128);
        assert_eq!(
            verify(&GENESIS, &kept, kept.len(), &head),
            Err(VerifyError::HeadMismatch),
            "the V0.10 verification of a trimmed trail"
        );
        let header = TrailHeader {
            boot: 3,
            count: kept.len(),
            base,
            head,
        };
        assert_eq!(header.verify(&kept), Ok(()));
        // Dropping the oldest record one at a time keeps the base in step:
        // extending the base over the dropped record is the same as
        // recomputing from GENESIS.
        let mut base = GENESIS;
        for (i, r) in all.iter().enumerate().take(72) {
            base = extend(&base, r);
            assert_eq!(base, compute(&GENESIS, &all[..=i]));
        }
        assert_eq!(base, header.base);
    }

    #[test]
    fn a_window_still_catches_editing() {
        let all = records();
        let head = compute(&GENESIS, &all);
        let (base, kept) = window(&all, 2);
        let header = TrailHeader {
            boot: 1,
            count: kept.len(),
            base,
            head,
        };
        assert_eq!(header.verify(&kept), Ok(()));
        // Altering a kept record, dropping one, or claiming another base.
        let mut edited: Vec<Vec<u8>> = kept.iter().map(|r| r.to_vec()).collect();
        edited[1][0] ^= 1;
        let edited: Vec<&[u8]> = edited.iter().map(|r| r.as_slice()).collect();
        assert_eq!(header.verify(&edited), Err(VerifyError::HeadMismatch));
        assert_eq!(header.verify(&kept[1..]), Err(VerifyError::CountMismatch));
        let mut other = header;
        other.base = GENESIS;
        assert_eq!(other.verify(&kept), Err(VerifyError::HeadMismatch));
        // Dropping the oldest record AND moving base over it is the one
        // change the chain accepts, because the trail then honestly stores
        // less — and its count is what the witness anchor compares.
        let shorter = TrailHeader {
            count: 1,
            base: extend(&base, kept[0]),
            ..header
        };
        assert_eq!(shorter.verify(&kept[1..]), Ok(()));
        assert_ne!(shorter.count, header.count);
    }

    #[test]
    fn headers_round_trip_and_v1_reads_as_genesis_base() {
        let r = records();
        let head = compute(&GENESIS, &r);
        let base = compute(&GENESIS, &r[..1]);
        let header = TrailHeader {
            boot: 7,
            count: 2,
            base,
            head,
        };
        let mut line = String::new();
        write_header(&header, &mut line).unwrap();
        assert_eq!(
            line,
            format!(
                "itisyou-audit v2 boot=7 count=2 base={} head={}",
                hex(&base),
                hex(&head)
            )
        );
        assert_eq!(parse_header(&line), Some(header));
        // What V0.8-V0.10 wrote, byte for byte.
        let v1 = format!("itisyou-audit v1 boot=0 count=3 head={}", hex(&head));
        let parsed = parse_header(&v1).unwrap();
        assert_eq!(
            parsed,
            TrailHeader {
                boot: 0,
                count: 3,
                base: GENESIS,
                head
            }
        );
        assert_eq!(parsed.verify(&r), Ok(()));
    }

    #[test]
    fn header_parsing_is_strict() {
        let h = hex(&GENESIS);
        let good = format!("itisyou-audit v2 boot=1 count=0 base={h} head={h}");
        assert!(parse_header(&good).is_some());
        for bad in [
            String::new(),
            format!("itisyou-audit v3 boot=1 count=0 base={h} head={h}"),
            format!("itisyou-audit v2  boot=1 count=0 base={h} head={h}"),
            format!("itisyou-audit v2 boot=+1 count=0 base={h} head={h}"),
            format!("itisyou-audit v2 boot=1 count=-0 base={h} head={h}"),
            format!("itisyou-audit v2 boot=1 count=0 head={h}"),
            format!("itisyou-audit v2 boot=1 count=0 base={h} head={h} "),
            format!("itisyou-audit v2 boot=1 count=0 base={h} head={h} head={h}"),
            format!("itisyou-audit v2 count=0 boot=1 base={h} head={h}"),
            format!("itisyou-audit v2 boot=1 count=0 base={h} head={}", &h[1..]),
            format!("itisyou-audit v2 boot=18446744073709551616 count=0 base={h} head={h}"),
            format!("itisyou-audit v1 boot=1 count=0 base={h} head={h}"),
            format!("itisyou-audit v2boot=1 count=0 base={h} head={h}"),
        ] {
            assert_eq!(parse_header(&bad), None, "{bad:?}");
        }
    }

    fn escaped(text: &str) -> String {
        let mut out = String::new();
        escape_untrusted(text, &mut out).unwrap();
        out
    }

    #[test]
    fn untrusted_text_escapes_to_one_unambiguous_line() {
        assert_eq!(
            escaped("path=/data/notes.txt bytes=4"),
            "path=/data/notes.txt bytes=4"
        );
        assert_eq!(escaped("caf\u{e9}"), "caf\u{e9}");
        assert_eq!(escaped(""), "");
        // The Ring 3 file name that would forge a marker line: one line, and
        // no kernel marker prefix left in it either.
        assert_eq!(
            escaped("path=/data/a\n[ITISYOU:AUDIT] forged"),
            "path=/data/a\\x0a[RING3-U:AUDIT] forged"
        );
        // A marker prefix mid-line, at the start, twice, and adjacent to
        // multi-byte text; a partial prefix is left alone.
        assert_eq!(escaped("[ITISYOU:B210] up"), "[RING3-U:B210] up");
        assert_eq!(
            escaped("\u{e9}[ITISYOU:[ITISYOU:x"),
            "\u{e9}[RING3-U:[RING3-U:x"
        );
        assert_eq!(escaped("[ITISYOU"), "[ITISYOU");
        assert_eq!(escaped("[itisyou:B210]"), "[itisyou:B210]");
        assert_eq!(escaped("a\r\tb\u{7f}"), "a\\x0d\\x09b\\x7f");
        assert_eq!(escaped("\u{85}\u{9b}"), "\\u{85}\\u{9b}");
        assert_eq!(escaped("say \"hi\""), "say \\\"hi\\\"");
        // A literal backslash-x cannot pass for an escaped control byte.
        assert_eq!(escaped("\\x0a"), "\\\\x0a");
        // Whatever goes in, what comes out is one line of printable text
        // with no kernel marker in it.
        let mut all: String = (0u32..0x2000).filter_map(char::from_u32).collect();
        all.push_str("[ITISYOU:PANIC] x\n[ITISYOU:B210]");
        let out = escaped(&all);
        assert!(!out.chars().any(char::is_control));
        assert_eq!(out.lines().count(), 1);
        assert!(!out.contains("[ITISYOU:"));
        assert_eq!(crate::marker::parse_line(&out), None);
    }
}
