//! `fs-user-persist` — proves a RING 3 write survives a real reboot.
//!
//! The V0.4 persistence test writes from the kernel; this one writes through
//! the userspace syscall path, which is a different claim: the capability
//! check, the sandbox check, the store-name parsing and the atomic commit all
//! sit between the program and the disk.
//!
//! Run twice against the same disk in two separate QEMU guests. The first run
//! finds nothing and writes; the second finds the file and verifies it byte
//! for byte. The program cannot tell which run it is except by looking at the
//! disk, which is exactly what makes the second run's success meaningful.

#![no_std]
#![no_main]

use ulib::{exit, fs_read, fs_write, write, write_u64, ERR_NOENT};

const PATH: &str = "/data/user-note";
const CONTENT: &[u8] = b"ITISYOU-OS V0.8 ring3 persistent write";

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut buf = [0u8; 128];
    let n = fs_read(PATH, &mut buf);
    if n == ERR_NOENT {
        // First boot: nothing there yet.
        if fs_write(PATH, CONTENT) != CONTENT.len() as u64 {
            write("FSUSER-WRITE-FAILED\n");
            exit(1)
        }
        write("FSUSER-WROTE bytes=");
        write_u64(CONTENT.len() as u64);
        write("\n");
        exit(0)
    }
    if n != CONTENT.len() as u64 || &buf[..CONTENT.len()] != CONTENT {
        // Something is there but it is not what was written — worse than
        // finding nothing, so it gets its own marker.
        write("FSUSER-CORRUPT bytes=");
        write_u64(n);
        write("\n");
        exit(2)
    }
    write("FSUSER-VERIFIED bytes=");
    write_u64(n);
    write("\n");
    exit(0)
}
