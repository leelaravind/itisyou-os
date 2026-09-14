//! The kernel -> init mailbox records (V0.11, ACT11-002, ADR-0024).
//!
//! `/sbin/init` supervises the services; the kernel executes an approved
//! `retry-service` by asking init, never by starting a program behind init's
//! back (which would take the service's row away from its supervisor). The
//! mailbox holds one command; only the live init reads it (syscall 44
//! `init_ctl`) and acknowledges it, so the kernel knows what init did.
//!
//! ```text
//! command (24 bytes): seq u32, op u8 (1 retry, 2 stop), 3 × zero,
//!                     service name [16] NUL-padded ([a-z0-9-]{1,15})
//! ack     (16 bytes): seq u32, ok u8 (0/1), 3 × zero, pid u64
//! ```
//!
//! `retry`: start the Failed service again with its restart count reset;
//! the ack carries the new instance's pid. `stop`: stop supervising the
//! service (no more restarts) and report it Failed; the ack carries the pid
//! of a still-running instance, which the kernel then kills (init has no
//! authority to), or 0.

pub const COMMAND_LEN: usize = 24;
pub const ACK_LEN: usize = 16;
pub const NAME_LEN: usize = 16;

/// What the kernel asks init to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Retry = 1,
    Stop = 2,
}

impl Op {
    pub const fn name(self) -> &'static str {
        match self {
            Op::Retry => "retry",
            Op::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub seq: u32,
    pub op: Op,
    pub name: [u8; NAME_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub seq: u32,
    pub ok: bool,
    pub pid: u64,
}

/// Why a record was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtlError {
    BadLength,
    BadOp,
    BadName,
    BadPadding,
    BadFlag,
}

impl CtlError {
    pub const fn name(self) -> &'static str {
        match self {
            CtlError::BadLength => "bad_length",
            CtlError::BadOp => "bad_op",
            CtlError::BadName => "bad_name",
            CtlError::BadPadding => "bad_padding",
            CtlError::BadFlag => "bad_flag",
        }
    }
}

fn valid_name(b: &[u8]) -> bool {
    !b.is_empty()
        && b.len() < NAME_LEN
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl Command {
    /// A command for a service name, or `None` if the name is malformed.
    pub fn new(seq: u32, op: Op, name: &str) -> Option<Command> {
        if !valid_name(name.as_bytes()) {
            return None;
        }
        let mut n = [0u8; NAME_LEN];
        n[..name.len()].copy_from_slice(name.as_bytes());
        Some(Command { seq, op, name: n })
    }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    pub fn encode(&self) -> [u8; COMMAND_LEN] {
        let mut b = [0u8; COMMAND_LEN];
        b[..4].copy_from_slice(&self.seq.to_le_bytes());
        b[4] = self.op as u8;
        b[8..24].copy_from_slice(&self.name);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Command, CtlError> {
        if b.len() != COMMAND_LEN {
            return Err(CtlError::BadLength);
        }
        let op = match b[4] {
            1 => Op::Retry,
            2 => Op::Stop,
            _ => return Err(CtlError::BadOp),
        };
        if b[5..8].iter().any(|&x| x != 0) {
            return Err(CtlError::BadPadding);
        }
        let name = &b[8..24];
        let end = name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
        if end == NAME_LEN || !valid_name(&name[..end]) || name[end..].iter().any(|&c| c != 0) {
            return Err(CtlError::BadName);
        }
        let mut n = [0u8; NAME_LEN];
        n.copy_from_slice(name);
        Ok(Command {
            seq: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            op,
            name: n,
        })
    }
}

impl Ack {
    pub fn encode(&self) -> [u8; ACK_LEN] {
        let mut b = [0u8; ACK_LEN];
        b[..4].copy_from_slice(&self.seq.to_le_bytes());
        b[4] = u8::from(self.ok);
        b[8..16].copy_from_slice(&self.pid.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8]) -> Result<Ack, CtlError> {
        if b.len() != ACK_LEN {
            return Err(CtlError::BadLength);
        }
        if b[4] > 1 {
            return Err(CtlError::BadFlag);
        }
        if b[5..8].iter().any(|&x| x != 0) {
            return Err(CtlError::BadPadding);
        }
        let mut pid = [0u8; 8];
        pid.copy_from_slice(&b[8..16]);
        Ok(Ack {
            seq: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            ok: b[4] == 1,
            pid: u64::from_le_bytes(pid),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        for (op, name) in [
            (Op::Retry, "flapd"),
            (Op::Stop, "a"),
            (Op::Retry, "abcdefghijklmno"),
        ] {
            let c = Command::new(7, op, name).unwrap();
            assert_eq!(Command::decode(&c.encode()), Ok(c));
            assert_eq!(c.name_str(), name);
        }
        for ack in [
            Ack {
                seq: 1,
                ok: true,
                pid: 9,
            },
            Ack {
                seq: u32::MAX,
                ok: false,
                pid: 0,
            },
        ] {
            assert_eq!(Ack::decode(&ack.encode()), Ok(ack));
        }
        assert!(Command::new(1, Op::Retry, "").is_none());
        assert!(Command::new(1, Op::Retry, "Flapd").is_none());
        assert!(Command::new(1, Op::Retry, &"a".repeat(16)).is_none());
    }

    #[test]
    fn hostile_records_are_refused_by_name() {
        let good = Command::new(3, Op::Stop, "flapd").unwrap().encode();
        let bad = |f: &dyn Fn(&mut [u8; COMMAND_LEN])| {
            let mut b = good;
            f(&mut b);
            Command::decode(&b)
        };
        assert_eq!(Command::decode(&good[..23]), Err(CtlError::BadLength));
        assert_eq!(bad(&|b| b[4] = 0), Err(CtlError::BadOp));
        assert_eq!(bad(&|b| b[4] = 3), Err(CtlError::BadOp));
        assert_eq!(bad(&|b| b[6] = 1), Err(CtlError::BadPadding));
        assert_eq!(bad(&|b| b[8] = b'['), Err(CtlError::BadName));
        assert_eq!(bad(&|b| b[8] = 0), Err(CtlError::BadName));
        assert_eq!(bad(&|b| b[20] = b'x'), Err(CtlError::BadName));
        let ack = Ack {
            seq: 3,
            ok: true,
            pid: 4,
        }
        .encode();
        assert_eq!(Ack::decode(&ack[..15]), Err(CtlError::BadLength));
        let mut b = ack;
        b[4] = 2;
        assert_eq!(Ack::decode(&b), Err(CtlError::BadFlag));
        let mut b = ack;
        b[7] = 1;
        assert_eq!(Ack::decode(&b), Err(CtlError::BadPadding));
    }
}
