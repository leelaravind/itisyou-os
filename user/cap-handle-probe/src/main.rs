//! V0.8 capability-handle enforcement probe.
//!
//! Proves, from Ring 3 against the real kernel, that authority is decided by
//! the live capability table rather than a static bitmask fixed at load time:
//!
//!   1. opaque handles are issued, and a forged generation is refused;
//!   2. rights inside a handle are honoured — dropping WRITE from the
//!      filesystem handle leaves READ working;
//!   3. an EXPIRED handle stops working without anyone revoking it;
//!   4. a REVOKED handle stops working on the very next syscall.
//!
//! (3) and (4) are the ones a bitmask model cannot express at all: under V0.7
//! the process's `fs_read` bit was set for its entire lifetime, so no runtime
//! event could have withdrawn the authority.

#![no_std]
#![no_main]

use ulib::{
    cap_check, cap_list, cap_restrict, cap_revoke, exit, fs_read, write, yield_now, ERR_PERM,
    KIND_FILESYSTEM, RIGHT_READ,
};

/// Any file guaranteed to exist in the initramfs.
const PROBE_PATH: &str = "/etc/version";

fn can_read() -> bool {
    let mut buf = [0u8; 64];
    let n = fs_read(PROBE_PATH, &mut buf);
    n > 0 && n <= buf.len() as u64
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut handles = [0u64; 16];
    let count = cap_list(&mut handles);
    if count == 0 || handles[KIND_FILESYSTEM as usize] == 0 {
        write(
            "CAPH-LIST-FAILED
",
        );
        exit(1)
    }

    // 1. No application gets to manufacture a valid generation. Flipping the
    //    generation field must fail regardless of the handle's resource kind.
    let forged = handles[KIND_FILESYSTEM as usize] ^ (1u64 << 32);
    if cap_check(forged, KIND_FILESYSTEM, 0) != ERR_PERM {
        write(
            "CAPH-FORGE-ESCAPE
",
        );
        exit(2)
    }
    write(
        "CAPH-FORGED-DENIED
",
    );

    // Baseline: the authority we are about to narrow currently works.
    if !can_read() {
        write(
            "CAPH-BASELINE-FAILED
",
        );
        exit(3)
    }

    // 2. Narrow the filesystem handle to READ only. Reads must survive
    //    (narrowing removed nothing we were using).
    if cap_restrict(KIND_FILESYSTEM, RIGHT_READ, 0) != 0 {
        write(
            "CAPH-RESTRICT-FAILED
",
        );
        exit(4)
    }
    if !can_read() {
        write(
            "CAPH-RESTRICT-BROKE-READ
",
        );
        exit(5)
    }
    write(
        "CAPH-RESTRICTED-READ-OK
",
    );

    // 3. Give the same handle a short lifetime, then outlive it. Nothing
    //    revokes it — it simply stops being valid, which the kernel can only
    //    notice because it revalidates on every call.
    if cap_restrict(KIND_FILESYSTEM, RIGHT_READ, 2) != 0 {
        write(
            "CAPH-EXPIRY-SETUP-FAILED
",
        );
        exit(6)
    }
    // Yield until the expiry tick passes; the timer advances while other
    // work runs, and each yield is a scheduling point rather than a busy spin.
    let mut spins = 0u32;
    while can_read() {
        yield_now();
        spins += 1;
        if spins > 100_000 {
            write(
                "CAPH-EXPIRY-NEVER-FIRED
",
            );
            exit(7)
        }
    }
    write(
        "CAPH-EXPIRED-DENIED
",
    );

    // 4. Revocation is immediate. Re-check on the Process handle, which is
    //    still live at full strength, so this tests withdrawal rather than an
    //    already-dead authority: spawn authority works, is revoked, and the
    //    very next use is refused.
    if cap_revoke(ulib::KIND_PROCESS) != 0 {
        write(
            "CAPH-REVOKE-FAILED
",
        );
        exit(8)
    }
    if ulib::spawn("/bin/init") != ERR_PERM {
        write(
            "CAPH-REVOKE-IGNORED
",
        );
        exit(9)
    }
    write(
        "CAPH-REVOKED-DENIED
",
    );

    // Revoking something already gone must fail cleanly, not panic the kernel.
    if cap_revoke(ulib::KIND_PROCESS) != ERR_PERM {
        write(
            "CAPH-DOUBLE-REVOKE-ESCAPE
",
        );
        exit(10)
    }
    // An out-of-range kind is rejected, not treated as slot 0.
    if cap_revoke(99) != ulib::ERR_INVAL {
        write(
            "CAPH-BAD-KIND-ESCAPE
",
        );
        exit(11)
    }
    write(
        "CAPH-ENFORCEMENT-OK
",
    );
    exit(0)
}
