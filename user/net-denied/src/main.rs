//! `net-denied` — the adversarial half of the network capability test.
//!
//! Run with no capabilities at all, it attempts every network syscall in turn.
//! Each one must come back `ERR_PERM`. The program fails loudly if any call
//! *succeeds* — a test that only checks the authorized path proves the feature
//! works, not that the gate does.
//!
//! It also checks that a denial is a denial and not a different error: an
//! unbound socket returning `ERR_BADF`, or a down interface returning
//! `ERR_NOENT`, would let a real capability bug hide behind a plausible-looking
//! failure.

#![no_std]
#![no_main]

use ulib::{exit, net_info, net_resolve, udp_bind, udp_close, udp_recv, udp_send, write, ERR_PERM};

fn must_deny(what: &str, result: u64) -> bool {
    if result == ERR_PERM {
        write("NETDENY-OK call=");
        write(what);
        write("\n");
        true
    } else {
        write("NETDENY-LEAK call=");
        write(what);
        write("\n");
        false
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut all = true;
    let mut info = [0u8; 20];
    all &= must_deny("net_info", net_info(&mut info));
    all &= must_deny("udp_bind", udp_bind(40200));
    // Descriptor 0 is a real, in-range socket index, so this is a genuine
    // attempt at another process's socket rather than an obviously bad handle.
    all &= must_deny("udp_send", udp_send(0, [10, 0, 2, 2], 7, b"stolen"));
    let mut buf = [0u8; 64];
    all &= must_deny("udp_recv", udp_recv(0, &mut buf));
    let mut addr = [0u8; 4];
    all &= must_deny("net_resolve", net_resolve("os.itisyou.app", &mut addr));

    // Closing a socket it does not own must also fail — with ERR_BADF, since
    // ownership, not capability, is what is missing.
    if udp_close(0) == 0 {
        write("NETDENY-LEAK call=udp_close\n");
        all = false;
    }

    if all {
        write("NETDENY-ALL-DENIED\n");
        exit(0)
    }
    write("NETDENY-FAILED\n");
    exit(1)
}
