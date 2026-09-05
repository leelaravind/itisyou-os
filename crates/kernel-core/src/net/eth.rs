//! Ethernet II framing: MAC addresses, EtherType, and frame parse/build.
//!
//! The driver hands us whatever the NIC's receive ring produced, so this layer
//! assumes nothing: no minimum payload, no FCS (every controller this kernel
//! will drive strips it), and no trust in the length the hardware reported.
//! Parsing borrows from the caller's DMA buffer — nothing is copied — which is
//! what keeps the whole stack allocation-free.

/// Length of a MAC address in bytes.
pub const MAC_LEN: usize = 6;
/// Length of an untagged Ethernet II header (dst, src, ethertype).
pub const HEADER_LEN: usize = 14;
/// Smallest legal frame on the wire, excluding the FCS. Frames shorter than
/// this are padded by the sender (or its NIC); a receiver seeing less has
/// received a runt.
pub const MIN_FRAME_LEN: usize = 60;
/// Payload length that [`MIN_FRAME_LEN`] implies once the header is removed.
pub const MIN_PAYLOAD_LEN: usize = MIN_FRAME_LEN - HEADER_LEN;

/// Frame-level rejection reasons. Nothing here is recoverable: the caller
/// drops the frame and (optionally) counts the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EthError {
    /// Fewer than [`HEADER_LEN`] bytes — there is not even a header.
    HeaderTruncated,
    /// At least a header, but fewer than [`MIN_FRAME_LEN`] bytes total. A
    /// conforming sender always pads, so a runt means a damaged or hostile
    /// frame and is refused separately from [`EthError::HeaderTruncated`].
    Runt,
    /// 802.1Q/802.1ad tagged. The tag shifts the real EtherType by four bytes;
    /// rather than silently parsing the TPID as a protocol number we refuse,
    /// because this kernel's NIC is never configured for VLANs.
    VlanTagged,
    /// The destination buffer cannot hold the frame being built.
    BufferTooSmall,
    /// Payload longer than the largest frame we will emit.
    PayloadTooLarge,
}

/// Largest payload we will build into a frame (standard, non-jumbo MTU).
pub const MAX_PAYLOAD_LEN: usize = 1500;

/// A 48-bit Ethernet MAC address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacAddr(pub [u8; MAC_LEN]);

impl MacAddr {
    /// `ff:ff:ff:ff:ff:ff` — the broadcast destination used by ARP requests.
    pub const BROADCAST: MacAddr = MacAddr([0xFF; MAC_LEN]);
    /// `00:00:00:00:00:00` — "unknown", used as the ARP target hardware
    /// address in a request and never valid as a real source.
    pub const ZERO: MacAddr = MacAddr([0x00; MAC_LEN]);

    /// Build from a slice, rejecting any length other than six.
    pub fn from_slice(bytes: &[u8]) -> Option<MacAddr> {
        let arr: [u8; MAC_LEN] = bytes.try_into().ok()?;
        Some(MacAddr(arr))
    }

    pub const fn octets(&self) -> [u8; MAC_LEN] {
        self.0
    }

    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF; MAC_LEN]
    }

    /// Group bit (LSB of the first octet). Broadcast is a special case of
    /// multicast, so this is true for it as well.
    pub fn is_multicast(&self) -> bool {
        self.0[0] & 0x01 != 0
    }

    pub fn is_unicast(&self) -> bool {
        !self.is_multicast()
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0x00; MAC_LEN]
    }

    /// Locally-administered bit (bit 1 of the first octet). QEMU's default
    /// `52:54:00:...` addresses have it set.
    pub fn is_locally_administered(&self) -> bool {
        self.0[0] & 0x02 != 0
    }

    /// Render as `aa:bb:cc:dd:ee:ff` into a caller-supplied buffer and return
    /// the borrowed `&str`. Exists because the kernel's serial logger has no
    /// allocator: `Display` is available too, but this form lets a caller build
    /// a message without a formatting machine.
    pub fn format<'a>(&self, out: &'a mut [u8; 17]) -> &'a str {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (i, byte) in self.0.iter().enumerate() {
            let base = i * 3;
            out[base] = HEX[(byte >> 4) as usize];
            out[base + 1] = HEX[(byte & 0x0F) as usize];
            if i < MAC_LEN - 1 {
                out[base + 2] = b':';
            }
        }
        // Every byte was just written from the ASCII hex table, so this is
        // valid UTF-8 by construction.
        core::str::from_utf8(out).unwrap_or("??:??:??:??:??:??")
    }
}

impl core::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut buf = [0u8; 17];
        f.write_str(self.format(&mut buf))
    }
}

/// EtherType values this stack understands, plus a catch-all so a parser can
/// report what it saw instead of discarding the information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtherType {
    Ipv4,
    Arp,
    Ipv6,
    Other(u16),
}

impl EtherType {
    pub const IPV4: u16 = 0x0800;
    pub const ARP: u16 = 0x0806;
    pub const IPV6: u16 = 0x86DD;
    /// 802.1Q VLAN tag protocol identifier — recognised only so it can be
    /// rejected explicitly.
    pub const VLAN: u16 = 0x8100;
    /// 802.1ad service VLAN (QinQ) TPID.
    pub const VLAN_QINQ: u16 = 0x88A8;

    pub const fn from_u16(v: u16) -> EtherType {
        match v {
            Self::IPV4 => EtherType::Ipv4,
            Self::ARP => EtherType::Arp,
            Self::IPV6 => EtherType::Ipv6,
            other => EtherType::Other(other),
        }
    }

    pub const fn as_u16(self) -> u16 {
        match self {
            EtherType::Ipv4 => Self::IPV4,
            EtherType::Arp => Self::ARP,
            EtherType::Ipv6 => Self::IPV6,
            EtherType::Other(v) => v,
        }
    }
}

/// A parsed frame. `payload` borrows the receive buffer; it is the bytes after
/// the header, *including* any trailing pad the sender added to reach
/// [`MIN_FRAME_LEN`]. Stripping that pad requires the upper layer's own length
/// field (IPv4's total length, ARP's fixed 28 bytes), which is why this layer
/// deliberately does not try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub dst: MacAddr,
    pub src: MacAddr,
    pub ethertype: EtherType,
    pub payload: &'a [u8],
}

/// Parse a received frame.
///
/// `strict_runt` selects whether a frame between [`HEADER_LEN`] and
/// [`MIN_FRAME_LEN`] bytes is refused. Loopback and some virtio paths deliver
/// unpadded short frames, so the driver decides; the wire-facing path should
/// pass `true`.
pub fn parse(bytes: &[u8], strict_runt: bool) -> Result<Frame<'_>, EthError> {
    if bytes.len() < HEADER_LEN {
        return Err(EthError::HeaderTruncated);
    }
    if strict_runt && bytes.len() < MIN_FRAME_LEN {
        return Err(EthError::Runt);
    }
    let ethertype_raw = u16::from_be_bytes([bytes[12], bytes[13]]);
    if ethertype_raw == EtherType::VLAN || ethertype_raw == EtherType::VLAN_QINQ {
        return Err(EthError::VlanTagged);
    }
    Ok(Frame {
        // Both indices are inside the length checked above.
        dst: MacAddr([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]),
        src: MacAddr([bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11]]),
        ethertype: EtherType::from_u16(ethertype_raw),
        payload: &bytes[HEADER_LEN..],
    })
}

/// Write only the 14-byte header at the start of `buf`.
///
/// Separate from [`build_into`] so a driver can assemble Ethernet + IPv4 + UDP
/// headers directly into one transmit buffer and write the payload once.
pub fn build_header_into(
    buf: &mut [u8],
    dst: MacAddr,
    src: MacAddr,
    ethertype: EtherType,
) -> Result<usize, EthError> {
    let header = buf.get_mut(..HEADER_LEN).ok_or(EthError::BufferTooSmall)?;
    header[0..6].copy_from_slice(&dst.0);
    header[6..12].copy_from_slice(&src.0);
    header[12..14].copy_from_slice(&ethertype.as_u16().to_be_bytes());
    Ok(HEADER_LEN)
}

/// Build a complete frame into `buf`, returning the number of bytes written.
///
/// The frame is zero-padded up to [`MIN_FRAME_LEN`] when `pad` is set. Most
/// NICs pad in hardware, so the flag exists for the ones that do not; the pad
/// bytes are zero rather than uninitialised so nothing leaks from a reused
/// transmit buffer.
pub fn build_into(
    buf: &mut [u8],
    dst: MacAddr,
    src: MacAddr,
    ethertype: EtherType,
    payload: &[u8],
    pad: bool,
) -> Result<usize, EthError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(EthError::PayloadTooLarge);
    }
    let unpadded = HEADER_LEN + payload.len();
    let total = if pad {
        unpadded.max(MIN_FRAME_LEN)
    } else {
        unpadded
    };
    if buf.len() < total {
        return Err(EthError::BufferTooSmall);
    }
    build_header_into(buf, dst, src, ethertype)?;
    buf[HEADER_LEN..unpadded].copy_from_slice(payload);
    if total > unpadded {
        for b in &mut buf[unpadded..total] {
            *b = 0;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: MacAddr = MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    const DST: MacAddr = MacAddr([0x52, 0x54, 0x00, 0xAB, 0xCD, 0xEF]);

    fn frame_bytes(ethertype: u16, payload_len: usize) -> [u8; 64] {
        let mut f = [0u8; 64];
        f[0..6].copy_from_slice(&DST.0);
        f[6..12].copy_from_slice(&SRC.0);
        f[12..14].copy_from_slice(&ethertype.to_be_bytes());
        for (i, b) in f[HEADER_LEN..HEADER_LEN + payload_len]
            .iter_mut()
            .enumerate()
        {
            *b = i as u8;
        }
        f
    }

    #[test]
    fn parses_ipv4_frame() {
        let raw = frame_bytes(EtherType::IPV4, 20);
        let f = parse(&raw, true).unwrap();
        assert_eq!(f.dst, DST);
        assert_eq!(f.src, SRC);
        assert_eq!(f.ethertype, EtherType::Ipv4);
        assert_eq!(f.payload.len(), 64 - HEADER_LEN);
        assert_eq!(f.payload[0], 0);
        assert_eq!(f.payload[5], 5);
    }

    #[test]
    fn parses_arp_and_unknown_ethertypes() {
        let arp = frame_bytes(EtherType::ARP, 28);
        assert_eq!(parse(&arp, true).unwrap().ethertype, EtherType::Arp);
        let odd = frame_bytes(0x1234, 4);
        assert_eq!(
            parse(&odd, true).unwrap().ethertype,
            EtherType::Other(0x1234)
        );
        let v6 = frame_bytes(EtherType::IPV6, 4);
        assert_eq!(parse(&v6, true).unwrap().ethertype, EtherType::Ipv6);
    }

    /// Truncation at every boundary from empty up to a full header must be a
    /// clean error, never a panic.
    #[test]
    fn rejects_truncation_at_every_boundary() {
        let raw = frame_bytes(EtherType::IPV4, 20);
        for n in 0..HEADER_LEN {
            assert_eq!(
                parse(&raw[..n], true),
                Err(EthError::HeaderTruncated),
                "len {n}"
            );
            assert_eq!(
                parse(&raw[..n], false),
                Err(EthError::HeaderTruncated),
                "len {n}"
            );
        }
        // Exactly a header and nothing else is structurally fine but a runt.
        assert_eq!(parse(&raw[..HEADER_LEN], true), Err(EthError::Runt));
        assert_eq!(parse(&raw[..HEADER_LEN], false).unwrap().payload.len(), 0);
    }

    #[test]
    fn runt_boundary_is_exact() {
        let raw = frame_bytes(EtherType::IPV4, 50);
        assert_eq!(parse(&raw[..MIN_FRAME_LEN - 1], true), Err(EthError::Runt));
        assert!(parse(&raw[..MIN_FRAME_LEN], true).is_ok());
    }

    #[test]
    fn rejects_vlan_tags_rather_than_misreading_them() {
        let q = frame_bytes(EtherType::VLAN, 40);
        assert_eq!(parse(&q, true), Err(EthError::VlanTagged));
        let qinq = frame_bytes(EtherType::VLAN_QINQ, 40);
        assert_eq!(parse(&qinq, true), Err(EthError::VlanTagged));
    }

    #[test]
    fn mac_classification() {
        assert!(MacAddr::BROADCAST.is_broadcast());
        assert!(MacAddr::BROADCAST.is_multicast());
        assert!(!MacAddr::BROADCAST.is_unicast());
        assert!(SRC.is_unicast());
        assert!(!SRC.is_broadcast());
        assert!(SRC.is_locally_administered(), "52 has bit 1 set");
        assert!(MacAddr::ZERO.is_zero());
        assert!(MacAddr([0x01, 0, 0x5e, 0, 0, 1]).is_multicast());
    }

    #[test]
    fn mac_formatting() {
        let mut buf = [0u8; 17];
        assert_eq!(SRC.format(&mut buf), "52:54:00:12:34:56");
        let mut buf2 = [0u8; 17];
        assert_eq!(MacAddr::BROADCAST.format(&mut buf2), "ff:ff:ff:ff:ff:ff");
        let mut buf3 = [0u8; 17];
        assert_eq!(MacAddr::ZERO.format(&mut buf3), "00:00:00:00:00:00");
    }

    #[test]
    fn mac_from_slice_rejects_wrong_length() {
        assert_eq!(
            MacAddr::from_slice(&[1, 2, 3, 4, 5, 6]),
            Some(MacAddr([1, 2, 3, 4, 5, 6]))
        );
        assert!(MacAddr::from_slice(&[1, 2, 3, 4, 5]).is_none());
        assert!(MacAddr::from_slice(&[1, 2, 3, 4, 5, 6, 7]).is_none());
        assert!(MacAddr::from_slice(&[]).is_none());
    }

    #[test]
    fn builds_and_reparses() {
        let payload = [0xAAu8; 46];
        let mut buf = [0u8; 128];
        let n = build_into(&mut buf, DST, SRC, EtherType::Ipv4, &payload, true).unwrap();
        assert_eq!(n, HEADER_LEN + 46);
        let f = parse(&buf[..n], true).unwrap();
        assert_eq!(f.dst, DST);
        assert_eq!(f.src, SRC);
        assert_eq!(f.ethertype, EtherType::Ipv4);
        assert_eq!(f.payload, &payload[..]);
    }

    #[test]
    fn build_pads_short_payloads_with_zeros() {
        let mut buf = [0xFFu8; 128];
        let n = build_into(&mut buf, DST, SRC, EtherType::Arp, &[1, 2, 3], true).unwrap();
        assert_eq!(n, MIN_FRAME_LEN);
        assert_eq!(&buf[HEADER_LEN..HEADER_LEN + 3], &[1, 2, 3]);
        assert!(
            buf[HEADER_LEN + 3..n].iter().all(|&b| b == 0),
            "pad must be zeroed"
        );
    }

    #[test]
    fn build_without_padding_keeps_exact_length() {
        let mut buf = [0u8; 128];
        let n = build_into(&mut buf, DST, SRC, EtherType::Arp, &[1, 2, 3], false).unwrap();
        assert_eq!(n, HEADER_LEN + 3);
    }

    #[test]
    fn build_rejects_small_buffers_and_huge_payloads() {
        let mut small = [0u8; 13];
        assert_eq!(
            build_into(&mut small, DST, SRC, EtherType::Ipv4, &[], false),
            Err(EthError::BufferTooSmall)
        );
        let mut buf = [0u8; 40];
        // Padding demands 60 bytes even though header+payload is only 17.
        assert_eq!(
            build_into(&mut buf, DST, SRC, EtherType::Ipv4, &[0; 3], true),
            Err(EthError::BufferTooSmall)
        );
        let mut big = [0u8; 2048];
        assert_eq!(
            build_into(&mut big, DST, SRC, EtherType::Ipv4, &[0u8; 1501], false),
            Err(EthError::PayloadTooLarge)
        );
        assert!(build_into(&mut big, DST, SRC, EtherType::Ipv4, &[0u8; 1500], false).is_ok());
    }

    #[test]
    fn build_header_only_leaves_the_rest_untouched() {
        let mut buf = [0x5Au8; 32];
        assert_eq!(
            build_header_into(&mut buf, DST, SRC, EtherType::Arp).unwrap(),
            HEADER_LEN
        );
        assert_eq!(u16::from_be_bytes([buf[12], buf[13]]), EtherType::ARP);
        assert!(buf[HEADER_LEN..].iter().all(|&b| b == 0x5A));
        let mut tiny = [0u8; 13];
        assert_eq!(
            build_header_into(&mut tiny, DST, SRC, EtherType::Arp),
            Err(EthError::BufferTooSmall)
        );
    }

    #[test]
    fn ethertype_round_trips() {
        for raw in [0x0800u16, 0x0806, 0x86DD, 0x0000, 0xFFFF, 0x1234] {
            assert_eq!(EtherType::from_u16(raw).as_u16(), raw);
        }
    }
}
