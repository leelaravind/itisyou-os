//! Capability-delegation test child (V0.7). Spawned by `cap-parent`, which
//! requested gui+spawn+fs_read for us while itself holding only
//! {spawn, fs_read}. Amplification must have been refused: gui must be
//! DENIED here, while the legitimately delegated fs_read must WORK.

#![no_std]
#![no_main]

use ulib::{exit, fs_read, gui_create, write, ERR_PERM};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // The parent never held CAP_GUI, so neither can we (no amplification).
    if gui_create(50, 50, 0, 0, "x") != ERR_PERM {
        write("CAP-CHILD-AMPLIFIED\n");
        exit(2);
    }
    // fs_read WAS legitimately delegated — it must work.
    let mut buf = [0u8; 128];
    let n = fs_read("/etc/version", &mut buf);
    if n == 0 || n > 128 {
        write("CAP-CHILD-FSREAD-FAILED\n");
        exit(3);
    }
    write("CAP-CHILD-OK\n");
    exit(0)
}
