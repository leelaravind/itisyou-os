//! ARP for IPv4 over Ethernet (RFC 826), restricted to exactly that binding.
//!
//! ARP is the one protocol on this path with no integrity story at all: a
//! reply is believed because it arrived. The parser therefore refuses
//! everything it is not certain about rather than being liberal — a wrong
//! hardware/protocol type, a length field that disagrees with the type, or an
//! opcode other than request/reply is an error, not something to skim past.
//! The *policy* of which replies to believe lives in the caller's cache; this
//! module's job is to make sure the caller never acts on a field it
//! misinterpreted.

use super::eth::{MacAddr, MAC_LEN};
use super::ipv4::Ipv4Addr;

/// Hardware type 1 = Ethernet. Nothing else is accepted.
pub const HTYPE_ETHERNET: u16 = 1;
/// Protocol type 0x0800 = IPv4. Nothing else is accepted.
pub const PTYPE_IPV4: u16 = 0x0800;

/// Length of an ARP-for-IPv4-over-Ethernet message. ARP has no length field of
/// its own; the sizes are implied by htype/ptype, which is exactly why those
/// must be validated before any offset is trusted.
pub const PACKET_LEN: usize = 28;

/// ARP operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Request,
    Reply,
}

impl Operation {
    pub const REQUEST: u16 = 1;
    pub const REPLY: u16 = 2;

    pub const fn as_u16(self) -> u16 {
        match self {
            Operation::Request => Operation::REQUEST,
            Operation::Reply => Operation::REPLY,
        }
    }
}

/// Why an ARP message was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpError {
    /// Fewer than [`PACKET_LEN`] bytes.
    TooShort,
    /// Hardware type is not Ethernet.
    BadHardwareType,
    /// Protocol type is not IPv4.
    BadProtocolType,
    /// hlen/plen disagree with the 6/4 that htype/ptype imply. A message that
    /// says "Ethernet" and then claims 8-byte hardware addresses is
    /// self-contradictory; parsing on would mean trusting one field over
    /// another for no reason.
    BadAddressLengths,
    /// Opcode is neither request (1) nor reply (2). RARP and InARP share this
    /// frame format and must not be silently treated as ARP.
    UnsupportedOperation,
    /// Destination buffer too small to build into.
    BufferTooSmall,
}

/// A validated ARP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArpPacket {
    pub operation: Operation,
    pub sender_mac: MacAddr,
    pub sender_ip: Ipv4Addr,
    pub target_mac: MacAddr,
    pub target_ip: Ipv4Addr,
}

impl ArpPacket {
    /// Does this message ask "who has `ip`?"
    pub fn is_request_for(&self, ip: Ipv4Addr) -> bool {
        self.operation == Operation::Request && self.target_ip == ip
    }

    /// The reply this host should send to a request for `ip`, answering with
    /// `mac`. Swapping sender/target is the whole of ARP's reply logic, and
    /// getting it backwards is the classic bug, so it lives here rather than
    /// in each caller.
    pub fn reply_with(&self, ip: Ipv4Addr, mac: MacAddr) -> ArpPacket {
        ArpPacket {
            operation: Operation::Reply,
            sender_mac: mac,
            sender_ip: ip,
            target_mac: self.sender_mac,
            target_ip: self.sender_ip,
        }
    }
}

/// Parse an ARP message from the payload of an Ethernet frame.
///
/// `bytes` may be longer than [`PACKET_LEN`] — an ARP message is 28 bytes
/// inside a 60-byte minimum frame, so padding is always present on the wire.
pub fn parse(bytes: &[u8]) -> Result<ArpPacket, ArpError> {
    if bytes.len() < PACKET_LEN {
        return Err(ArpError::TooShort);
    }
    let htype = u16::from_be_bytes([bytes[0], bytes[1]]);
    if htype != HTYPE_ETHERNET {
        return Err(ArpError::BadHardwareType);
    }
    let ptype = u16::from_be_bytes([bytes[2], bytes[3]]);
    if ptype != PTYPE_IPV4 {
        return Err(ArpError::BadProtocolType);
    }
    if bytes[4] as usize != MAC_LEN || bytes[5] != 4 {
        return Err(ArpError::BadAddressLengths);
    }
    let operation = match u16::from_be_bytes([bytes[6], bytes[7]]) {
        Operation::REQUEST => Operation::Request,
        Operation::REPLY => Operation::Reply,
        _ => return Err(ArpError::UnsupportedOperation),
    };
    // Every index below is inside the length checked above.
    Ok(ArpPacket {
        operation,
        sender_mac: MacAddr([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13],
        ]),
        sender_ip: Ipv4Addr([bytes[14], bytes[15], bytes[16], bytes[17]]),
        target_mac: MacAddr([
            bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]),
        target_ip: Ipv4Addr([bytes[24], bytes[25], bytes[26], bytes[27]]),
    })
}

/// Serialize an ARP message into `buf`. Returns [`PACKET_LEN`].
pub fn build_into(buf: &mut [u8], p: &ArpPacket) -> Result<usize, ArpError> {
    let out = buf.get_mut(..PACKET_LEN).ok_or(ArpError::BufferTooSmall)?;
    out[0..2].copy_from_slice(&HTYPE_ETHERNET.to_be_bytes());
    out[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
    out[4] = MAC_LEN as u8;
    out[5] = 4;
    out[6..8].copy_from_slice(&p.operation.as_u16().to_be_bytes());
    out[8..14].copy_from_slice(&p.sender_mac.0);
    out[14..18].copy_from_slice(&p.sender_ip.0);
    out[18..24].copy_from_slice(&p.target_mac.0);
    out[24..28].copy_from_slice(&p.target_ip.0);
    Ok(PACKET_LEN)
}

/// The request "who has `target_ip`? tell `sender_ip`".
///
/// The target hardware address is zero, not broadcast: it is the field being
/// asked about, and filling it with `ff:ff:ff:ff:ff:ff` (a common mistake,
/// since the *frame* does go to broadcast) makes some stacks reject the
/// request.
pub fn request(sender_mac: MacAddr, sender_ip: Ipv4Addr, target_ip: Ipv4Addr) -> ArpPacket {
    ArpPacket {
        operation: Operation::Request,
        sender_mac,
        sender_ip,
        target_mac: MacAddr::ZERO,
        target_ip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC_A: MacAddr = MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    const MAC_B: MacAddr = MacAddr([0x52, 0x55, 0x0A, 0x00, 0x02, 0x02]);
    const IP_A: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
    const IP_B: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);

    fn encoded(p: &ArpPacket) -> [u8; PACKET_LEN] {
        let mut buf = [0u8; PACKET_LEN];
        assert_eq!(build_into(&mut buf, p).unwrap(), PACKET_LEN);
        buf
    }

    #[test]
    fn request_round_trips() {
        let req = request(MAC_A, IP_A, IP_B);
        let wire = encoded(&req);
        assert_eq!(parse(&wire).unwrap(), req);
        assert_eq!(req.target_mac, MacAddr::ZERO);
        assert!(req.is_request_for(IP_B));
        assert!(!req.is_request_for(IP_A));
    }

    #[test]
    fn reply_swaps_sender_and_target() {
        let req = request(MAC_B, IP_B, IP_A);
        let reply = req.reply_with(IP_A, MAC_A);
        assert_eq!(reply.operation, Operation::Reply);
        // The reply's sender is us; its target is whoever asked.
        assert_eq!(reply.sender_mac, MAC_A);
        assert_eq!(reply.sender_ip, IP_A);
        assert_eq!(reply.target_mac, MAC_B);
        assert_eq!(reply.target_ip, IP_B);
        assert_eq!(parse(&encoded(&reply)).unwrap(), reply);
    }

    #[test]
    fn parses_padded_frames() {
        // A 28-byte message inside the 46-byte minimum Ethernet payload.
        let mut padded = [0u8; 46];
        padded[..PACKET_LEN].copy_from_slice(&encoded(&request(MAC_A, IP_A, IP_B)));
        assert_eq!(parse(&padded).unwrap().target_ip, IP_B);
    }

    #[test]
    fn rejects_truncation_at_every_boundary() {
        let wire = encoded(&request(MAC_A, IP_A, IP_B));
        for n in 0..PACKET_LEN {
            assert_eq!(parse(&wire[..n]), Err(ArpError::TooShort), "len {n}");
        }
        assert!(parse(&wire).is_ok());
    }

    #[test]
    fn rejects_wrong_hardware_and_protocol_types() {
        let mut wire = encoded(&request(MAC_A, IP_A, IP_B));
        wire[1] = 6; // token ring
        assert_eq!(parse(&wire), Err(ArpError::BadHardwareType));
        wire[1] = 1;
        wire[2..4].copy_from_slice(&0x86DDu16.to_be_bytes()); // IPv6
        assert_eq!(parse(&wire), Err(ArpError::BadProtocolType));
    }

    #[test]
    fn rejects_lengths_that_contradict_the_types() {
        let mut wire = encoded(&request(MAC_A, IP_A, IP_B));
        wire[4] = 8;
        assert_eq!(parse(&wire), Err(ArpError::BadAddressLengths));
        wire[4] = 6;
        wire[5] = 16;
        assert_eq!(parse(&wire), Err(ArpError::BadAddressLengths));
    }

    #[test]
    fn rejects_operations_that_are_not_arp() {
        let mut wire = encoded(&request(MAC_A, IP_A, IP_B));
        for op in [0u16, 3, 4, 8, 9, 0xFFFF] {
            wire[6..8].copy_from_slice(&op.to_be_bytes());
            assert_eq!(
                parse(&wire),
                Err(ArpError::UnsupportedOperation),
                "opcode {op}"
            );
        }
    }

    #[test]
    fn build_rejects_small_buffers() {
        let mut small = [0u8; PACKET_LEN - 1];
        assert_eq!(
            build_into(&mut small, &request(MAC_A, IP_A, IP_B)),
            Err(ArpError::BufferTooSmall)
        );
    }

    #[test]
    fn wire_layout_is_exact() {
        let wire = encoded(&request(MAC_A, IP_A, IP_B));
        assert_eq!(&wire[0..2], &[0x00, 0x01]);
        assert_eq!(&wire[2..4], &[0x08, 0x00]);
        assert_eq!(wire[4], 6);
        assert_eq!(wire[5], 4);
        assert_eq!(&wire[6..8], &[0x00, 0x01]);
        assert_eq!(&wire[8..14], &MAC_A.0);
        assert_eq!(&wire[14..18], &IP_A.0);
        assert_eq!(&wire[18..24], &[0u8; 6]);
        assert_eq!(&wire[24..28], &IP_B.0);
    }
}
