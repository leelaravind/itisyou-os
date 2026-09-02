//! Service client (V0.7): a oneshot supervised task that depends on `echod`.
//! Sends three pings on channel 0 and requires three pongs on channel 1 —
//! proof that a supervised service actually served requests over the safe
//! IPC boundary. Bounded retries; exits nonzero on any shortfall.

#![no_std]
#![no_main]

use ulib::{exit, msg_recv, msg_send, write, yield_now, ERR_AGAIN};

const REQ_CH: u64 = 0;
const REP_CH: u64 = 1;

fn send_ping(budget: &mut u32) -> bool {
    while *budget > 0 {
        *budget -= 1;
        if msg_send(REQ_CH, b"ping") != ERR_AGAIN {
            return true;
        }
        yield_now();
    }
    false
}

fn recv_pong(budget: &mut u32) -> bool {
    let mut buf = [0u8; 64];
    while *budget > 0 {
        *budget -= 1;
        let n = msg_recv(REP_CH, &mut buf);
        if n == ERR_AGAIN {
            yield_now();
            continue;
        }
        return n <= 64 && &buf[..n as usize] == b"pong";
    }
    false
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut budget = 200_000u32;
    let mut got = 0u32;
    for _ in 0..3 {
        if send_ping(&mut budget) && recv_pong(&mut budget) {
            got += 1;
        }
    }
    if got == 3 {
        write("SVC-CLIENT-OK\n");
        exit(0)
    }
    write("SVC-CLIENT-FAILED\n");
    exit(1)
}
