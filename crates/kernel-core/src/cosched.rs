//! Always-on co-scheduling policy (V0.10, SCHED10-001; ADR-0022).
//!
//! The kernel is not preemptible and the timer interrupt does not schedule.
//! Instead, background processes get bounded slices at a few audited "safe
//! points" in kernel code — the console prompt, job waits, every network
//! poll, the desktop's idle loop. This module is the pure decision: may this
//! safe point run a slice now? And, afterwards: did a command's window starve
//! the background?
//!
//! The gate is checked in a fixed order (the design's rule R2), so the reason
//! reported for a skipped slice is always the most fundamental one.

/// Ticks between two slices taken at busy safe points (100 Hz: 50 ms).
pub const PERIOD_TICKS: u64 = 5;
/// A slice runs at most this many ticks...
pub const SLICE_TICKS: u64 = 1;
/// ...and at most this many Ring 3 quanta...
pub const SLICE_QUANTA: u32 = 32;
/// ...and at most this long in wall time (TSC), whichever comes first.
pub const SLICE_MAX_MS: u64 = 20;
/// A window is starved if the background went longer than this without a
/// quantum.
pub const GAP_BOUND_MS: u64 = 250;
/// Windows shorter than this are too short to judge.
pub const JUDGE_MIN_MS: u64 = 100;

/// Where a slice was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// Kernel work in progress (a network wait, the desktop loop).
    Busy,
    /// The console is idle, waiting for input.
    Idle,
    /// The console is waiting for a job that runs only in slices.
    JobWait,
}

/// Why a safe point did not slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// Interrupts are masked: this is a syscall or an interrupt handler.
    IfMasked,
    /// A run-loop is already running processes.
    RunLoop,
    /// A Ring 3 quantum's state is live (a syscall path).
    InQuantum,
    /// The kernel holds a counted lock.
    LockHeld,
    /// A non-schedulable region (e.g. an open NVMe store) is active.
    NoSched,
    /// Always-on scheduling is not enabled (the selftest image).
    Disabled,
    /// Scheduling was paused by the console (`sched pause`).
    Paused,
    /// The console is mid-line on the serial port.
    MidLine,
    /// A slice ran less than [`PERIOD_TICKS`] ago.
    Period,
}

impl Skip {
    pub const fn name(self) -> &'static str {
        match self {
            Skip::IfMasked => "if_masked",
            Skip::RunLoop => "run_loop",
            Skip::InQuantum => "in_quantum",
            Skip::LockHeld => "lock_held",
            Skip::NoSched => "no_sched",
            Skip::Disabled => "disabled",
            Skip::Paused => "paused",
            Skip::MidLine => "mid_line",
            Skip::Period => "period",
        }
    }
}

/// Everything the decision depends on, sampled at the safe point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    pub enabled: bool,
    pub paused: bool,
    pub if_enabled: bool,
    pub runloop_active: bool,
    pub in_quantum: bool,
    pub no_sched_depth: u32,
    pub lock_depth: u32,
    pub at_line_start: bool,
    pub now_tick: u64,
    pub last_slice_tick: u64,
}

/// May this safe point slice now? Checks in the fixed R2 order: the
/// context conditions first (never slice from a syscall, an ISR, a run-loop
/// or under a lock), then the policy conditions. A job wait ignores pause,
/// line position and period, because its job only ever runs in slices; an
/// idle point ignores line position and period.
pub fn decide(g: &Gate, point: Point) -> Result<(), Skip> {
    if !g.if_enabled {
        return Err(Skip::IfMasked);
    }
    if g.runloop_active {
        return Err(Skip::RunLoop);
    }
    if g.in_quantum {
        return Err(Skip::InQuantum);
    }
    if g.lock_depth != 0 {
        return Err(Skip::LockHeld);
    }
    if g.no_sched_depth != 0 {
        return Err(Skip::NoSched);
    }
    if point == Point::JobWait {
        return Ok(());
    }
    if !g.enabled {
        return Err(Skip::Disabled);
    }
    if g.paused {
        return Err(Skip::Paused);
    }
    if point == Point::Idle {
        return Ok(());
    }
    if !g.at_line_start {
        return Err(Skip::MidLine);
    }
    if g.now_tick.saturating_sub(g.last_slice_tick) < PERIOD_TICKS {
        return Err(Skip::Period);
    }
    Ok(())
}

/// What happened to the background during one console command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Window {
    pub ms: u64,
    pub slices: u64,
    /// Quanta given to processes other than the console's own job.
    pub other_quanta: u64,
    /// Longest stretch, in ms, with no such quantum.
    pub max_gap_ms: u64,
    pub paused: bool,
}

impl Window {
    /// Long enough to judge.
    pub fn judged(&self) -> bool {
        self.ms >= JUDGE_MIN_MS
    }

    /// The background starved: nothing ran, or it waited too long.
    pub fn starved(&self) -> bool {
        self.judged() && (self.other_quanta == 0 || self.max_gap_ms > GAP_BOUND_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Gate {
        Gate {
            enabled: true,
            paused: false,
            if_enabled: true,
            runloop_active: false,
            in_quantum: false,
            no_sched_depth: 0,
            lock_depth: 0,
            at_line_start: true,
            now_tick: 100,
            last_slice_tick: 0,
        }
    }

    #[test]
    fn an_open_gate_slices_at_every_point() {
        for p in [Point::Busy, Point::Idle, Point::JobWait] {
            assert_eq!(decide(&open(), p), Ok(()));
        }
    }

    #[test]
    fn skip_reasons_follow_the_fixed_precedence() {
        // Every condition false at once: the first in R2 order wins, and
        // each fix reveals the next.
        let mut g = Gate {
            enabled: false,
            paused: true,
            if_enabled: false,
            runloop_active: true,
            in_quantum: true,
            no_sched_depth: 1,
            lock_depth: 2,
            at_line_start: false,
            now_tick: 10,
            last_slice_tick: 9,
        };
        let order = [
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
        for expect in order {
            assert_eq!(decide(&g, Point::Busy), Err(expect), "{}", expect.name());
            match expect {
                Skip::IfMasked => g.if_enabled = true,
                Skip::RunLoop => g.runloop_active = false,
                Skip::InQuantum => g.in_quantum = false,
                Skip::LockHeld => g.lock_depth = 0,
                Skip::NoSched => g.no_sched_depth = 0,
                Skip::Disabled => g.enabled = true,
                Skip::Paused => g.paused = false,
                Skip::MidLine => g.at_line_start = true,
                Skip::Period => g.last_slice_tick = 0,
            }
        }
        assert_eq!(decide(&g, Point::Busy), Ok(()));
    }

    #[test]
    fn context_conditions_bind_every_point() {
        for p in [Point::Busy, Point::Idle, Point::JobWait] {
            let mut g = open();
            g.if_enabled = false;
            assert_eq!(decide(&g, p), Err(Skip::IfMasked));
            let mut g = open();
            g.lock_depth = 1;
            assert_eq!(decide(&g, p), Err(Skip::LockHeld));
            let mut g = open();
            g.no_sched_depth = 1;
            assert_eq!(decide(&g, p), Err(Skip::NoSched));
        }
    }

    #[test]
    fn a_job_wait_ignores_pause_line_position_period_and_disable() {
        let mut g = open();
        g.enabled = false;
        g.paused = true;
        g.at_line_start = false;
        g.last_slice_tick = g.now_tick;
        assert_eq!(decide(&g, Point::JobWait), Ok(()));
    }

    #[test]
    fn an_idle_point_ignores_line_position_and_period_but_not_pause() {
        let mut g = open();
        g.at_line_start = false;
        g.last_slice_tick = g.now_tick;
        assert_eq!(decide(&g, Point::Idle), Ok(()));
        g.paused = true;
        assert_eq!(decide(&g, Point::Idle), Err(Skip::Paused));
    }

    #[test]
    fn the_period_boundary_is_exact() {
        let mut g = open();
        g.last_slice_tick = g.now_tick - (PERIOD_TICKS - 1);
        assert_eq!(decide(&g, Point::Busy), Err(Skip::Period));
        g.last_slice_tick = g.now_tick - PERIOD_TICKS;
        assert_eq!(decide(&g, Point::Busy), Ok(()));
    }

    #[test]
    fn windows_are_judged_only_when_long_enough() {
        let short = Window {
            ms: JUDGE_MIN_MS - 1,
            ..Window::default()
        };
        assert!(!short.judged() && !short.starved());
        let long_idle = Window {
            ms: JUDGE_MIN_MS,
            ..Window::default()
        };
        assert!(long_idle.judged() && long_idle.starved());
    }

    #[test]
    fn a_gap_over_the_bound_starves_even_with_progress() {
        let w = Window {
            ms: 1000,
            slices: 10,
            other_quanta: 40,
            max_gap_ms: GAP_BOUND_MS + 1,
            paused: false,
        };
        assert!(w.starved());
        let ok = Window {
            max_gap_ms: GAP_BOUND_MS,
            ..w
        };
        assert!(!ok.starved());
    }
}
