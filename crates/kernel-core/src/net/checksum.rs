//! The RFC 1071 Internet checksum, plus the RFC 768/793 pseudo-header variant.
//!
//! Every IP-family protocol below reuses this one primitive, so it is worth
//! isolating: the subtle parts (odd-length padding, the "the checksum field
//! reads as zero while you compute it" convention, and the fact that the sum
//! is carry-folded rather than truncated) are easy to get wrong once and then
//! wrong everywhere.
//!
//! Overflow policy: [`Checksum`] folds the carry back after *every* 16-bit
//! word, so the accumulator is always `<= 0xFFFF` before the next addition and
//! `sum + word <= 0x1_FFFE` can never overflow `u32`. That costs two extra ALU
//! ops per word versus deferring the fold, which is irrelevant at the frame
//! sizes this kernel handles, and it removes any dependence on the input
//! length staying under 128 KiB. No `wrapping_*` is needed here.

/// Incremental one's-complement accumulator.
///
/// Split across calls because a transport checksum spans three disjoint byte
/// ranges (pseudo-header, transport header with a zeroed checksum field, and
/// payload) that are never contiguous in memory. The accumulator carries a
/// pending odd byte across [`Checksum::push`] calls so that splitting the same
/// logical byte stream at an odd boundary yields the same result as checksumming
/// it in one piece.
#[derive(Debug, Clone, Copy)]
pub struct Checksum {
    sum: u32,
    odd_byte: u8,
    has_odd: bool,
}

impl Default for Checksum {
    fn default() -> Self {
        Checksum::new()
    }
}

impl Checksum {
    pub const fn new() -> Checksum {
        Checksum {
            sum: 0,
            odd_byte: 0,
            has_odd: false,
        }
    }

    /// Add one big-endian 16-bit word, folding the carry immediately.
    fn add_word(&mut self, word: u16) {
        let s = self.sum + word as u32;
        self.sum = (s & 0xFFFF) + (s >> 16);
    }

    /// Feed an arbitrary byte range. Handles a byte left pending by a previous
    /// odd-length `push` so that chunk boundaries are invisible to the result.
    pub fn push(&mut self, data: &[u8]) {
        let mut rest = data;
        if self.has_odd {
            if let Some((&first, tail)) = rest.split_first() {
                self.add_word(u16::from_be_bytes([self.odd_byte, first]));
                self.has_odd = false;
                self.odd_byte = 0;
                rest = tail;
            }
        }
        let (pairs, remainder) = rest.as_chunks::<2>();
        for &pair in pairs {
            self.add_word(u16::from_be_bytes(pair));
        }
        if let Some(&last) = remainder.first() {
            self.odd_byte = last;
            self.has_odd = true;
        }
    }

    /// Feed `count` zero bytes — used to stand in for a checksum field that
    /// must read as zero without mutating the caller's buffer.
    ///
    /// Zero words add nothing to the sum, but they still shift the odd/even
    /// *phase* of the byte stream, which changes how every later byte pairs up.
    /// Only an even run of zeros on an already-aligned stream is a true no-op.
    pub fn push_zeros(&mut self, count: usize) {
        if !self.has_odd && count.is_multiple_of(2) {
            return;
        }
        const ZEROS: [u8; 32] = [0; 32];
        let mut left = count;
        while left > 0 {
            let n = left.min(ZEROS.len());
            self.push(&ZEROS[..n]);
            left -= n;
        }
    }

    /// Add a big-endian 16-bit value (convenience for pseudo-header fields).
    pub fn push_u16(&mut self, value: u16) {
        self.push(&value.to_be_bytes());
    }

    /// The final, complemented checksum, ready to be written into a header.
    ///
    /// A trailing odd byte is padded on the right with zero (RFC 1071 §1).
    /// The pad is *not* part of the packet; it only affects the arithmetic.
    pub fn finish(self) -> u16 {
        !self.folded_sum()
    }

    /// The folded, *uncomplemented* sum. A datagram whose checksum field is
    /// already filled in sums to `0xFFFF` when it is intact, which is what
    /// [`is_valid`] tests.
    pub fn folded_sum(mut self) -> u16 {
        if self.has_odd {
            let pending = self.odd_byte;
            self.has_odd = false;
            self.add_word(u16::from_be_bytes([pending, 0]));
        }
        self.sum as u16
    }
}

/// Checksum a single contiguous range (e.g. an IPv4 header whose checksum
/// field the caller has already zeroed).
pub fn checksum(data: &[u8]) -> u16 {
    let mut c = Checksum::new();
    c.push(data);
    c.finish()
}

/// Checksum `data` while treating the two bytes at `field_offset` as zero.
///
/// This is the non-destructive form of "zero the checksum field, then compute":
/// it lets a parser verify a received header without needing a mutable copy of
/// an attacker-supplied buffer. An out-of-range `field_offset` degenerates to a
/// plain checksum of the whole range rather than panicking.
pub fn checksum_with_zeroed_field(data: &[u8], field_offset: usize) -> u16 {
    let mut c = Checksum::new();
    let after = field_offset.saturating_add(2);
    match (data.get(..field_offset), data.get(after..)) {
        (Some(head), Some(tail)) => {
            c.push(head);
            c.push_zeros(2);
            c.push(tail);
        }
        _ => c.push(data),
    }
    c.finish()
}

/// True when `data` (checksum field included) is internally consistent.
///
/// The one's-complement sum of a correct datagram is `0xFFFF`, so the
/// complement is zero. Note that a datagram whose real checksum happens to be
/// `0x0000` is transmitted as `0xFFFF` by the sender (the two are the same
/// value in one's-complement arithmetic), which is why this test works for
/// both encodings.
pub fn is_valid(data: &[u8]) -> bool {
    checksum(data) == 0
}

/// The 12-byte IPv4 pseudo-header prefixed to TCP and UDP checksums
/// (RFC 793 §3.1 / RFC 768). It is never transmitted; it exists so that a
/// misdelivered datagram (wrong addresses or protocol) fails the check.
pub fn pseudo_header(src: [u8; 4], dst: [u8; 4], protocol: u8, transport_len: u16) -> [u8; 12] {
    let len = transport_len.to_be_bytes();
    [
        src[0], src[1], src[2], src[3], dst[0], dst[1], dst[2], dst[3], 0, protocol, len[0], len[1],
    ]
}

/// Compute a TCP/UDP checksum over pseudo-header + transport header + payload.
///
/// `checksum_offset` is the byte offset of the checksum field *within
/// `header`*; those two bytes are treated as zero, so the caller may pass a
/// header that already contains a (stale or received) checksum.
///
/// Returns `None` when `header.len() + payload.len()` exceeds `u16::MAX`,
/// because the pseudo-header's length field could not then be encoded and any
/// checksum produced would be silently wrong. Callers map this to their own
/// "too large" error rather than truncating.
pub fn transport_checksum(
    src: [u8; 4],
    dst: [u8; 4],
    protocol: u8,
    header: &[u8],
    checksum_offset: usize,
    payload: &[u8],
) -> Option<u16> {
    let total = header.len().checked_add(payload.len())?;
    if total > u16::MAX as usize {
        return None;
    }
    let mut c = Checksum::new();
    c.push(&pseudo_header(src, dst, protocol, total as u16));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 1071 §3 worked example: the sum of these bytes is 0xddf2, so the
    /// checksum is its complement, 0x220d.
    #[test]
    fn rfc1071_example() {
        let data = [0x00u8, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&data), 0x220d);
    }

    /// A real IPv4 header (checksum field already correct) must sum to zero.
    #[test]
    fn intact_ipv4_header_verifies() {
        let hdr = [
            0x45u8, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61, 0xc0, 0xa8,
            0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7,
        ];
        assert!(is_valid(&hdr));
        // And recomputing with the field zeroed reproduces the stored value.
        assert_eq!(checksum_with_zeroed_field(&hdr, 10), 0xb861);
    }

    #[test]
    fn single_bit_flip_is_detected() {
        let mut hdr = [
            0x45u8, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61, 0xc0, 0xa8,
            0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7,
        ];
        hdr[15] ^= 0x01;
        assert!(!is_valid(&hdr));
    }

    #[test]
    fn odd_length_pads_on_the_right() {
        // Three bytes: 0x1234 + 0x5600.
        assert_eq!(checksum(&[0x12, 0x34, 0x56]), !(0x1234u16 + 0x5600));
        // A trailing zero byte must not change the result.
        assert_eq!(
            checksum(&[0x12, 0x34, 0x56]),
            checksum(&[0x12, 0x34, 0x56, 0])
        );
    }

    #[test]
    fn empty_input_checksums_to_all_ones() {
        assert_eq!(checksum(&[]), 0xFFFF);
    }

    /// Splitting the byte stream at every possible boundary, including odd
    /// ones, must produce the identical checksum.
    #[test]
    fn chunked_push_matches_contiguous() {
        let data: [u8; 9] = [0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04, 0x05];
        let want = checksum(&data);
        for split in 0..=data.len() {
            let mut c = Checksum::new();
            c.push(&data[..split]);
            c.push(&data[split..]);
            assert_eq!(c.finish(), want, "split at {split}");
        }
    }

    #[test]
    fn three_way_odd_splits_match() {
        let data: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];
        let want = checksum(&data);
        let mut c = Checksum::new();
        c.push(&data[..1]);
        c.push(&data[1..4]);
        c.push(&data[4..]);
        assert_eq!(c.finish(), want);
    }

    #[test]
    fn carries_fold_around_rather_than_truncating() {
        // 0xFFFF + 0x0001 must fold to 0x0001, not truncate to 0x0000.
        let mut c = Checksum::new();
        c.push_u16(0xFFFF);
        c.push_u16(0x0001);
        assert_eq!(c.folded_sum(), 0x0001);
    }

    /// 64 KiB of 0xFF exercises the accumulator far past where a naive
    /// deferred-fold u32 sum would overflow.
    #[test]
    fn long_input_never_overflows() {
        let data = [0xFFu8; 65536];
        let mut c = Checksum::new();
        c.push(&data);
        // Sum of 32768 words of 0xFFFF folds to 0xFFFF; complement is 0.
        assert_eq!(c.finish(), 0);
    }

    #[test]
    fn zeroed_field_helper_tolerates_bad_offsets() {
        let data = [1u8, 2, 3, 4];
        // Offset past the end: falls back to a plain checksum, no panic.
        assert_eq!(checksum_with_zeroed_field(&data, 99), checksum(&data));
        // Offset where only one byte remains: also falls back.
        assert_eq!(checksum_with_zeroed_field(&data, 3), checksum(&data));
    }

    #[test]
    fn pseudo_header_layout() {
        let ph = pseudo_header([10, 0, 0, 1], [10, 0, 0, 2], 17, 0x1234);
        assert_eq!(
            ph,
            [10, 0, 0, 1, 10, 0, 0, 2, 0, 17, 0x12, 0x34],
            "zero pad, protocol, then big-endian length"
        );
    }

    /// Substituting the computed checksum back into the header must make the
    /// whole pseudo-header + header + payload sum verify as zero.
    #[test]
    fn udp_transport_checksum_round_trips() {
        let src = [192, 168, 0, 1];
        let dst = [192, 168, 0, 199];
        // src=53, dst=32795, len=0x5f, csum=0x2ad0
        let header = [0x00u8, 0x35, 0x80, 0x1b, 0x00, 0x5f, 0x2a, 0xd0];
        let payload = [0xAAu8; 87 - 8];
        let got = transport_checksum(src, dst, 17, &header, 6, &payload).unwrap();
        let mut with = header;
        with[6..8].copy_from_slice(&got.to_be_bytes());
        let mut c = Checksum::new();
        c.push(&pseudo_header(src, dst, 17, 87));
        c.push(&with);
        c.push(&payload);
        assert_eq!(c.finish(), 0);
    }

    /// The pseudo-header length field is 16 bits: a longer datagram cannot be
    /// checksummed correctly, so the guard must refuse rather than truncate.
    #[test]
    fn transport_checksum_rejects_oversize_length() {
        let header = [0u8; 8];
        let too_big = [0u8; 65530]; // 8 + 65530 = 65538 > u16::MAX
        assert!(transport_checksum([0; 4], [0; 4], 17, &header, 6, &too_big).is_none());
        let just_fits = [0u8; 65527]; // 8 + 65527 = 65535
        assert!(transport_checksum([0; 4], [0; 4], 17, &header, 6, &just_fits).is_some());
    }

    #[test]
    fn wrong_addresses_change_the_checksum() {
        let header = [0x00u8, 0x35, 0x80, 0x1b, 0x00, 0x0a, 0x00, 0x00];
        let a = transport_checksum([1, 2, 3, 4], [5, 6, 7, 8], 17, &header, 6, &[0, 1]).unwrap();
        let b = transport_checksum([1, 2, 3, 5], [5, 6, 7, 8], 17, &header, 6, &[0, 1]).unwrap();
        assert_ne!(
            a, b,
            "pseudo-header must bind the checksum to the addresses"
        );
    }
}
