//! `flapd` — a background service that always dies (V0.8).
//!
//! Declared `long_running`, but it exits cleanly almost immediately. That is
//! the point: for a daemon, returning IS a failure, so the supervisor must
//! treat a clean exit exactly like a crash — restart it under the bounded
//! policy, and after the ceiling mark it Failed and stop.
//!
//! It exists to prove the failure path of background supervision: crash
//! containment, a BOUNDED restart count (never a restart storm), and a system
//! that stays healthy and interactive while one of its services is broken.

#![no_std]
#![no_main]

use ulib::{exit, write, yield_now};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("FLAPD-START\n");
    // Yield once so the supervisor observes a genuinely scheduled process
    // rather than one that never ran.
    yield_now();
    // A daemon returning to its caller: clean by process semantics, a failure
    // by service semantics.
    exit(0)
}
