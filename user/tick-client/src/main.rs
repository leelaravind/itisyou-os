//! `tickc` — proves the persistent `tickd` service is still *serving*, not
//! merely still resident (V0.8).
//!
//! Run as a co-scheduled background job (`bg /bin/tick-client`) so it and
//! `tickd` are on the CPU in the same period. It sends one `stat` request and
//! prints the live pass count `tickd` replies with — a number that only the
//! long-running service could produce, and that grows between runs.

#![no_std]
#![no_main]

use ulib::{exit, msg_recv, msg_send, write, write_u64, yield_now, ERR_AGAIN};

const REQ_CH: u64 = 2;
const REP_CH: u64 = 3;

/// Bounded so a missing service fails the test instead of hanging it.
const BUDGET: u32 = 200_000;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut budget = BUDGET;
    while msg_send(REQ_CH, b"stat") == ERR_AGAIN {
        yield_now();
        budget -= 1;
        if budget == 0 {
            write("TICKC-SEND-TIMEOUT\n");
            exit(1)
        }
    }
    let mut buf = [0u8; 8];
    loop {
        let n = msg_recv(REP_CH, &mut buf);
        if n == 8 {
            let passes = u64::from_le_bytes(buf);
            if passes == 0 {
                write("TICKC-BAD-REPLY\n");
                exit(3)
            }
            write("TICKC-OK passes=");
            write_u64(passes);
            write("\n");
            exit(0)
        }
        if n != ERR_AGAIN {
            write("TICKC-BAD-REPLY\n");
            exit(3)
        }
        yield_now();
        budget -= 1;
        if budget == 0 {
            write("TICKC-RECV-TIMEOUT\n");
            exit(2)
        }
    }
}
