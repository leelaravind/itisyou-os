//! Supervisor report (V0.10): the fixed 32-byte record `/sbin/init` sends to
//! describe one service lifecycle event.
//!
//! ```text
//! offset size field
//!      0    1 event         1 start, 2 restart, 3 done, 4 failed, 5 ready
//!      1    1 long_running  0 or 1
//!      2    1 clean_exit    0 or 1
//!      3    1 pad           0
//!      4    4 restarts      u32 LE, at most service::RESTART_LIMIT
//!      8    8 pid           u64 LE
//!     16   16 name          [a-z0-9-]{1,15}, NUL-padded
//! ```
//!
//! The record crosses a privilege boundary, so [`decode`] is strict: exactly
//! 32 bytes, known event, flags that are exactly 0 or 1, a zero pad byte, a
//! restart count within the ceiling, and a name whose padding is all NUL.
//! Anything else is refused with a stable [`ReportError`] — nothing is
//! clamped or ignored. Pure and allocation-free.

use crate::service::RESTART_LIMIT;

/// Size of one encoded report.
pub const REPORT_LEN: usize = 32;

/// Size of the NUL-padded name field.
pub const NAME_FIELD: usize = 16;

/// Longest service name; the name field always keeps at least one NUL.
pub const MAX_NAME: usize = NAME_FIELD - 1;

const NAME_OFFSET: usize = REPORT_LEN - NAME_FIELD;

/// A lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Start,
    Restart,
    Done,
    Failed,
    Ready,
}

impl Event {
    /// Every event, in wire-code order.
    pub const ALL: [Event; 5] = [
        Event::Start,
        Event::Restart,
        Event::Done,
        Event::Failed,
        Event::Ready,
    ];

    /// Wire code (byte 0).
    pub const fn code(self) -> u8 {
        match self {
            Event::Start => 1,
            Event::Restart => 2,
            Event::Done => 3,
            Event::Failed => 4,
            Event::Ready => 5,
        }
    }

    /// The event for a wire code, if it is one.
    pub const fn from_code(code: u8) -> Option<Event> {
        match code {
            1 => Some(Event::Start),
            2 => Some(Event::Restart),
            3 => Some(Event::Done),
            4 => Some(Event::Failed),
            5 => Some(Event::Ready),
            _ => None,
        }
    }

    /// Stable short name (serial evidence).
    pub const fn name(self) -> &'static str {
        match self {
            Event::Start => "start",
            Event::Restart => "restart",
            Event::Done => "done",
            Event::Failed => "failed",
            Event::Ready => "ready",
        }
    }
}

/// Why a report was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportError {
    /// Not exactly [`REPORT_LEN`] bytes (short, or trailing bytes).
    Truncated,
    /// Event code outside 1..=5.
    BadEvent,
    /// A flag byte that is not 0 or 1, or a nonzero pad byte.
    BadFlag,
    /// Empty, too long, a byte outside `[a-z0-9-]`, or non-NUL padding.
    BadName,
    /// More restarts than [`RESTART_LIMIT`] allows.
    TooManyRestarts,
}

impl ReportError {
    /// Stable short name, used in console messages and serial evidence.
    pub const fn name(self) -> &'static str {
        match self {
            ReportError::Truncated => "truncated",
            ReportError::BadEvent => "bad_event",
            ReportError::BadFlag => "bad_flag",
            ReportError::BadName => "bad_name",
            ReportError::TooManyRestarts => "too_many_restarts",
        }
    }
}

/// A validated service name: 1 to [`MAX_NAME`] bytes of `[a-z0-9-]`.
/// The same rule `/etc/init.conf` applies to `service <name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceName {
    /// The name, NUL-padded — exactly the wire field.
    field: [u8; NAME_FIELD],
    len: u8,
}

impl ServiceName {
    /// True for a well-formed service name.
    pub const fn is_valid(name: &[u8]) -> bool {
        if name.is_empty() || name.len() > MAX_NAME {
            return false;
        }
        let mut i = 0;
        while i < name.len() {
            let b = name[i];
            if !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Validate `name`.
    pub fn new(name: &str) -> Result<ServiceName, ReportError> {
        let bytes = name.as_bytes();
        if !ServiceName::is_valid(bytes) {
            return Err(ReportError::BadName);
        }
        let mut field = [0u8; NAME_FIELD];
        field[..bytes.len()].copy_from_slice(bytes);
        Ok(ServiceName {
            field,
            len: bytes.len() as u8,
        })
    }

    /// Parse a NUL-padded wire field.
    fn from_field(field: &[u8]) -> Result<ServiceName, ReportError> {
        let len = field
            .iter()
            .position(|&b| b == 0)
            .ok_or(ReportError::BadName)?;
        if field[len..].iter().any(|&b| b != 0) {
            return Err(ReportError::BadName);
        }
        match core::str::from_utf8(&field[..len]) {
            Ok(name) => ServiceName::new(name),
            Err(_) => Err(ReportError::BadName),
        }
    }

    /// The name's bytes (no padding).
    pub fn as_bytes(&self) -> &[u8] {
        &self.field[..usize::from(self.len)]
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        // Only `new` builds a ServiceName, and it admits ASCII alone.
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

/// One supervisor report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub event: Event,
    /// The service runs under the long-running (`restart=always`) policy.
    pub long_running: bool,
    /// The termination that caused this event was `exit(0)`.
    pub clean_exit: bool,
    /// Restarts performed so far for this service.
    pub restarts: u32,
    pub pid: u64,
    pub name: ServiceName,
}

/// Encode `report` as its 32-byte wire form.
///
/// Encoding does not validate `restarts`; a report whose count exceeds
/// [`RESTART_LIMIT`] encodes, and [`decode`] refuses it.
pub fn encode(report: &Report) -> [u8; REPORT_LEN] {
    let mut out = [0u8; REPORT_LEN];
    out[0] = report.event.code();
    out[1] = u8::from(report.long_running);
    out[2] = u8::from(report.clean_exit);
    out[3] = 0;
    out[4..8].copy_from_slice(&report.restarts.to_le_bytes());
    out[8..16].copy_from_slice(&report.pid.to_le_bytes());
    out[NAME_OFFSET..].copy_from_slice(&report.name.field);
    out
}

fn flag(b: u8) -> Result<bool, ReportError> {
    match b {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ReportError::BadFlag),
    }
}

/// Decode and validate one report. Fields are checked in wire order, so the
/// error names the first bad field.
pub fn decode(bytes: &[u8]) -> Result<Report, ReportError> {
    let bytes: &[u8; REPORT_LEN] = bytes.try_into().map_err(|_| ReportError::Truncated)?;
    let event = Event::from_code(bytes[0]).ok_or(ReportError::BadEvent)?;
    let long_running = flag(bytes[1])?;
    let clean_exit = flag(bytes[2])?;
    if bytes[3] != 0 {
        return Err(ReportError::BadFlag);
    }
    let restarts = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if restarts > RESTART_LIMIT {
        return Err(ReportError::TooManyRestarts);
    }
    let mut pid = [0u8; 8];
    pid.copy_from_slice(&bytes[8..16]);
    let name = ServiceName::from_field(&bytes[NAME_OFFSET..])?;
    Ok(Report {
        event,
        long_running,
        clean_exit,
        restarts,
        pid: u64::from_le_bytes(pid),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(event: Event, name: &str) -> Report {
        Report {
            event,
            long_running: true,
            clean_exit: false,
            restarts: 2,
            pid: 0x0102_0304_0506_0708,
            name: ServiceName::new(name).unwrap(),
        }
    }

    #[test]
    fn wire_layout_is_fixed_little_endian() {
        let bytes = encode(&report(Event::Restart, "tickd"));
        let mut expected = [0u8; REPORT_LEN];
        expected[0] = 2;
        expected[1] = 1;
        expected[2] = 0;
        expected[4..8].copy_from_slice(&[2, 0, 0, 0]);
        expected[8..16].copy_from_slice(&[8, 7, 6, 5, 4, 3, 2, 1]);
        expected[16..21].copy_from_slice(b"tickd");
        assert_eq!(bytes, expected);
    }

    #[test]
    fn round_trips_every_event_flag_count_and_pid() {
        for event in Event::ALL {
            assert_eq!(Event::from_code(event.code()), Some(event));
            for long_running in [false, true] {
                for clean_exit in [false, true] {
                    for restarts in 0..=RESTART_LIMIT {
                        for pid in [0, 1, 4096, u64::MAX] {
                            for name in ["a", "flapd", "0-9", "abcdefghijklmno"] {
                                let r = Report {
                                    event,
                                    long_running,
                                    clean_exit,
                                    restarts,
                                    pid,
                                    name: ServiceName::new(name).unwrap(),
                                };
                                let decoded = decode(&encode(&r)).unwrap();
                                assert_eq!(decoded, r);
                                assert_eq!(decoded.name.as_str(), name);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn truncation_at_every_length_is_refused() {
        let bytes = encode(&report(Event::Start, "tickd"));
        for len in 0..REPORT_LEN {
            assert_eq!(decode(&bytes[..len]), Err(ReportError::Truncated), "{len}");
        }
        assert!(decode(&bytes).is_ok());
        // Trailing bytes are refused too: a report is exactly one record.
        let mut long = bytes.to_vec();
        long.push(0);
        assert_eq!(decode(&long), Err(ReportError::Truncated));
    }

    #[test]
    fn refuses_unknown_events() {
        let good = encode(&report(Event::Ready, "tickd"));
        for code in [0u8, 6, 7, 0x80, 0xFF] {
            let mut bytes = good;
            bytes[0] = code;
            assert_eq!(decode(&bytes), Err(ReportError::BadEvent), "{code}");
        }
    }

    #[test]
    fn refuses_non_boolean_flags_and_nonzero_pad() {
        let good = encode(&report(Event::Done, "tickd"));
        for (offset, value) in [(1, 2u8), (1, 0xFF), (2, 2), (2, 0x80), (3, 1), (3, 0xFF)] {
            let mut bytes = good;
            bytes[offset] = value;
            assert_eq!(
                decode(&bytes),
                Err(ReportError::BadFlag),
                "{offset}={value}"
            );
        }
    }

    #[test]
    fn refuses_restart_counts_above_the_ceiling() {
        let mut r = report(Event::Failed, "flapd");
        r.restarts = RESTART_LIMIT;
        assert!(decode(&encode(&r)).is_ok());
        for restarts in [RESTART_LIMIT + 1, 0x100, u32::MAX] {
            r.restarts = restarts;
            assert_eq!(
                decode(&encode(&r)),
                Err(ReportError::TooManyRestarts),
                "{restarts}"
            );
        }
    }

    #[test]
    fn name_bounds_and_alphabet() {
        // 15 bytes is the longest name; 16 does not fit with its NUL.
        assert_eq!(
            ServiceName::new("abcdefghijklmno").unwrap().as_str(),
            "abcdefghijklmno"
        );
        assert_eq!(
            ServiceName::new("abcdefghijklmnop"),
            Err(ReportError::BadName)
        );
        for bad in [
            "", "Tickd", "tick_d", "tick.d", "tick d", "/bin", "é", "a\0",
        ] {
            assert_eq!(ServiceName::new(bad), Err(ReportError::BadName), "{bad:?}");
            assert!(!ServiceName::is_valid(bad.as_bytes()));
        }
        assert!(ServiceName::is_valid(b"-"));
        assert!(ServiceName::is_valid(b"svc-2"));

        let good = encode(&report(Event::Start, "tickd"));
        // A 16-byte name with no NUL.
        let mut bytes = good;
        bytes[16..].copy_from_slice(b"abcdefghijklmnop");
        assert_eq!(decode(&bytes), Err(ReportError::BadName));
        // An empty name.
        let mut bytes = good;
        bytes[16..].fill(0);
        assert_eq!(decode(&bytes), Err(ReportError::BadName));
        // Garbage after the terminator.
        let mut bytes = good;
        bytes[31] = b'x';
        assert_eq!(decode(&bytes), Err(ReportError::BadName));
        // Bad characters, including a non-UTF-8 byte.
        for b in [b'A', b'_', b'.', b' ', b'/', 0x7F, 0x80, 0xFF] {
            let mut bytes = good;
            bytes[17] = b;
            assert_eq!(decode(&bytes), Err(ReportError::BadName), "{b:#x}");
        }
    }

    #[test]
    fn fields_are_checked_in_wire_order() {
        let mut bytes = encode(&report(Event::Start, "tickd"));
        bytes[0] = 9;
        bytes[1] = 9;
        bytes[4] = 9;
        bytes[16] = b'X';
        assert_eq!(decode(&bytes), Err(ReportError::BadEvent));
        bytes[0] = 1;
        assert_eq!(decode(&bytes), Err(ReportError::BadFlag));
        bytes[1] = 1;
        assert_eq!(decode(&bytes), Err(ReportError::TooManyRestarts));
        bytes[4] = 0;
        assert_eq!(decode(&bytes), Err(ReportError::BadName));
    }

    #[test]
    fn names_are_stable() {
        assert_eq!(ReportError::Truncated.name(), "truncated");
        assert_eq!(ReportError::BadEvent.name(), "bad_event");
        assert_eq!(ReportError::BadFlag.name(), "bad_flag");
        assert_eq!(ReportError::BadName.name(), "bad_name");
        assert_eq!(ReportError::TooManyRestarts.name(), "too_many_restarts");
        let names: Vec<&str> = Event::ALL.iter().map(|e| e.name()).collect();
        assert_eq!(names, ["start", "restart", "done", "failed", "ready"]);
        let codes: Vec<u8> = Event::ALL.iter().map(|e| e.code()).collect();
        assert_eq!(codes, [1, 2, 3, 4, 5]);
    }
}
