//! `tickd` — a PERSISTENT Ring 3 system service (V0.8).
//!
//! Unlike the V0.7 services, which ran to completion and exited, this one
//! never terminates: it is the evidence that the OS can host long-lived
//! userspace work co-scheduled with an interactive shell, rather than only
//! run-to-completion tasks under a supervisor that waits for them.
//!
//! It does two things forever:
//!   * emits a heartbeat every two seconds of REAL time, carrying the kernel
//!     tick it fired at, so the test can prove it is still making progress
//!     much later in the boot;
//!   * answers `stat` requests over IPC with its own live pass count, so the
//!     test can prove it is still *serving*, not merely still resident — and
//!     that the SAME instance served twice, since a restart would reset it.
//!
//! It yields on every pass, so it can never starve the shell or the other
//! services sharing the CPU.

#![no_std]
#![no_main]

use ulib::{msg_recv, msg_send, uptime_ticks, write, write_u64, yield_now, ERR_AGAIN};

/// Requests arrive here; replies go out on the next channel up.
const REQ_CH: u64 = 2;
const REP_CH: u64 = 3;

/// Heartbeat cadence in kernel ticks (100 Hz), i.e. one line every two
/// seconds.
///
/// Deliberately measured in TIME rather than in scheduling passes: the pass
/// rate is a function of system load, so a pass-based cadence emits a couple
/// of lines while the machine is busy and hundreds per second while it is
/// idle -- drowning the serial console it shares with the shell. Ticks make
/// the heartbeat mean what an operator reads it to mean: the service was
/// alive this recently.
const HEARTBEAT_TICKS: u64 = 200;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("TICKD-READY\n");
    let mut passes = 0u64;
    let mut served = 0u64;
    let mut buf = [0u8; 64];
    // Absolute deadline rather than a countdown, so a long stall cannot
    // silently shift the cadence.
    let mut next_beat = uptime_ticks() + HEARTBEAT_TICKS;
    loop {
        passes += 1;

        // Serve any pending request without blocking: a service that blocks
        // on IPC cannot also heartbeat, and a stalled heartbeat is
        // indistinguishable from a dead service.
        let n = msg_recv(REQ_CH, &mut buf);
        if n != ERR_AGAIN && n <= buf.len() as u64 && &buf[..n as usize] == b"stat" {
            // Reply with the live pass count so the client sees real state.
            let mut reply = [0u8; 8];
            reply.copy_from_slice(&passes.to_le_bytes());
            if msg_send(REP_CH, &reply) != ERR_AGAIN {
                served += 1;
                write("TICKD-SERVED n=");
                write_u64(served);
                write("\n");
            }
        }

        let now = uptime_ticks();
        if now >= next_beat {
            // Re-base on `now` instead of advancing the old deadline: after a
            // busy stretch that would fire a burst of back-dated heartbeats.
            next_beat = now + HEARTBEAT_TICKS;
            write("TICKD-ALIVE tick=");
            write_u64(now);
            write(" passes=");
            write_u64(passes);
            write(" served=");
            write_u64(served);
            write("\n");
        }
        yield_now();
    }
}
