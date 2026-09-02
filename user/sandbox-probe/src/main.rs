//! Adversarial sandbox probe (V0.7): launched with ZERO capabilities, this
//! program attempts every privileged syscall class and REQUIRES each to be
//! refused with ERR_PERM. It exits 0 only if every escape attempt was denied
//! — a passing run is machine-readable proof that default-deny holds.

#![no_std]
#![no_main]

use ulib::{
    devinfo, exit, fs_read, gui_create, msg_send, spawn, spawn_caps, wait, write, write_u64,
    ERR_PERM,
};

fn expect_denied(what: &str, got: u64, failures: &mut u64) {
    if got == ERR_PERM {
        write("SANDBOX-DENIED ");
        write(what);
        write("\n");
    } else {
        write("SANDBOX-ESCAPE ");
        write(what);
        write(" got=");
        write_u64(got);
        write("\n");
        *failures += 1;
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut failures = 0u64;

    // GUI: window creation must be refused.
    expect_denied("gui_create", gui_create(50, 50, 0, 0, "x"), &mut failures);
    // Devices: device-table queries must be refused.
    let mut dev = [0u8; 16];
    expect_denied("devinfo", devinfo(0, &mut dev), &mut failures);
    // Process control: spawn (both forms) and wait must be refused.
    expect_denied("spawn", spawn("/bin/init"), &mut failures);
    expect_denied(
        "spawn_caps",
        spawn_caps("/bin/init", u64::MAX),
        &mut failures,
    );
    expect_denied("wait", wait(1), &mut failures);
    // IPC: channel access must be refused.
    expect_denied("msg_send", msg_send(0, b"x"), &mut failures);
    // Filesystem: reads must be refused (no CAP_FS_READ at all).
    let mut buf = [0u8; 64];
    expect_denied("fs_read", fs_read("/etc/version", &mut buf), &mut failures);

    if failures == 0 {
        write("SANDBOX-DENIED-OK\n");
        exit(0)
    }
    write("SANDBOX-PROBE-FAILED\n");
    exit(1)
}
