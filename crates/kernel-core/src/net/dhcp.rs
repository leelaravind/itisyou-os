//! DHCPv4 client messages (RFC 2131 / RFC 2132), V0.9.
//!
//! Only the client side of the four-message exchange is built here —
//! DISCOVER and REQUEST — and only the server's OFFER, ACK and NAK are parsed.
//! A server's reply is untrusted input like any other frame: the option walk
//! is bounded by the buffer, every option length is checked before it is
//! read, a reply for a different transaction or hardware address is refused,
//! and a lease without the fields a host needs to configure itself (address,
//! server identity) is refused rather than half-applied.

use super::eth::MacAddr;
use super::ipv4::Ipv4Addr;

pub const CLIENT_PORT: u16 = 68;
pub const SERVER_PORT: u16 = 67;

/// Fixed BOOTP header (236 bytes) plus the 4-byte magic cookie.
const FIXED_LEN: usize = 236;
const COOKIE: [u8; 4] = [99, 130, 83, 99];
const OPTIONS_AT: usize = FIXED_LEN + 4;

/// Largest message this client builds (fixed part + a few options).
pub const MAX_MESSAGE_LEN: usize = 300;

const OP_REQUEST: u8 = 1;
const OP_REPLY: u8 = 2;
const HTYPE_ETHERNET: u8 = 1;
const FLAG_BROADCAST: u16 = 0x8000;

mod opt {
    pub const PAD: u8 = 0;
    pub const SUBNET_MASK: u8 = 1;
    pub const ROUTER: u8 = 3;
    pub const DNS: u8 = 6;
    pub const REQUESTED_IP: u8 = 50;
    pub const LEASE_TIME: u8 = 51;
    pub const MESSAGE_TYPE: u8 = 53;
    pub const SERVER_ID: u8 = 54;
    pub const PARAM_REQUEST: u8 = 55;
    pub const RENEWAL_T1: u8 = 58;
    pub const REBINDING_T2: u8 = 59;
    pub const END: u8 = 255;
}

/// DHCP message types this client sends or understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Ack = 5,
    Nak = 6,
}

impl MessageType {
    fn from_u8(v: u8) -> Option<MessageType> {
        match v {
            1 => Some(MessageType::Discover),
            2 => Some(MessageType::Offer),
            3 => Some(MessageType::Request),
            5 => Some(MessageType::Ack),
            6 => Some(MessageType::Nak),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpError {
    Truncated,
    BufferTooSmall,
    NotAReply,
    WrongHardware,
    BadCookie,
    /// An option's declared length runs past the message.
    BadOption,
    WrongTransaction,
    WrongClient,
    MissingMessageType,
    UnexpectedMessageType,
    /// An OFFER/ACK without the fields a host needs (address, server id).
    IncompleteLease,
}

/// What a server offered or acknowledged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub kind: MessageType,
    pub address: Ipv4Addr,
    pub server: Ipv4Addr,
    pub netmask: Option<Ipv4Addr>,
    pub router: Option<Ipv4Addr>,
    pub dns: Option<Ipv4Addr>,
    /// Seconds; `None` when the server did not say (a NAK carries none).
    pub lease_secs: Option<u32>,
    pub renew_secs: Option<u32>,
    pub rebind_secs: Option<u32>,
}

impl Lease {
    /// T1: when to start renewing — the server's value, or half the lease
    /// (RFC 2131 §4.4.5).
    pub fn renew_after(&self) -> Option<u32> {
        self.renew_secs.or(self.lease_secs.map(|l| l / 2))
    }
}

fn header(out: &mut [u8], xid: u32, mac: MacAddr) {
    out[..OPTIONS_AT].fill(0);
    out[0] = OP_REQUEST;
    out[1] = HTYPE_ETHERNET;
    out[2] = 6;
    out[4..8].copy_from_slice(&xid.to_be_bytes());
    // Ask for broadcast replies: until it has an address the client cannot
    // promise to accept a unicast to one it does not own yet.
    out[10..12].copy_from_slice(&FLAG_BROADCAST.to_be_bytes());
    out[28..34].copy_from_slice(&mac.0);
    out[FIXED_LEN..OPTIONS_AT].copy_from_slice(&COOKIE);
}

fn push(out: &mut [u8], at: &mut usize, code: u8, data: &[u8]) {
    out[*at] = code;
    out[*at + 1] = data.len() as u8;
    out[*at + 2..*at + 2 + data.len()].copy_from_slice(data);
    *at += 2 + data.len();
}

const PARAMS: [u8; 5] = [
    opt::SUBNET_MASK,
    opt::ROUTER,
    opt::DNS,
    opt::LEASE_TIME,
    opt::RENEWAL_T1,
];

/// Build a DHCPDISCOVER. Returns its length.
pub fn build_discover(out: &mut [u8], xid: u32, mac: MacAddr) -> Result<usize, DhcpError> {
    if out.len() < MAX_MESSAGE_LEN {
        return Err(DhcpError::BufferTooSmall);
    }
    header(out, xid, mac);
    let mut at = OPTIONS_AT;
    push(
        out,
        &mut at,
        opt::MESSAGE_TYPE,
        &[MessageType::Discover as u8],
    );
    push(out, &mut at, opt::PARAM_REQUEST, &PARAMS);
    out[at] = opt::END;
    Ok(at + 1)
}

/// Build a DHCPREQUEST for the address `offer` carried, naming its server.
pub fn build_request(
    out: &mut [u8],
    xid: u32,
    mac: MacAddr,
    offer: &Lease,
) -> Result<usize, DhcpError> {
    if out.len() < MAX_MESSAGE_LEN {
        return Err(DhcpError::BufferTooSmall);
    }
    header(out, xid, mac);
    let mut at = OPTIONS_AT;
    push(
        out,
        &mut at,
        opt::MESSAGE_TYPE,
        &[MessageType::Request as u8],
    );
    push(out, &mut at, opt::REQUESTED_IP, &offer.address.octets());
    push(out, &mut at, opt::SERVER_ID, &offer.server.octets());
    push(out, &mut at, opt::PARAM_REQUEST, &PARAMS);
    out[at] = opt::END;
    Ok(at + 1)
}

fn addr(data: &[u8]) -> Option<Ipv4Addr> {
    (data.len() >= 4).then(|| Ipv4Addr::new(data[0], data[1], data[2], data[3]))
}

fn secs(data: &[u8]) -> Option<u32> {
    (data.len() == 4).then(|| u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
}

/// Parse a server reply for transaction `xid` addressed to `mac`.
pub fn parse_reply(bytes: &[u8], xid: u32, mac: MacAddr) -> Result<Lease, DhcpError> {
    if bytes.len() < OPTIONS_AT + 1 {
        return Err(DhcpError::Truncated);
    }
    if bytes[0] != OP_REPLY {
        return Err(DhcpError::NotAReply);
    }
    if bytes[1] != HTYPE_ETHERNET || bytes[2] != 6 {
        return Err(DhcpError::WrongHardware);
    }
    if u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) != xid {
        return Err(DhcpError::WrongTransaction);
    }
    if bytes[28..34] != mac.0 {
        return Err(DhcpError::WrongClient);
    }
    if bytes[FIXED_LEN..OPTIONS_AT] != COOKIE {
        return Err(DhcpError::BadCookie);
    }
    let address = Ipv4Addr::new(bytes[16], bytes[17], bytes[18], bytes[19]);
    let mut lease = Lease {
        kind: MessageType::Discover, // placeholder until option 53 is seen
        address,
        server: Ipv4Addr::UNSPECIFIED,
        netmask: None,
        router: None,
        dns: None,
        lease_secs: None,
        renew_secs: None,
        rebind_secs: None,
    };
    let mut kind = None;
    let mut at = OPTIONS_AT;
    while at < bytes.len() {
        let code = bytes[at];
        if code == opt::PAD {
            at += 1;
            continue;
        }
        if code == opt::END {
            break;
        }
        let len = *bytes.get(at + 1).ok_or(DhcpError::BadOption)? as usize;
        let data = bytes
            .get(at + 2..at + 2 + len)
            .ok_or(DhcpError::BadOption)?;
        match code {
            opt::MESSAGE_TYPE if len == 1 => kind = MessageType::from_u8(data[0]),
            opt::SERVER_ID => lease.server = addr(data).ok_or(DhcpError::BadOption)?,
            opt::SUBNET_MASK => lease.netmask = addr(data),
            // Router and DNS options may list several; the first is used.
            opt::ROUTER => lease.router = addr(data),
            opt::DNS => lease.dns = addr(data),
            opt::LEASE_TIME => lease.lease_secs = secs(data),
            opt::RENEWAL_T1 => lease.renew_secs = secs(data),
            opt::REBINDING_T2 => lease.rebind_secs = secs(data),
            _ => {}
        }
        at += 2 + len;
    }
    let kind = kind.ok_or(DhcpError::MissingMessageType)?;
    match kind {
        MessageType::Offer | MessageType::Ack => {
            if address.is_unspecified() || address.is_broadcast() || lease.server.is_unspecified() {
                return Err(DhcpError::IncompleteLease);
            }
        }
        MessageType::Nak => {}
        _ => return Err(DhcpError::UnexpectedMessageType),
    }
    lease.kind = kind;
    Ok(lease)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    const MAC: MacAddr = MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    const XID: u32 = 0x1234_5678;

    /// A reply shaped like QEMU user-mode networking's: 10.0.2.15 from
    /// 10.0.2.2, /24, router 10.0.2.2, DNS 10.0.2.3, 24 h lease.
    fn reply(kind: u8) -> Vec<u8> {
        let mut m = std::vec![0u8; OPTIONS_AT];
        m[0] = OP_REPLY;
        m[1] = HTYPE_ETHERNET;
        m[2] = 6;
        m[4..8].copy_from_slice(&XID.to_be_bytes());
        m[16..20].copy_from_slice(&[10, 0, 2, 15]);
        m[28..34].copy_from_slice(&MAC.0);
        m[FIXED_LEN..OPTIONS_AT].copy_from_slice(&COOKIE);
        m.extend_from_slice(&[opt::MESSAGE_TYPE, 1, kind]);
        m.extend_from_slice(&[opt::SERVER_ID, 4, 10, 0, 2, 2]);
        m.extend_from_slice(&[opt::SUBNET_MASK, 4, 255, 255, 255, 0]);
        m.extend_from_slice(&[opt::ROUTER, 4, 10, 0, 2, 2]);
        m.extend_from_slice(&[opt::DNS, 8, 10, 0, 2, 3, 8, 8, 8, 8]);
        m.extend_from_slice(&[opt::LEASE_TIME, 4, 0, 1, 0x51, 0x80]);
        m.push(opt::PAD);
        m.push(opt::END);
        m
    }

    #[test]
    fn discover_and_request_have_the_rfc_shape() {
        let mut buf = [0u8; MAX_MESSAGE_LEN];
        let n = build_discover(&mut buf, XID, MAC).unwrap();
        assert_eq!((buf[0], buf[1], buf[2]), (OP_REQUEST, 1, 6));
        assert_eq!(&buf[4..8], &XID.to_be_bytes());
        assert_eq!(&buf[10..12], &FLAG_BROADCAST.to_be_bytes());
        assert_eq!(&buf[28..34], &MAC.0);
        assert_eq!(&buf[FIXED_LEN..OPTIONS_AT], &COOKIE);
        assert_eq!(&buf[OPTIONS_AT..OPTIONS_AT + 3], &[53, 1, 1]);
        assert_eq!(buf[n - 1], opt::END);

        let offer = parse_reply(&reply(2), XID, MAC).unwrap();
        let n = build_request(&mut buf, XID, MAC, &offer).unwrap();
        let options = &buf[OPTIONS_AT..n];
        assert_eq!(&options[..3], &[53, 1, 3]);
        assert!(options.windows(6).any(|w| w == [50, 4, 10, 0, 2, 15]));
        assert!(options.windows(6).any(|w| w == [54, 4, 10, 0, 2, 2]));
    }

    #[test]
    fn parses_an_offer_and_an_ack_like_qemu_sends() {
        let offer = parse_reply(&reply(2), XID, MAC).unwrap();
        assert_eq!(offer.kind, MessageType::Offer);
        assert_eq!(offer.address, Ipv4Addr::new(10, 0, 2, 15));
        assert_eq!(offer.server, Ipv4Addr::new(10, 0, 2, 2));
        assert_eq!(offer.netmask, Some(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(offer.router, Some(Ipv4Addr::new(10, 0, 2, 2)));
        assert_eq!(offer.dns, Some(Ipv4Addr::new(10, 0, 2, 3)));
        assert_eq!(offer.lease_secs, Some(86_400));
        assert_eq!(offer.renew_after(), Some(43_200));
        assert_eq!(
            parse_reply(&reply(5), XID, MAC).unwrap().kind,
            MessageType::Ack
        );
    }

    #[test]
    fn refuses_replies_for_someone_else() {
        assert_eq!(
            parse_reply(&reply(2), XID ^ 1, MAC),
            Err(DhcpError::WrongTransaction)
        );
        let other = MacAddr([0x52, 0x54, 0, 0, 0, 1]);
        assert_eq!(
            parse_reply(&reply(2), XID, other),
            Err(DhcpError::WrongClient)
        );
        let mut req = reply(2);
        req[0] = OP_REQUEST;
        assert_eq!(parse_reply(&req, XID, MAC), Err(DhcpError::NotAReply));
    }

    #[test]
    fn refuses_malformed_options_and_cookies() {
        let mut bad_cookie = reply(2);
        bad_cookie[FIXED_LEN] = 0;
        assert_eq!(
            parse_reply(&bad_cookie, XID, MAC),
            Err(DhcpError::BadCookie)
        );

        let mut overrun = reply(2);
        let len = overrun.len();
        overrun[len - 1] = opt::DNS; // replace END with an option that claims more bytes than exist
        overrun.push(40);
        overrun.push(10);
        assert_eq!(parse_reply(&overrun, XID, MAC), Err(DhcpError::BadOption));

        assert_eq!(
            parse_reply(&reply(2)[..OPTIONS_AT], XID, MAC),
            Err(DhcpError::Truncated)
        );
    }

    #[test]
    fn refuses_leases_a_host_could_not_use() {
        let mut no_type = reply(2);
        no_type[OPTIONS_AT] = opt::PAD;
        no_type[OPTIONS_AT + 1] = opt::PAD;
        no_type[OPTIONS_AT + 2] = opt::PAD;
        assert_eq!(
            parse_reply(&no_type, XID, MAC),
            Err(DhcpError::MissingMessageType)
        );

        let mut no_address = reply(2);
        no_address[16..20].fill(0);
        assert_eq!(
            parse_reply(&no_address, XID, MAC),
            Err(DhcpError::IncompleteLease)
        );

        let mut discover_echo = reply(2);
        discover_echo[OPTIONS_AT + 2] = MessageType::Discover as u8;
        assert_eq!(
            parse_reply(&discover_echo, XID, MAC),
            Err(DhcpError::UnexpectedMessageType)
        );

        // A NAK carries no lease but is a valid, meaningful answer.
        let mut nak = reply(6);
        nak[16..20].fill(0);
        assert_eq!(parse_reply(&nak, XID, MAC).unwrap().kind, MessageType::Nak);
    }
}
