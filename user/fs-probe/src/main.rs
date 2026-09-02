//! Filesystem sandbox probe (V0.7). Launched with CAP_FS_READ but sandboxed
//! to the `/etc` prefix: in-prefix reads must work, everything else —
//! including `..` traversal attempts that normalize outside the prefix —
//! must be denied with ERR_PERM. A missing file INSIDE the sandbox is
//! ERR_NOENT (visibility, not permission).

#![no_std]
#![no_main]

use ulib::{exit, fs_read, write, write_u64, ERR_NOENT, ERR_PERM};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut buf = [0u8; 256];
    let mut failures = 0u64;

    // Allowed: inside the sandbox prefix.
    let n = fs_read("/etc/version", &mut buf);
    if n == 0 || n > 256 {
        write("FS-PROBE allowed-read failed\n");
        failures += 1;
    }
    // Denied: outside the prefix.
    if fs_read("/bin/init", &mut buf) != ERR_PERM {
        write("FS-PROBE escape: /bin/init readable\n");
        failures += 1;
    }
    if fs_read("/apps/hello-app/manifest", &mut buf) != ERR_PERM {
        write("FS-PROBE escape: foreign app dir readable\n");
        failures += 1;
    }
    // Denied: traversal that normalizes outside the prefix.
    if fs_read("/etc/../bin/init", &mut buf) != ERR_PERM {
        write("FS-PROBE escape: traversal readable\n");
        failures += 1;
    }
    // Denied: root escape is rejected outright.
    if fs_read("/../etc/version", &mut buf) != ERR_PERM {
        write("FS-PROBE escape: root-escape accepted\n");
        failures += 1;
    }
    // In-sandbox miss is NOENT, not PERM (sandbox does not mask existence
    // semantics inside the allowed view).
    if fs_read("/etc/definitely-missing", &mut buf) != ERR_NOENT {
        write("FS-PROBE wrong error for in-sandbox miss\n");
        failures += 1;
    }

    if failures == 0 {
        write("FS-SANDBOX-OK\n");
        exit(0)
    }
    write("FS-SANDBOX-FAILED n=");
    write_u64(failures);
    write("\n");
    exit(1)
}
