//! Capability-delegation test parent (V0.7). Launched with only
//! {spawn, fs_read}. It spawns `cap-child` REQUESTING gui+spawn+fs_read —
//! the kernel must intersect that with what this parent actually holds, so
//! the child gets {spawn, fs_read} and NO gui. The child verifies its own
//! authority; this parent verifies the child succeeded.

#![no_std]
#![no_main]

use ulib::{exit, spawn_caps, wait, write, write_u64, CAP_FS_READ, CAP_GUI, CAP_SPAWN};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // Request MORE than we hold (gui) — delegation must intersect it away.
    let child = spawn_caps("/bin/cap-child", CAP_GUI | CAP_SPAWN | CAP_FS_READ);
    if child > u64::MAX - 16 {
        write("CAP-PARENT-SPAWN-FAILED\n");
        exit(1);
    }
    let status = wait(child);
    if status == 0 {
        write("CAP-DELEGATION-OK\n");
        exit(0)
    }
    write("CAP-DELEGATION-FAILED status=");
    write_u64(status);
    write("\n");
    exit(1)
}
