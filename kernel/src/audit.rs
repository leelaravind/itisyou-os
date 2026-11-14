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

use crate::sync::Mutex;
use alloc::collections::VecDeque;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

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

/// The durable trail's name in the persistent store (`/data/audit.log`). It
/// is kernel-owned: the `fs_*` syscalls refuse it (SEC11-001).
const TRAIL_NAME: &str = kernel_core::update::AUDIT_TRAIL;

/// Most records a stored trail holds (V0.11). The trail is the newest part of
/// the continuous history; its header's `base` stands for everything before.
const TRAIL_MAX: usize = 128;

/// Live hash-chain state.
///
/// `head` covers every record ever made, across boots: a boot that recovers
/// a trail continues from the head it found, so the chain is one continuous
/// history rather than a fresh log per restart. `window` is the part of that
/// history the next save stores, and `base` the head its first record
/// extends — kept together under one lock, so a save can never pair a head
/// with records it does not cover (AUDIT11-001).
struct Chain {
    head: kernel_core::audit_chain::Head,
    /// The head the oldest record in `window` extends.
    base: kernel_core::audit_chain::Head,
    /// The newest records in their canonical encoding, oldest first. Unlike
    /// the display ring, it carries the records recovered from disk too.
    window: VecDeque<String>,
    /// Head as of the last successful save — the value a verifier compares
    /// against, kept separate so an unsaved record cannot make the stored
    /// trail look wrong.
    saved_head: kernel_core::audit_chain::Head,
    /// Records covered by `saved_head`.
    saved_count: usize,
    /// SHA-256 of the exact trail bytes this boot last wrote or recovered:
    /// what `audit verify` compares the file against, so a trail replaced
    /// underneath a running system is caught even when its chain is valid.
    saved_digest: Option<[u8; 32]>,
    /// Boot number, incremented each time a trail is recovered.
    boot: u64,
}

static CHAIN: Mutex<Chain> = Mutex::new(Chain {
    head: kernel_core::audit_chain::GENESIS,
    base: kernel_core::audit_chain::GENESIS,
    window: VecDeque::new(),
    saved_head: kernel_core::audit_chain::GENESIS,
    saved_count: 0,
    saved_digest: None,
    boot: 0,
});

impl Chain {
    /// Append one encoded record, dropping the oldest past [`TRAIL_MAX`] and
    /// moving `base` over each one dropped.
    fn append(&mut self, line: String) {
        self.head = kernel_core::audit_chain::extend(&self.head, line.as_bytes());
        self.window.push_back(line);
        while self.window.len() > TRAIL_MAX {
            if let Some(old) = self.window.pop_front() {
                self.base = kernel_core::audit_chain::extend(&self.base, old.as_bytes());
            }
        }
    }
}

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
    // A detail can carry text a program chose (a file name): escaped here,
    // once, so the serial line, the ring and the stored trail all hold the
    // same single line, with no kernel marker in it (AUDIT11-002).
    let detail = detail.map(|d| crate::untrusted(&d));
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
    CHAIN.lock().append(encode(&record));
    let mut ring = RING.lock();
    if ring.len() >= RING_MAX {
        ring.pop_front();
    }
    ring.push_back(record);
}

/// Heap bytes the audit ring and the stored-trail window hold (V1.0 leak
/// accounting): both are bounded, but a record spells out numbers that grow
/// with the clock, so their footprint creeps without any leak.
pub fn heap_bytes() -> usize {
    // What the kernel heap (`linked_list_allocator`) charges for an
    // allocation of `n` bytes: at least 16, rounded up to 8; nothing for 0.
    let charged = |n: usize| {
        if n == 0 {
            0
        } else {
            n.max(16).next_multiple_of(8)
        }
    };
    let ring = RING.lock();
    let ring_bytes = charged(ring.capacity() * core::mem::size_of::<Record>())
        + ring
            .iter()
            .map(|r| r.detail.as_ref().map_or(0, |d| charged(d.capacity())))
            .sum::<usize>();
    drop(ring);
    let chain = CHAIN.lock();
    ring_bytes
        + charged(chain.window.capacity() * core::mem::size_of::<String>())
        + chain
            .window
            .iter()
            .map(|s| charged(s.capacity()))
            .sum::<usize>()
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
/// one that would be indistinguishable from tampering. The header, the
/// records and the head are taken under one lock, so the file is exactly the
/// window `head` covers — never a head over records the file does not hold.
pub fn save() -> bool {
    let (header, lines) = {
        let chain = CHAIN.lock();
        let header = kernel_core::audit_chain::TrailHeader {
            boot: chain.boot,
            count: chain.window.len(),
            base: chain.base,
            head: chain.head,
        };
        let lines: alloc::vec::Vec<String> = chain.window.iter().cloned().collect();
        (header, lines)
    };
    let mut out = String::new();
    let _ = kernel_core::audit_chain::write_header(&header, &mut out);
    out.push('\n');
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    let written = crate::with_persistent_store(|fs| fs.write(TRAIL_NAME, out.as_bytes()));
    let mut head_hex = [0u8; 64];
    let head_text = kernel_core::audit_chain::format_head(&header.head, &mut head_hex);
    match written {
        Some(Ok(())) => {
            {
                let mut chain = CHAIN.lock();
                chain.saved_head = header.head;
                chain.saved_count = header.count;
                chain.saved_digest = Some(kernel_core::sha256::digest(out.as_bytes()));
            }
            let mut base_hex = [0u8; 64];
            crate::serial_println!(
                "[ITISYOU:AUDIT] trail_saved records={} head={head_text} boot={} base={}",
                header.count,
                header.boot,
                kernel_core::audit_chain::format_head(&header.base, &mut base_hex)
            );
            true
        }
        _ => {
            crate::serial_println!("[ITISYOU:AUDIT] trail_save_failed");
            false
        }
    }
}

/// A stored trail, parsed: its header and its record lines.
struct StoredTrail<'a> {
    header: kernel_core::audit_chain::TrailHeader,
    records: alloc::vec::Vec<&'a [u8]>,
}

impl StoredTrail<'_> {
    fn verifies(&self) -> bool {
        self.header.verify(&self.records).is_ok()
    }
}

/// Longest stored trail the kernel reads: far above any genuine one (at most
/// [`TRAIL_MAX`] records of a few hundred bytes), far below the heap. The
/// file is on disk, so its size is the disk's claim: a larger one is not a
/// trail and is reported unreadable, never allocated (V0.11 review - a
/// crafted multi-megabyte trail panicked recovery on every boot).
const TRAIL_BYTES_MAX: usize = 256 * 1024;

/// Parse a stored trail's bytes; `None` when they are not a trail at all -
/// including one with more record lines than any kernel ever stored.
fn parse_trail(bytes: &[u8]) -> Option<StoredTrail<'_>> {
    let text = core::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    let header = kernel_core::audit_chain::parse_header(lines.next()?)?;
    let records: alloc::vec::Vec<&[u8]> = lines.take(TRAIL_MAX + 1).map(str::as_bytes).collect();
    if records.len() > TRAIL_MAX {
        return None;
    }
    Some(StoredTrail { header, records })
}

/// Selftest: does [`parse_trail`] take a trail of `n` record lines? (It must
/// for every n up to [`TRAIL_MAX`] and refuse every larger one, whatever the
/// header claims, without collecting more than `TRAIL_MAX + 1` of them.)
pub fn parse_trail_accepts(n: usize) -> bool {
    let zeros = "0".repeat(64);
    let mut text = alloc::format!("itisyou-audit v2 boot=0 count={n} base={zeros} head={zeros}");
    for _ in 0..n {
        text.push_str("\nx");
    }
    parse_trail(text.as_bytes()).is_some()
}

/// Read the stored trail, without ever formatting the disk: `None` when
/// there is no store or no trail file; an empty (unreadable) trail when the
/// file is larger than [`TRAIL_BYTES_MAX`].
fn read_trail() -> Option<alloc::vec::Vec<u8>> {
    // Read-only mount: reading must never be the reason a disk gets
    // formatted (see `with_mounted_store`).
    crate::with_mounted_store(|fs| match fs.size_of(TRAIL_NAME) {
        Some(n) if n > TRAIL_BYTES_MAX => Ok(alloc::vec::Vec::new()),
        _ => fs.read(TRAIL_NAME),
    })?
    .ok()
}

/// Read and verify the persisted trail at boot, and continue its chain.
///
/// Boot only: this ADOPTS the stored trail as the start of this boot's
/// history (`audit verify` is the read-only [`check`]). The recovered head
/// becomes this boot's starting head whether or not the chain verified:
/// refusing to continue from a tampered trail would only hide the tampering
/// from every later verification. The recovered records stay in the window,
/// so the next save stores them again rather than only this boot's.
pub fn recover() -> RecoverStatus {
    let Some(bytes) = read_trail() else {
        return report(RecoverStatus::Absent, 0, None);
    };
    let Some(trail) = parse_trail(&bytes) else {
        return report(RecoverStatus::Unreadable, 0, None);
    };
    let status = if trail.verifies() {
        RecoverStatus::Verified
    } else {
        RecoverStatus::Tampered
    };
    let header = trail.header;
    {
        let mut chain = CHAIN.lock();
        // Records this boot made before the trail was read continue from the
        // recovered head, after the recovered records.
        let early: alloc::vec::Vec<String> = chain.window.drain(..).collect();
        chain.head = header.head;
        chain.base = header.base;
        for record in &trail.records {
            chain
                .window
                .push_back(String::from_utf8_lossy(record).into_owned());
        }
        while chain.window.len() > TRAIL_MAX {
            if let Some(old) = chain.window.pop_front() {
                chain.base = kernel_core::audit_chain::extend(&chain.base, old.as_bytes());
            }
        }
        for line in early {
            chain.append(line);
        }
        chain.saved_head = header.head;
        chain.saved_count = trail.records.len();
        chain.saved_digest = Some(kernel_core::sha256::digest(&bytes));
        chain.boot = header.boot.saturating_add(1);
    }
    report(status, trail.records.len(), Some(header.head))
}

/// Verify the stored trail now, WITHOUT adopting it (`audit verify`).
///
/// The running kernel's chain is the reference: the file must verify on its
/// own AND be byte-for-byte the trail this boot last saved or recovered. A
/// trail replaced while the system runs — even by one whose chain is valid —
/// is `TAMPERED`, and the live chain is untouched, so the next save writes
/// the true history back rather than continuing the forgery. (Before V0.11
/// this re-ran boot recovery: it dropped this boot's unsaved records from the
/// chain and bumped the boot counter, so the next save was reported as
/// tampered on the following boot.)
pub fn check() -> RecoverStatus {
    let (expected, saved_count) = {
        let chain = CHAIN.lock();
        (chain.saved_digest, chain.saved_count)
    };
    let bytes = read_trail();
    let (status, reason, records, head) = match &bytes {
        None if expected.is_some() => (RecoverStatus::Tampered, "missing", 0, None),
        None => (RecoverStatus::Absent, "none", 0, None),
        Some(bytes) => match parse_trail(bytes) {
            None if expected.is_some() => (RecoverStatus::Tampered, "unreadable", 0, None),
            None => (RecoverStatus::Unreadable, "format", 0, None),
            Some(trail) => {
                let (status, reason) = if !trail.verifies() {
                    (RecoverStatus::Tampered, "chain")
                } else if expected != Some(kernel_core::sha256::digest(bytes)) {
                    (RecoverStatus::Tampered, "replaced")
                } else {
                    (RecoverStatus::Verified, "none")
                };
                (status, reason, trail.records.len(), Some(trail.header.head))
            }
        },
    };
    let mut buf = [0u8; 64];
    let head_text = match &head {
        Some(h) => kernel_core::audit_chain::format_head(h, &mut buf),
        None => "-",
    };
    crate::serial_println!(
        "[ITISYOU:AUDIT] trail_checked status={} reason={reason} records={records} head={head_text} saved_records={saved_count}",
        status.name()
    );
    status
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

/// The live chain head, the boot counter, the records the last save stored
/// and the records the next save would store, for the `audit` command.
pub fn chain_state() -> (kernel_core::audit_chain::Head, u64, usize, usize) {
    let chain = CHAIN.lock();
    (
        chain.head,
        chain.boot,
        chain.saved_count,
        chain.window.len(),
    )
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

/// Record a denial with a short reason (V0.10), for refusals that are not
/// capability checks — e.g. `console_read` by a process that does not own
/// the console's input.
pub fn denied_reason(action: &'static str, cap: u64, reason: &'static str) {
    push(action, cap, false, Some(alloc::format!("reason={reason}")));
}

/// Record a permitted privileged action (with optional short detail).
pub fn allowed(action: &'static str, cap: u64, detail: Option<String>) {
    push(action, cap, true, detail);
}

/// Total records ever written / total denials (diagnostic evidence).
pub fn counts() -> (u64, u64) {
    (SEQ.load(Ordering::SeqCst), DENIALS.load(Ordering::SeqCst))
}

/// Records in the ring now, and how many of them are denials (V0.11, the
/// approved view's recent-activity counts).
pub fn ring_counts() -> (usize, usize) {
    let ring = RING.lock();
    (ring.len(), ring.iter().filter(|r| !r.ok).count())
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
