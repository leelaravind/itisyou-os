//! Always-on co-scheduling (V0.10, SCHED10-001; ADR-0022).
//!
//! Background processes get bounded slices at a few audited "safe points"
//! in kernel code. [`kernel_core::cosched::decide`] is the policy; this
//! module samples the live state for it, runs the slice, and measures what
//! the background actually got. It holds no lock of its own (atomics only),
//! so it can be called from any safe point; the one lock it takes — to store
//! a finished command window — is taken outside any slice.

use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use kernel_core::cosched::{self, Gate, Point, Skip, Window};

static ENABLED: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static NO_SCHED: AtomicU32 = AtomicU32::new(0);
static LAST_SLICE_TICK: AtomicU64 = AtomicU64::new(0);

static SLICES: AtomicU64 = AtomicU64::new(0);
static QUANTA: AtomicU64 = AtomicU64::new(0);
static OTHER_QUANTA: AtomicU64 = AtomicU64::new(0);
/// Slices refused per reason, indexed like [`SKIP_ORDER`].
static SKIPS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
const SKIP_ORDER: [Skip; 9] = [
    Skip::IfMasked,
    Skip::RunLoop,
    Skip::InQuantum,
    Skip::LockHeld,
    Skip::NoSched,
    Skip::Disabled,
    Skip::Paused,
    Skip::MidLine,
    Skip::Period,
];

/// The pid the console is waiting for (`bg`), whose quanta are not
/// "background" for starvation purposes; 0 = none.
static JOB: AtomicU64 = AtomicU64::new(0);

/// TSC of the last background quantum, and the longest gap seen (ms).
static LAST_OTHER_TSC: AtomicU64 = AtomicU64::new(0);
static MAX_GAP_MS: AtomicU64 = AtomicU64::new(0);

/// The console command whose window is open, and its running counters.
static WIN_ACTIVE: AtomicBool = AtomicBool::new(false);
static WIN_START_TSC: AtomicU64 = AtomicU64::new(0);
static WIN_SLICES: AtomicU64 = AtomicU64::new(0);
static WIN_OTHER: AtomicU64 = AtomicU64::new(0);
static WIN_LAST_OTHER_TSC: AtomicU64 = AtomicU64::new(0);
static WIN_MAX_GAP_MS: AtomicU64 = AtomicU64::new(0);

/// The last finished window, for `sched last`.
static LAST_WINDOW: Mutex<Option<(CmdName, Window)>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct CmdName {
    bytes: [u8; 15],
    len: usize,
}

impl CmdName {
    fn new(s: &str) -> Self {
        let mut bytes = [0u8; 15];
        let n = s.len().min(15);
        bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        CmdName { bytes, len: n }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}

fn ms_of(cycles: u64) -> u64 {
    cycles / crate::interrupts::cycles_for_ms(1).max(1)
}

/// Turn always-on scheduling on (the interactive kernel only; never the
/// selftest image, whose checks count every process step).
pub fn enable() {
    LAST_OTHER_TSC.store(crate::interrupts::tsc(), Relaxed);
    ENABLED.store(true, Relaxed);
    crate::serial_println!(
        "[ITISYOU:SCHED] enabled period_ticks={} slice_ticks={} slice_quanta={} gap_bound_ms={}",
        cosched::PERIOD_TICKS,
        cosched::SLICE_TICKS,
        cosched::SLICE_QUANTA,
        cosched::GAP_BOUND_MS
    );
}

pub fn enabled() -> bool {
    ENABLED.load(Relaxed)
}

pub fn pause() {
    PAUSED.store(true, Relaxed);
}

pub fn resume() {
    PAUSED.store(false, Relaxed);
}

pub fn paused() -> bool {
    PAUSED.load(Relaxed)
}

/// A non-schedulable region (e.g. an open NVMe store): while any token is
/// alive, no safe point slices.
pub struct NoSched;

impl NoSched {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        NO_SCHED.fetch_add(1, Relaxed);
        NoSched
    }
}

impl Drop for NoSched {
    fn drop(&mut self) {
        NO_SCHED.fetch_sub(1, Relaxed);
    }
}

pub fn no_sched_depth() -> u32 {
    NO_SCHED.load(Relaxed)
}

fn gate() -> Gate {
    Gate {
        enabled: ENABLED.load(Relaxed),
        paused: PAUSED.load(Relaxed),
        if_enabled: x86_64::instructions::interrupts::are_enabled(),
        runloop_active: crate::proc::runloop_active(),
        in_quantum: crate::syscall::CURRENT_PID.load(Relaxed) != 0,
        no_sched_depth: NO_SCHED.load(Relaxed),
        lock_depth: crate::sync::depth() as u32,
        at_line_start: crate::serial::at_line_start(),
        now_tick: crate::interrupts::ticks(),
        last_slice_tick: LAST_SLICE_TICK.load(Relaxed),
    }
}

fn try_slice(point: Point) -> bool {
    match cosched::decide(&gate(), point) {
        Ok(()) => {
            slice();
            true
        }
        Err(skip) => {
            if let Some(i) = SKIP_ORDER.iter().position(|s| *s == skip) {
                SKIPS[i].fetch_add(1, Relaxed);
            }
            false
        }
    }
}

/// Kernel work in progress (network waits, the desktop loop, a foreground
/// program between quanta).
pub fn safe_point() -> bool {
    try_slice(Point::Busy)
}

/// The console is idle at its prompt.
pub fn idle_point() -> bool {
    try_slice(Point::Idle)
}

/// The console is waiting for `job`, which only ever runs in slices.
pub fn console_wait_step(job: u64) -> bool {
    JOB.store(job, Relaxed);
    try_slice(Point::JobWait)
}

fn slice() {
    LAST_SLICE_TICK.store(crate::interrupts::ticks(), Relaxed);
    SLICES.fetch_add(1, Relaxed);
    if WIN_ACTIVE.load(Relaxed) {
        WIN_SLICES.fetch_add(1, Relaxed);
    }
    crate::services::pump_slice();
}

/// Called by the run-loop before each quantum.
pub fn note_quantum(pid: u64) {
    QUANTA.fetch_add(1, Relaxed);
    if pid == JOB.load(Relaxed) && pid != 0 {
        return;
    }
    OTHER_QUANTA.fetch_add(1, Relaxed);
    let now = crate::interrupts::tsc();
    let prev = LAST_OTHER_TSC.swap(now, Relaxed);
    if prev != 0 {
        MAX_GAP_MS.fetch_max(ms_of(now.saturating_sub(prev)), Relaxed);
    }
    if WIN_ACTIVE.load(Relaxed) {
        WIN_OTHER.fetch_add(1, Relaxed);
        let wprev = WIN_LAST_OTHER_TSC.swap(now, Relaxed);
        WIN_MAX_GAP_MS.fetch_max(ms_of(now.saturating_sub(wprev)), Relaxed);
    }
}

/// Nothing was runnable: an empty queue is not starvation, so restart the
/// gap clocks.
pub fn note_runq_empty() {
    let now = crate::interrupts::tsc();
    LAST_OTHER_TSC.store(now, Relaxed);
    if WIN_ACTIVE.load(Relaxed) {
        WIN_LAST_OTHER_TSC.store(now, Relaxed);
    }
}

pub fn clear_job() {
    JOB.store(0, Relaxed);
}

pub fn quanta_total() -> u64 {
    QUANTA.load(Relaxed)
}

pub fn other_quanta_total() -> u64 {
    OTHER_QUANTA.load(Relaxed)
}

pub fn slices_total() -> u64 {
    SLICES.load(Relaxed)
}

/// Measures one console command: opened when the command starts, recorded
/// when it ends.
pub struct CommandWindow {
    cmd: CmdName,
}

impl CommandWindow {
    pub fn begin(cmd: &str) -> Self {
        let now = crate::interrupts::tsc();
        WIN_START_TSC.store(now, Relaxed);
        WIN_LAST_OTHER_TSC.store(now, Relaxed);
        WIN_SLICES.store(0, Relaxed);
        WIN_OTHER.store(0, Relaxed);
        WIN_MAX_GAP_MS.store(0, Relaxed);
        WIN_ACTIVE.store(true, Relaxed);
        CommandWindow {
            cmd: CmdName::new(cmd),
        }
    }
}

impl Drop for CommandWindow {
    fn drop(&mut self) {
        WIN_ACTIVE.store(false, Relaxed);
        let now = crate::interrupts::tsc();
        // The stretch since the last background quantum counts too.
        let tail = ms_of(now.saturating_sub(WIN_LAST_OTHER_TSC.load(Relaxed)));
        let w = Window {
            ms: ms_of(now.saturating_sub(WIN_START_TSC.load(Relaxed))),
            slices: WIN_SLICES.load(Relaxed),
            other_quanta: WIN_OTHER.load(Relaxed),
            max_gap_ms: WIN_MAX_GAP_MS.load(Relaxed).max(tail),
            paused: PAUSED.load(Relaxed),
        };
        *LAST_WINDOW.lock() = Some((self.cmd, w));
    }
}

fn skips(skip: Skip) -> u64 {
    SKIP_ORDER
        .iter()
        .position(|s| *s == skip)
        .map_or(0, |i| SKIPS[i].load(Relaxed))
}

/// The `sched` report.
pub fn report() {
    crate::serial_println!(
        "[ITISYOU:SCHED] always_on={} paused={} period_ticks={} slice_ticks={} slice_quanta={} slices={} quanta={} other_quanta={} max_gap_ms={} skips_period={} skips_midline={} skips_nosched={} lock_skips={}",
        enabled(),
        paused(),
        cosched::PERIOD_TICKS,
        cosched::SLICE_TICKS,
        cosched::SLICE_QUANTA,
        SLICES.load(Relaxed),
        QUANTA.load(Relaxed),
        OTHER_QUANTA.load(Relaxed),
        MAX_GAP_MS.load(Relaxed),
        skips(Skip::Period),
        skips(Skip::MidLine),
        skips(Skip::NoSched),
        skips(Skip::LockHeld)
    );
}

/// The `sched last` report: the most recent finished command window.
pub fn report_last() {
    let last = *LAST_WINDOW.lock();
    match last {
        Some((cmd, w)) => crate::serial_println!(
            "[ITISYOU:SCHED] window cmd={} paused={} judged={} starved={} ms={} slices={} other_quanta={} max_gap_ms={}",
            cmd.as_str(),
            w.paused,
            w.judged(),
            w.starved(),
            w.ms,
            w.slices,
            w.other_quanta,
            w.max_gap_ms
        ),
        None => crate::serial_println!("[ITISYOU:SCHED] window none"),
    }
}
