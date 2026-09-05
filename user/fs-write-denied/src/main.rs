//! `fs-write-denied` — the adversarial half of the filesystem-write test.
//!
//! Run with `fs_read` and nothing else, it must be able to READ the persistent
//! store and unable to CHANGE it. That pairing is the point: a program that
//! could neither read nor write would prove only that a capability-less
//! process is refused, which the sandbox tests already show. Here the process
//! holds real filesystem authority — just not the write RIGHT — so every
//! refusal below is the right/scope machinery working, not a blanket denial.

#![no_std]
#![no_main]

use ulib::{exit, fs_delete, fs_list, fs_read, fs_write, write, ERR_PERM};

const PATH: &str = "/data/denied-probe.txt";

fn must_deny(what: &str, result: u64) -> bool {
    if result == ERR_PERM {
        write("FSDENY-OK call=");
        write(what);
        write("\n");
        true
    } else {
        write("FSDENY-LEAK call=");
        write(what);
        write("\n");
        false
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut all = true;
    all &= must_deny("fs_write", fs_write(PATH, b"should-never-land"));
    all &= must_deny("fs_delete", fs_delete("/data/planted.txt"));

    // Reading is still permitted: the process holds the READ right. The file
    // was planted by the shell before this program ran, so a successful read
    // proves the store is genuinely reachable — the writes above were refused
    // on authority, not because the store was missing.
    let mut buf = [0u8; 64];
    let n = fs_read("/data/planted.txt", &mut buf);
    if n != 7 || &buf[..7] != b"planted" {
        write("FSDENY-READ-FAILED\n");
        exit(2)
    }
    write("FSDENY-READ-STILL-ALLOWED\n");

    // Listing needs only READ as well.
    let mut listing = [0u8; 256];
    if fs_list(&mut listing) == ERR_PERM {
        write("FSDENY-LIST-WRONGLY-DENIED\n");
        exit(3)
    }
    write("FSDENY-LIST-STILL-ALLOWED\n");

    if all {
        write("FSDENY-ALL-DENIED\n");
        exit(0)
    }
    write("FSDENY-FAILED\n");
    exit(1)
}
