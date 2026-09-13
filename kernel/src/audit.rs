//! Audit / provenance trail (V0.7).
//!
//! Every privileged platform action — and every DENIED capability check —
//! is recorded: sequence number, timer tick, acting pid, action name, the
//! capability involved, and the result. Records land in a bounded in-memory
//! ring (queryable via the `audit` shell command) and are mirrored as
//! machine-readable `[ITISYOU:AUDIT]` serial markers so automated tests can
//! assert on them. Contents are metadata only — no user data/payloads are
//! logged. Future AI-agent actions must flow through the same syscall →
//! capability check → service path, so they inherit this provenance trail
//! automatically (intelligence ≠ authority; see ADR-0012).

use alloc::collections::VecDeque;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// One audit record.
#[derive(Debug, Clone)]
pub struct Record {
    pub seq: u64,
    pub tick: u64,
    pub pid: u64,
    pub action: &'static str,
    /// Capability bit involved (0 = not capability-related).
    pub cap: u64,
    pub ok: bool,
    /// Short free-form detail (a path/app name), never payload contents.
    pub detail: Option<String>,
}

const RING_MAX: usize = 64;

static RING: Mutex<VecDeque<Record>> = Mutex::new(VecDeque::new());
static SEQ: AtomicU64 = AtomicU64::new(0);
static DENIALS: AtomicU64 = AtomicU64::new(0);

/// File in the persistent store holding the durable trail.
pub const TRAIL_PATH: &str = "/data/audit.log";
/// Format marker, so a future change of layout is a recognised version rather
/// than a parse failure that looks like tampering.
const TRAIL_MAGIC: &str = "itisyou-audit v1";

/// Live hash-chain state.
///
/// `head` covers every record ever *persisted*, across boots: a boot that
/// recovers a trail continues from the head it found, so the chain is one
/// continuous history rather than a fresh log per restart.
struct Chain {
    head: kernel_core::audit_chain::Head,
    /// Head as of the last successful save — the value a verifier compares
    /// against, kept separate so an unsaved record cannot make the stored
    /// trail look wrong.
    saved_head: kernel_core::audit_chain::Head,
    /// Records covered by `saved_head`.
    saved_count: usize,
    /// Boot number, incremented each time a trail is recovered.
    boot: u64,
}

static CHAIN: Mutex<Chain> = Mutex::new(Chain {
    head: kernel_core::audit_chain::GENESIS,
    saved_head: kernel_core::audit_chain::GENESIS,
    saved_count: 0,
    boot: 0,
});

/// The canonical one-line form of a record. This exact byte sequence is what
/// the hash chain covers and what is written to the trail, so a record's hash
/// cannot depend on how some caller chose to format it.
fn encode(record: &Record) -> String {
    alloc::format!(
        "{} {} {} {} {:#x} {} {}",
        record.seq,
        record.tick,
        record.pid,
        record.action,
        record.cap,
        if record.ok { "ok" } else { "denied" },
        record.detail.as_deref().unwrap_or("-"),
    )
}

fn push(action: &'static str, cap: u64, ok: bool, detail: Option<String>) {
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let pid = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
    let tick = crate::interrupts::ticks();
    let result = if ok { "ok" } else { "denied" };
    match &detail {
        Some(d) => crate::serial_println!(
            "[ITISYOU:AUDIT] seq={seq} tick={tick} pid={pid} action={action} cap={cap:#x} result={result} detail=\"{d}\""
        ),
        None => crate::serial_println!(
            "[ITISYOU:AUDIT] seq={seq} tick={tick} pid={pid} action={action} cap={cap:#x} result={result}"
        ),
    }
    if !ok {
        DENIALS.fetch_add(1, Ordering::SeqCst);
    }
    let record = Record {
        seq,
        tick,
        pid,
        action,
        cap,
        ok,
        detail,
    };
    // Extend the chain BEFORE the ring drops anything: the in-memory ring is
    // bounded, but the chain must cover every record that ever existed, or a
    // system under load could lose evidence simply by being busy.
    {
        let mut chain = CHAIN.lock();
        chain.head = kernel_core::audit_chain::extend(&chain.head, encode(&record).as_bytes());
    }
    let mut ring = RING.lock();
    if ring.len() >= RING_MAX {
        ring.pop_front();
    }
    ring.push_back(record);
}

/// Outcome of reading the persisted trail at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverStatus {
    /// Nothing to recover: no storage attached, no filesystem on it, or no
    /// trail file yet. All three are a first boot rather than a fault, and
    /// splitting them would imply a distinction the recovery path cannot make
    /// without mounting — which it must not do speculatively.
    Absent,
    /// The trail parsed and its chain verified.
    Verified,
    /// The trail parsed but its chain does not match — records were altered,
    /// removed, reordered or inserted since it was written.
    Tampered,
    /// The file could not be read as a trail at all.
    Unreadable,
}

impl RecoverStatus {
    pub const fn name(self) -> &'static str {
        match self {
            RecoverStatus::Absent => "absent",
            RecoverStatus::Verified => "verified",
            RecoverStatus::Tampered => "TAMPERED",
            RecoverStatus::Unreadable => "unreadable",
        }
    }
}

/// Write the trail to the persistent store.
///
/// One atomic filesystem write (see `fs_disk::FileSystem::write`), so an
/// interrupted save leaves the previous trail intact rather than a truncated
/// one that would be indistinguishable from tampering.
pub fn save() -> bool {
    let (boot, head) = {
        let chain = CHAIN.lock();
        (chain.boot, chain.head)
    };
    let mut out = String::new();
    let records = with_records(|rs| {
        let mut v = alloc::vec::Vec::new();
        for r in rs {
            v.push(encode(r));
        }
        v
    });
    let mut head_hex = [0u8; 64];
    let head_text = kernel_core::audit_chain::format_head(&head, &mut head_hex);
    out.push_str(&alloc::format!(
        "{TRAIL_MAGIC} boot={boot} count={} head={head_text}\n",
        records.len()
    ));
    for r in &records {
        out.push_str(r);
        out.push('\n');
    }
    let written = crate::with_persistent_store(|fs| {
        fs.write(
            crate::store_name(TRAIL_PATH).unwrap_or("audit.log"),
            out.as_bytes(),
        )
    });
    match written {
        Some(Ok(())) => {
            let mut chain = CHAIN.lock();
            chain.saved_head = head;
            chain.saved_count = records.len();
            crate::serial_println!(
                "[ITISYOU:AUDIT] trail_saved records={} head={head_text} boot={boot}",
                records.len()
            );
            true
        }
        _ => {
            crate::serial_println!("[ITISYOU:AUDIT] trail_save_failed");
            false
        }
    }
}

/// Read and verify the persisted trail, and continue its chain.
///
/// The recovered head becomes this boot's starting head whether or not the
/// chain verified: refusing to continue from a tampered trail would only hide
/// the tampering from every later verification.
pub fn recover() -> RecoverStatus {
    // Read-only mount: recovery must never be the reason a disk gets
    // formatted (see `with_mounted_store`).
    let Some(data) = crate::with_mounted_store(|fs| {
        fs.read(crate::store_name(TRAIL_PATH).unwrap_or("audit.log"))
    }) else {
        return report(RecoverStatus::Absent, 0, None);
    };
    let Ok(bytes) = data else {
        return report(RecoverStatus::Absent, 0, None);
    };
    let Ok(text) = core::str::from_utf8(&bytes) else {
        return report(RecoverStatus::Unreadable, 0, None);
    };
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return report(RecoverStatus::Unreadable, 0, None);
    };
    if !header.starts_with(TRAIL_MAGIC) {
        return report(RecoverStatus::Unreadable, 0, None);
    }
    let mut boot = 0u64;
    let mut count = None;
    let mut head = None;
    for field in header.split_whitespace() {
        if let Some(v) = field.strip_prefix("boot=") {
            boot = v.parse().unwrap_or(0);
        } else if let Some(v) = field.strip_prefix("count=") {
            count = v.parse::<usize>().ok();
        } else if let Some(v) = field.strip_prefix("head=") {
            head = kernel_core::audit_chain::parse_head(v);
        }
    }
    let (Some(count), Some(head)) = (count, head) else {
        return report(RecoverStatus::Unreadable, 0, None);
    };
    let records: alloc::vec::Vec<&[u8]> = lines.map(|l| l.as_bytes()).collect();
    let status = match kernel_core::audit_chain::verify(
        &kernel_core::audit_chain::GENESIS,
        &records,
        count,
        &head,
    ) {
        Ok(()) => RecoverStatus::Verified,
        Err(_) => RecoverStatus::Tampered,
    };
    {
        let mut chain = CHAIN.lock();
        chain.head = head;
        chain.saved_head = head;
        chain.saved_count = records.len();
        chain.boot = boot + 1;
    }
    report(status, records.len(), Some(head))
}

fn report(
    status: RecoverStatus,
    records: usize,
    head: Option<kernel_core::audit_chain::Head>,
) -> RecoverStatus {
    let mut buf = [0u8; 64];
    let head_text = match &head {
        Some(h) => kernel_core::audit_chain::format_head(h, &mut buf),
        None => "-",
    };
    crate::serial_println!(
        "[ITISYOU:AUDIT] trail_recovered status={} records={records} head={head_text}",
        status.name()
    );
    status
}

/// The live chain head and the boot counter, for the `audit` command.
pub fn chain_state() -> (kernel_core::audit_chain::Head, u64, usize) {
    let chain = CHAIN.lock();
    (chain.head, chain.boot, chain.saved_count)
}

/// Record a denied capability check.
pub fn denied(action: &'static str, cap: u64) {
    push(action, cap, false, None);
}

/// Record a denied V0.8 handle check with the resource class and the exact
/// reason (`revoked`, `expired`, `scope_denied`, `not_owner`, …). A bare
/// "permission denied" is not diagnosable; the reason distinguishes an
/// authority that was never held from one that was withdrawn or timed out.
pub fn denied_capability(
    action: &'static str,
    kind: kernel_core::capability::CapabilityKind,
    reason: &'static str,
) {
    push(
        action,
        0,
        false,
        Some(alloc::format!("kind={} reason={reason}", kind.name())),
    );
}

/// Record a permitted privileged action (with optional short detail).
pub fn allowed(action: &'static str, cap: u64, detail: Option<String>) {
    push(action, cap, true, detail);
}

/// Total records ever written / total denials (diagnostic evidence).
pub fn counts() -> (u64, u64) {
    (SEQ.load(Ordering::SeqCst), DENIALS.load(Ordering::SeqCst))
}

/// Run `f` over a snapshot of the retained ring (newest last).
pub fn with_records<R>(f: impl FnOnce(&[Record]) -> R) -> R {
    let ring = RING.lock();
    let snapshot: alloc::vec::Vec<Record> = ring.iter().cloned().collect();
    drop(ring);
    f(&snapshot)
}

// --- Anchoring (V0.9) --------------------------------------------------------
//
// The chain detects a record being altered, removed, reordered or inserted —
// but it is unkeyed, so an attacker who can rewrite the whole trail can write
// one that verifies: in the limit, an EMPTY trail (`count=0`, genesis head),
// which erases the entire history without leaving a mark. A key stored beside
// the trail would not help (the same attacker reads it), and there is no
// hardware root of trust to seal one. What does help is a copy of the head
// the attacker cannot reach: a witness on another machine. `anchor` sends the
// SAVED trail's head there; `check_anchor` asks for it back and compares it
// with the head this boot recovered from disk.

/// What the witness said about the trail recovered at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorVerdict {
    /// The witness's last anchor is exactly the trail on disk.
    Match,
    /// The trail on disk is not the one last anchored: rewritten, replaced or
    /// rolled back.
    Mismatch,
    /// The witness holds no anchor yet.
    Unanchored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorError {
    /// Nothing has been saved in this boot or recovered from disk.
    NothingSaved,
    NoReply,
    BadReply,
}

const ANCHOR_TIMEOUT_MS: u64 = 2000;

fn parse_witness(reply: &str) -> Option<(usize, kernel_core::audit_chain::Head)> {
    let mut count = None;
    let mut head = None;
    for field in reply.split_whitespace() {
        if let Some(v) = field.strip_prefix("count=") {
            count = v.parse().ok();
        } else if let Some(v) = field.strip_prefix("head=") {
            head = kernel_core::audit_chain::parse_head(v);
        }
    }
    Some((count?, head?))
}

/// Send the saved trail's head to the witness at `ip:port` and wait for its
/// acknowledgement. Returns what was anchored.
pub fn anchor(
    ip: kernel_core::net::ipv4::Ipv4Addr,
    port: u16,
) -> Result<(usize, kernel_core::audit_chain::Head), AnchorError> {
    let (count, head, saved) = {
        let chain = CHAIN.lock();
        (
            chain.saved_count,
            chain.saved_head,
            chain.boot > 0 || chain.saved_count > 0,
        )
    };
    if !saved {
        return Err(AnchorError::NothingSaved);
    }
    let mut hex = [0u8; 64];
    let head_text = kernel_core::audit_chain::format_head(&head, &mut hex);
    let msg = alloc::format!("ITISYOU-ANCHOR v1 count={count} head={head_text}");
    let mut reply = [0u8; 256];
    let n = crate::net::udp_request(ip, port, msg.as_bytes(), &mut reply, ANCHOR_TIMEOUT_MS)
        .ok_or(AnchorError::NoReply)?;
    let text = core::str::from_utf8(&reply[..n]).map_err(|_| AnchorError::BadReply)?;
    // The witness must echo exactly what it stored; anything else means the
    // anchor cannot be relied on.
    match text.strip_prefix("ANCHORED").and_then(parse_witness) {
        Some((c, h)) if c == count && h == head => {
            crate::serial_println!(
                "[ITISYOU:AUDIT] anchored count={count} head={head_text} witness_ack=ok"
            );
            allowed("audit_anchor", 0, None);
            Ok((count, head))
        }
        _ => Err(AnchorError::BadReply),
    }
}

/// Ask the witness for the last anchored head and compare it with the trail
/// recovered from disk at boot (or saved since).
pub fn check_anchor(
    ip: kernel_core::net::ipv4::Ipv4Addr,
    port: u16,
) -> Result<AnchorVerdict, AnchorError> {
    let (count, head) = {
        let chain = CHAIN.lock();
        (chain.saved_count, chain.saved_head)
    };
    let mut reply = [0u8; 256];
    let n = crate::net::udp_request(
        ip,
        port,
        b"ITISYOU-ANCHOR-QUERY v1",
        &mut reply,
        ANCHOR_TIMEOUT_MS,
    )
    .ok_or(AnchorError::NoReply)?;
    let text = core::str::from_utf8(&reply[..n]).map_err(|_| AnchorError::BadReply)?;
    let mut trail_hex = [0u8; 64];
    let trail_text = kernel_core::audit_chain::format_head(&head, &mut trail_hex);
    let verdict = if text.starts_with("NONE") {
        AnchorVerdict::Unanchored
    } else {
        let (wc, wh) = text
            .strip_prefix("LAST")
            .and_then(parse_witness)
            .ok_or(AnchorError::BadReply)?;
        let mut w_hex = [0u8; 64];
        let w_text = kernel_core::audit_chain::format_head(&wh, &mut w_hex);
        crate::serial_println!(
            "[ITISYOU:AUDIT] anchor_witness count={wc} head={w_text} trail_count={count} trail_head={trail_text}"
        );
        if wc == count && wh == head {
            AnchorVerdict::Match
        } else {
            AnchorVerdict::Mismatch
        }
    };
    let result = match verdict {
        AnchorVerdict::Match => "MATCH",
        AnchorVerdict::Mismatch => "MISMATCH",
        AnchorVerdict::Unanchored => "UNANCHORED",
    };
    crate::serial_println!("[ITISYOU:AUDIT] anchor_check result={result}");
    if verdict == AnchorVerdict::Mismatch {
        denied("audit_anchor_mismatch", 0);
    }
    Ok(verdict)
}
