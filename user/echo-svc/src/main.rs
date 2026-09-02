//! `echod` — a supervised Ring 3 system service (V0.7). Serves ping→pong
//! over IPC: requests on channel 0, replies on channel 1. Exits 0 (Done)
//! after serving three requests; a bounded poll budget guarantees it can
//! never hang the supervisor if no client ever arrives.

#![no_std]
#![no_main]

use ulib::{exit, msg_recv, msg_send, write, yield_now, ERR_AGAIN};

const REQ_CH: u64 = 0;
const REP_CH: u64 = 1;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("ECHOD-READY\n");
    let mut served = 0u32;
    let mut budget = 200_000u32;
    let mut buf = [0u8; 64];
    while served < 3 && budget > 0 {
        budget -= 1;
        let n = msg_recv(REQ_CH, &mut buf);
        if n == ERR_AGAIN {
            yield_now();
            continue;
        }
        if n <= 64 && &buf[..n as usize] == b"ping" {
            // Reply (retry while the queue is full).
            while msg_send(REP_CH, b"pong") == ERR_AGAIN {
                yield_now();
            }
            served += 1;
        }
    }
    if served == 3 {
        write("ECHOD-SERVED-3\n");
        exit(0)
    }
    write("ECHOD-TIMEOUT\n");
    exit(7)
}
