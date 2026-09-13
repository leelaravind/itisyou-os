//! The approved system view (V0.11, VIEW11-001, ADR-0024).
//!
//! Everything a program holding `sys_view` may learn about the running system,
//! and nothing more: counts and a few allow-listed rows in one fixed,
//! versioned binary layout. No paths, arguments, file names, memory contents
//! or payloads — the diagnostic agent needs to know THAT a service failed, not
//! what any program is doing. The kernel fills a [`View`] from its own tables
//! and serves [`View::encode`]'s bytes; a program decodes them with
//! [`View::decode`], which refuses anything malformed instead of guessing, and
//! derives the classifier's input with [`features`] — the one function the
//! agent, the tests and the kernel's own recomputation of a proposal's
//! condition all use.
//!
//! ```text
//! offset size field
//!      0    8 magic "ITVIEW01"
//!      8    8 uptime, timer ticks (100 Hz)
//!     16    2 processes (every slot in the process table)
//!     18    2 runnable
//!     20    2 waiting (in wait, sleep, a console read or a window event)
//!     22    2 ended, not yet collected
//!     24    1 flags: bit 0 always-on scheduling, bit 1 paused, bit 2 truncated
//!     25    1 service rows used (at most 8)
//!     26    2 reserved, zero
//!     28    4 longest background gap since boot, ms
//!     32    4 busy-point slices     36 4 idle-point slices     40 4 job slices
//!     44    4 audit records since boot     48 4 audit denials since boot
//!     52    2 records in the audit ring    54 2 denials among them
//!     56    4 frames received   60 4 frames sent   64 4 frames refused
//!     68    4 reserved, zero
//!     72 8×20 service rows: name[16] (NUL-padded, [a-z0-9-]{1,15}), state u8,
//!             reported by init u8 (0/1), restarts u16; unused rows all zero
//! ```
//!
//! All integers little-endian. Counters wider than their field saturate.

/// Size of an encoded view.
pub const LEN: usize = 232;
pub const MAGIC: &[u8; 8] = b"ITVIEW01";
/// Service rows a view carries; more set the `truncated` flag.
pub const MAX_SERVICES: usize = 8;
/// A service name's field (15 bytes and a NUL at least).
pub const NAME_LEN: usize = 16;
/// Features [`features`] derives from a view.
pub const FEATURES: usize = 16;
/// Every feature is scaled into `0..=FEATURE_MAX`.
pub const FEATURE_MAX: i32 = 1000;
/// Timer ticks per second (the PIT tick counter the kernel reports).
pub const TICKS_PER_SEC: u64 = 100;

const ROWS: usize = 72;
const ROW_LEN: usize = 20;
const FLAG_ALWAYS_ON: u8 = 1;
const FLAG_PAUSED: u8 = 2;
const FLAG_TRUNCATED: u8 = 4;

/// A service row's state, as the kernel's service table knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvcState {
    #[default]
    Stopped = 0,
    Running = 1,
    Done = 2,
    Restarting = 3,
    Failed = 4,
}

impl SvcState {
    pub const fn from_u8(v: u8) -> Option<SvcState> {
        match v {
            0 => Some(SvcState::Stopped),
            1 => Some(SvcState::Running),
            2 => Some(SvcState::Done),
            3 => Some(SvcState::Restarting),
            4 => Some(SvcState::Failed),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            SvcState::Stopped => "stopped",
            SvcState::Running => "running",
            SvcState::Done => "done",
            SvcState::Restarting => "restarting",
            SvcState::Failed => "failed",
        }
    }
}

/// One service row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServiceRow {
    pub name: [u8; NAME_LEN],
    pub state: SvcState,
    pub restarts: u16,
    /// Reported by `/sbin/init` (true) or by the kernel's own supervisor.
    pub by_init: bool,
}

impl ServiceRow {
    /// A row for `name`, or `None` if the name is not `[a-z0-9-]{1,15}`.
    pub fn new(name: &str, state: SvcState, restarts: u32, by_init: bool) -> Option<ServiceRow> {
        if !valid_name(name.as_bytes()) {
            return None;
        }
        let mut n = [0u8; NAME_LEN];
        n[..name.len()].copy_from_slice(name.as_bytes());
        Some(ServiceRow {
            name: n,
            state,
            restarts: u16::try_from(restarts).unwrap_or(u16::MAX),
            by_init,
        })
    }

    /// The name, without its NUL padding.
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

fn valid_name(b: &[u8]) -> bool {
    !b.is_empty()
        && b.len() < NAME_LEN
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// The system view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct View {
    pub uptime_ticks: u64,
    pub processes: u16,
    pub runnable: u16,
    pub waiting: u16,
    pub ended: u16,
    pub always_on: bool,
    pub paused: bool,
    pub truncated: bool,
    pub max_gap_ms: u32,
    pub busy_slices: u32,
    pub idle_slices: u32,
    pub job_slices: u32,
    pub audit_total: u32,
    pub audit_denials: u32,
    pub recent_records: u16,
    pub recent_denials: u16,
    pub net_rx: u32,
    pub net_tx: u32,
    pub net_refused: u32,
    pub services: [ServiceRow; MAX_SERVICES],
    pub service_count: u8,
}

/// Why a view was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewError {
    BadLength,
    BadMagic,
    BadFlags,
    BadCount,
    BadReserved,
    BadName,
    BadState,
    /// An unused service row is not all zero.
    StrayRow,
    /// The counts contradict each other (more runnable than processes, more
    /// denials than records, ...).
    Inconsistent,
}

impl ViewError {
    pub const fn name(self) -> &'static str {
        match self {
            ViewError::BadLength => "bad_length",
            ViewError::BadMagic => "bad_magic",
            ViewError::BadFlags => "bad_flags",
            ViewError::BadCount => "bad_count",
            ViewError::BadReserved => "bad_reserved",
            ViewError::BadName => "bad_name",
            ViewError::BadState => "bad_state",
            ViewError::StrayRow => "stray_row",
            ViewError::Inconsistent => "inconsistent",
        }
    }
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn get16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn get32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Saturate a counter into 16 or 32 bits.
pub fn sat16(v: u64) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}
pub fn sat32(v: u64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

impl View {
    /// Add a service row; past [`MAX_SERVICES`] the view is marked
    /// `truncated` instead.
    pub fn push_service(&mut self, row: ServiceRow) {
        let n = usize::from(self.service_count);
        if n < MAX_SERVICES {
            self.services[n] = row;
            self.service_count += 1;
        } else {
            self.truncated = true;
        }
    }

    /// The used service rows.
    pub fn rows(&self) -> &[ServiceRow] {
        &self.services[..usize::from(self.service_count).min(MAX_SERVICES)]
    }

    pub fn encode(&self) -> [u8; LEN] {
        let mut b = [0u8; LEN];
        b[..8].copy_from_slice(MAGIC);
        b[8..16].copy_from_slice(&self.uptime_ticks.to_le_bytes());
        put16(&mut b, 16, self.processes);
        put16(&mut b, 18, self.runnable);
        put16(&mut b, 20, self.waiting);
        put16(&mut b, 22, self.ended);
        let flag = |on: bool, bit: u8| if on { bit } else { 0 };
        b[24] = flag(self.always_on, FLAG_ALWAYS_ON)
            | flag(self.paused, FLAG_PAUSED)
            | flag(self.truncated, FLAG_TRUNCATED);
        b[25] = self.service_count.min(MAX_SERVICES as u8);
        put32(&mut b, 28, self.max_gap_ms);
        put32(&mut b, 32, self.busy_slices);
        put32(&mut b, 36, self.idle_slices);
        put32(&mut b, 40, self.job_slices);
        put32(&mut b, 44, self.audit_total);
        put32(&mut b, 48, self.audit_denials);
        put16(&mut b, 52, self.recent_records);
        put16(&mut b, 54, self.recent_denials);
        put32(&mut b, 56, self.net_rx);
        put32(&mut b, 60, self.net_tx);
        put32(&mut b, 64, self.net_refused);
        for (i, row) in self.rows().iter().enumerate() {
            let o = ROWS + i * ROW_LEN;
            b[o..o + NAME_LEN].copy_from_slice(&row.name);
            b[o + 16] = row.state as u8;
            b[o + 17] = u8::from(row.by_init);
            put16(&mut b, o + 18, row.restarts);
        }
        b
    }

    /// Decode and validate a view. Never panics, whatever the bytes.
    pub fn decode(b: &[u8]) -> Result<View, ViewError> {
        if b.len() != LEN {
            return Err(ViewError::BadLength);
        }
        if &b[..8] != MAGIC {
            return Err(ViewError::BadMagic);
        }
        let flags = b[24];
        if flags & !(FLAG_ALWAYS_ON | FLAG_PAUSED | FLAG_TRUNCATED) != 0 {
            return Err(ViewError::BadFlags);
        }
        let count = usize::from(b[25]);
        if count > MAX_SERVICES {
            return Err(ViewError::BadCount);
        }
        if get16(b, 26) != 0 || get32(b, 68) != 0 {
            return Err(ViewError::BadReserved);
        }
        let mut v = View {
            uptime_ticks: u64::from_le_bytes([
                b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
            ]),
            processes: get16(b, 16),
            runnable: get16(b, 18),
            waiting: get16(b, 20),
            ended: get16(b, 22),
            always_on: flags & FLAG_ALWAYS_ON != 0,
            paused: flags & FLAG_PAUSED != 0,
            truncated: flags & FLAG_TRUNCATED != 0,
            max_gap_ms: get32(b, 28),
            busy_slices: get32(b, 32),
            idle_slices: get32(b, 36),
            job_slices: get32(b, 40),
            audit_total: get32(b, 44),
            audit_denials: get32(b, 48),
            recent_records: get16(b, 52),
            recent_denials: get16(b, 54),
            net_rx: get32(b, 56),
            net_tx: get32(b, 60),
            net_refused: get32(b, 64),
            services: [ServiceRow::default(); MAX_SERVICES],
            service_count: count as u8,
        };
        for i in 0..MAX_SERVICES {
            let o = ROWS + i * ROW_LEN;
            let raw = &b[o..o + ROW_LEN];
            if i >= count {
                if raw.iter().any(|&x| x != 0) {
                    return Err(ViewError::StrayRow);
                }
                continue;
            }
            let name = &raw[..NAME_LEN];
            let end = name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
            if end == NAME_LEN || !valid_name(&name[..end]) || name[end..].iter().any(|&c| c != 0) {
                return Err(ViewError::BadName);
            }
            let state = SvcState::from_u8(raw[16]).ok_or(ViewError::BadState)?;
            if raw[17] > 1 {
                return Err(ViewError::BadState);
            }
            let mut n = [0u8; NAME_LEN];
            n.copy_from_slice(name);
            v.services[i] = ServiceRow {
                name: n,
                state,
                by_init: raw[17] == 1,
                restarts: get16(raw, 18),
            };
        }
        let procs = u32::from(v.processes);
        if u32::from(v.runnable) + u32::from(v.waiting) + u32::from(v.ended) > procs
            || v.recent_denials > v.recent_records
            || v.audit_denials > v.audit_total
            || (v.paused && !v.always_on)
        {
            return Err(ViewError::Inconsistent);
        }
        Ok(v)
    }
}

/// What each feature means, in [`features`]' order (for documentation and
/// the scenario file's header).
pub const FEATURE_NAMES: [&str; FEATURES] = [
    "processes x50",
    "runnable x100",
    "waiting x100",
    "ended x300",
    "services x200",
    "services running x200",
    "services failed x500",
    "most restarts x300",
    "scheduler paused (0/1000)",
    "always-on scheduling (0/1000)",
    "longest background gap, ms",
    "denials in the audit ring x15",
    "records in the audit ring x15",
    "denial share of the ring, per mille",
    "frames refused x10",
    "uptime, s",
];

fn scaled(v: u64, by: u64) -> i32 {
    v.saturating_mul(by).min(FEATURE_MAX as u64) as i32
}

/// The classifier's input: 16 features, each scaled into `0..=1000`.
pub fn features(v: &View) -> [i32; FEATURES] {
    let rows = v.rows();
    let running = rows.iter().filter(|r| r.state == SvcState::Running).count() as u64;
    let failed = rows.iter().filter(|r| r.state == SvcState::Failed).count() as u64;
    let most_restarts = rows
        .iter()
        .map(|r| u64::from(r.restarts))
        .max()
        .unwrap_or(0);
    let records = u64::from(v.recent_records);
    let denials = u64::from(v.recent_denials);
    // An empty ring has no share (checked_div: 0 records -> None -> 0).
    let share = (denials * 1000)
        .checked_div(records)
        .map_or(0, |s| s.min(1000) as i32);
    [
        scaled(u64::from(v.processes), 50),
        scaled(u64::from(v.runnable), 100),
        scaled(u64::from(v.waiting), 100),
        scaled(u64::from(v.ended), 300),
        scaled(rows.len() as u64, 200),
        scaled(running, 200),
        scaled(failed, 500),
        scaled(most_restarts, 300),
        if v.paused { FEATURE_MAX } else { 0 },
        if v.always_on { FEATURE_MAX } else { 0 },
        scaled(u64::from(v.max_gap_ms), 1),
        scaled(denials, 15),
        scaled(records, 15),
        share,
        scaled(u64::from(v.net_refused), 10),
        scaled(v.uptime_ticks / TICKS_PER_SEC, 1),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A view like the one the V0.10 image shows after boot: init, tickd,
    /// flapd failed, the console running one program.
    fn booted() -> View {
        let mut v = View {
            uptime_ticks: 1234,
            processes: 4,
            runnable: 2,
            waiting: 1,
            ended: 0,
            always_on: true,
            paused: false,
            truncated: false,
            max_gap_ms: 61,
            busy_slices: 10,
            idle_slices: 567,
            job_slices: 8,
            audit_total: 9,
            audit_denials: 0,
            recent_records: 9,
            recent_denials: 0,
            net_rx: 3,
            net_tx: 2,
            net_refused: 0,
            ..View::default()
        };
        v.push_service(ServiceRow::new("tickd", SvcState::Running, 0, true).unwrap());
        v.push_service(ServiceRow::new("flapd", SvcState::Failed, 3, true).unwrap());
        v
    }

    #[test]
    fn a_view_round_trips_byte_for_byte() {
        let v = booted();
        let b = v.encode();
        assert_eq!(b.len(), LEN);
        assert_eq!(View::decode(&b), Ok(v));
        assert_eq!(View::decode(&b).unwrap().encode(), b);
        // The layout is pinned: a field moved is a new version, not a quiet
        // change (the agent and the kernel must agree byte for byte).
        assert_eq!(&b[..8], b"ITVIEW01");
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 4);
        assert_eq!(b[24], FLAG_ALWAYS_ON);
        assert_eq!(b[25], 2);
        assert_eq!(&b[ROWS + ROW_LEN..ROWS + ROW_LEN + 5], b"flapd");
        assert_eq!(b[ROWS + ROW_LEN + 16], SvcState::Failed as u8);
        assert_eq!(
            u16::from_le_bytes([b[ROWS + ROW_LEN + 18], b[ROWS + ROW_LEN + 19]]),
            3
        );
    }

    #[test]
    fn hostile_views_are_refused_for_their_own_reason() {
        let good = booted().encode();
        let bad = |f: &dyn Fn(&mut [u8; LEN])| {
            let mut b = good;
            f(&mut b);
            View::decode(&b)
        };
        assert_eq!(View::decode(&good[..LEN - 1]), Err(ViewError::BadLength));
        assert_eq!(bad(&|b| b[0] = b'X'), Err(ViewError::BadMagic));
        assert_eq!(bad(&|b| b[24] |= 8), Err(ViewError::BadFlags));
        assert_eq!(bad(&|b| b[25] = 9), Err(ViewError::BadCount));
        assert_eq!(bad(&|b| b[26] = 1), Err(ViewError::BadReserved));
        assert_eq!(bad(&|b| b[70] = 1), Err(ViewError::BadReserved));
        assert_eq!(bad(&|b| b[ROWS] = b'T'), Err(ViewError::BadName));
        assert_eq!(bad(&|b| b[ROWS] = b'\n'), Err(ViewError::BadName));
        assert_eq!(bad(&|b| b[ROWS] = 0), Err(ViewError::BadName));
        // A name that fills all 16 bytes has no terminator.
        assert_eq!(
            bad(&|b| b[ROWS..ROWS + NAME_LEN].copy_from_slice(b"aaaaaaaaaaaaaaaa")),
            Err(ViewError::BadName)
        );
        // Bytes after the terminator.
        assert_eq!(bad(&|b| b[ROWS + 10] = b'x'), Err(ViewError::BadName));
        assert_eq!(bad(&|b| b[ROWS + 16] = 5), Err(ViewError::BadState));
        assert_eq!(bad(&|b| b[ROWS + 17] = 2), Err(ViewError::BadState));
        assert_eq!(
            bad(&|b| b[ROWS + 2 * ROW_LEN] = b'a'),
            Err(ViewError::StrayRow)
        );
        assert_eq!(bad(&|b| b[18] = 9), Err(ViewError::Inconsistent));
        assert_eq!(bad(&|b| b[54] = 10), Err(ViewError::Inconsistent));
        assert_eq!(bad(&|b| b[24] = FLAG_PAUSED), Err(ViewError::Inconsistent));
        // Every single-byte change either decodes to a different view or is
        // refused - and nothing panics.
        for i in 0..LEN {
            for x in [0u8, 1, 0x7f, 0xff] {
                let mut b = good;
                if b[i] == x {
                    continue;
                }
                b[i] = x;
                if let Ok(v) = View::decode(&b) {
                    assert_ne!(v, booted(), "byte {i} = {x:#x} decoded to the same view");
                }
            }
        }
    }

    #[test]
    fn service_rows_are_validated_and_truncation_is_flagged() {
        assert!(ServiceRow::new("", SvcState::Running, 0, true).is_none());
        assert!(ServiceRow::new("Tickd", SvcState::Running, 0, true).is_none());
        assert!(ServiceRow::new("a\n", SvcState::Running, 0, true).is_none());
        assert!(ServiceRow::new(&"a".repeat(15), SvcState::Running, 0, true).is_some());
        assert!(ServiceRow::new(&"a".repeat(16), SvcState::Running, 0, true).is_none());
        let r = ServiceRow::new("flapd", SvcState::Failed, 70_000, true).unwrap();
        assert_eq!((r.name_str(), r.restarts), ("flapd", u16::MAX));
        let mut v = View::default();
        for i in 0..MAX_SERVICES + 1 {
            let name = [b'a' + i as u8];
            v.push_service(
                ServiceRow::new(
                    core::str::from_utf8(&name).unwrap(),
                    SvcState::Running,
                    0,
                    false,
                )
                .unwrap(),
            );
        }
        assert_eq!(
            (usize::from(v.service_count), v.truncated),
            (MAX_SERVICES, true)
        );
        assert_eq!(View::decode(&v.encode()), Ok(v));
    }

    #[test]
    fn features_are_scaled_clamped_and_pinned() {
        let f = features(&booted());
        assert_eq!(
            f,
            [200, 200, 100, 0, 400, 200, 500, 900, 0, 1000, 61, 0, 135, 0, 0, 12]
        );
        // Saturation: every feature stays within 0..=1000 whatever the counts.
        let mut v = booted();
        v.processes = u16::MAX;
        v.runnable = 1000;
        v.waiting = 1000;
        v.ended = 1000;
        v.max_gap_ms = u32::MAX;
        v.recent_records = u16::MAX;
        v.recent_denials = u16::MAX;
        v.net_refused = u32::MAX;
        v.uptime_ticks = u64::MAX;
        v.paused = true;
        v.services[1].restarts = u16::MAX;
        for x in features(&v) {
            assert!((0..=FEATURE_MAX).contains(&x), "{x}");
        }
        assert_eq!(features(&v)[13], 1000);
        assert_eq!(features(&View::default()), [0; FEATURES]);
        assert_eq!(FEATURE_NAMES.len(), FEATURES);
    }
}
