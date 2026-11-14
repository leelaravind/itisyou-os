//! `flakyd` - a test fixture service (V0.11, ACT11-002).
//!
//! It fails whenever it is started in the first 3 seconds of uptime and runs
//! forever when started later - so at boot /sbin/init restarts it three times
//! and gives up (the row is Failed), and a `retry-service flakyd` approved at
//! the console later on succeeds and stays up. It is what lets the retry
//! path's SUCCESS be verified; flapd, which fails every time, shows the
//! rollback. It holds no capabilities.

#![no_std]
#![no_main]

use ulib::{exit, sleep_ticks, uptime_ticks, write};

/// Ticks of uptime (100 Hz) below which it fails.
const YOUNG_TICKS: u64 = 300;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    if uptime_ticks() < YOUNG_TICKS {
        write("FLAKYD-EARLY-EXIT\n");
        exit(1)
    }
    write("FLAKYD-UP\n");
    loop {
        sleep_ticks(100);
    }
}
