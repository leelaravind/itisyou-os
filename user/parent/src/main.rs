//! `parent` — drives V0.3 concurrency from Ring 3. Spawns two children,
//! yields to let them interleave, waits for both (asserting the exit code
//! the kernel delivered), round-trips an IPC message, then exits.
//!
//! Every RING3-PARENT-* line is deterministic evidence asserted by the
//! QEMU harness.

#![no_std]
#![no_main]

use ulib::{exit, msg_recv, msg_send, spawn, wait, write, write_u64, yield_now, ERR_AGAIN};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("RING3-PARENT-START\n");

    let a = spawn("/bin/child");
    let b = spawn("/bin/child");
    if a >= 1 && b >= 1 && a != b {
        write("RING3-PARENT-SPAWNED\n");
    }

    // Give the children a few slots to run before we block on them.
    for _ in 0..2 {
        yield_now();
    }

    let sa = wait(a);
    let sb = wait(b);
    if sa == 7 && sb == 7 {
        write("RING3-PARENT-WAIT-OK\n");
    } else {
        write("RING3-PARENT-WAIT-BAD a=");
        write_u64(sa);
        write(" b=");
        write_u64(sb);
        write("\n");
    }

    // IPC round-trip on channel 0.
    let recv_empty = {
        let mut buf = [0u8; 8];
        msg_recv(0, &mut buf)
    };
    let sent = msg_send(0, b"ping");
    let mut buf = [0u8; 8];
    let got = msg_recv(0, &mut buf);
    if recv_empty == ERR_AGAIN && sent == 4 && got == 4 && &buf[..4] == b"ping" {
        write("RING3-PARENT-IPC-OK\n");
    }

    write("RING3-PARENT-DONE\n");
    exit(0)
}
