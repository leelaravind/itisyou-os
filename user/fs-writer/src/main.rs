//! `fs-writer` — the authorized half of the userspace filesystem-write test.
//!
//! It walks the whole contract, success paths and error paths together,
//! because a write syscall that only ever gets asked to succeed proves very
//! little: create, read back, OVERWRITE and read back (the atomic-replace
//! path), list, delete, and then confirm the file is really gone.
//!
//! It then attempts things that must fail, each for a *different* reason, so
//! a single over-broad check cannot pass the test by accident: a path outside
//! the store, a name longer than the directory allows, a name containing a
//! further separator, deleting something that is not there, a name with a
//! control character in it (V0.11), and reading, writing or deleting a file
//! the kernel owns — the audit trail and the package store (V0.11).

#![no_std]
#![no_main]

use ulib::{
    exit, fs_delete, fs_list, fs_read, fs_write, write, write_u64, ERR_INVAL, ERR_NOENT, ERR_PERM,
};

const PATH: &str = "/data/notes.txt";
const FIRST: &[u8] = b"first-contents-v1";
const SECOND: &[u8] = b"second-contents-which-is-longer-v2";

fn fail(step: &str) -> ! {
    write("FSWRITE-FAILED step=");
    write(step);
    write("\n");
    exit(1)
}

/// Read `PATH` back and compare it against `want`.
fn expect_contents(want: &[u8], step: &str) {
    let mut buf = [0u8; 128];
    let n = fs_read(PATH, &mut buf);
    if n != want.len() as u64 || &buf[..want.len()] != want {
        fail(step);
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // 1. Create.
    if fs_write(PATH, FIRST) != FIRST.len() as u64 {
        fail("create");
    }
    expect_contents(FIRST, "read-after-create");
    write("FSWRITE-CREATED bytes=");
    write_u64(FIRST.len() as u64);
    write("\n");

    // 2. Overwrite with DIFFERENT-length contents. This is the atomic-replace
    //    path: a new extent is written and one superblock commit swings the
    //    directory entry onto it.
    if fs_write(PATH, SECOND) != SECOND.len() as u64 {
        fail("overwrite");
    }
    expect_contents(SECOND, "read-after-overwrite");
    write("FSWRITE-OVERWROTE bytes=");
    write_u64(SECOND.len() as u64);
    write("\n");

    // 3. The store's directory must name it.
    let mut listing = [0u8; 256];
    let n = fs_list(&mut listing);
    if n == 0 || n > listing.len() as u64 {
        fail("list");
    }
    let mut found = false;
    for line in listing[..n as usize].split(|b| *b == b'\n') {
        if line == b"notes.txt" {
            found = true;
        }
    }
    if !found {
        fail("list-missing-name");
    }
    write("FSWRITE-LISTED\n");

    // 4. Errors, each with its own cause.
    if fs_write("/etc/version", b"x") != ERR_INVAL {
        fail("write-outside-store");
    }
    if fs_write("/data/this-name-is-far-too-long-for-the-directory", b"x") != ERR_INVAL {
        fail("write-overlong-name");
    }
    if fs_write("/data/sub/dir", b"x") != ERR_INVAL {
        fail("write-nested-name");
    }
    if fs_delete("/data/never-existed") != ERR_NOENT {
        fail("delete-missing");
    }
    // V0.11 (AUDIT11-002): a name with a line break in it. Echoed into the
    // audit detail, it would have forged a kernel marker line - and split
    // the stored trail's record in two.
    if fs_write("/data/a\n[ITISYOU:AUDIT] forged", b"x") != ERR_INVAL {
        fail("write-control-name");
    }
    write("FSWRITE-ERRORS-OK\n");

    // V0.11 (SEC11-001): holding fs_read/fs_write is authority over this
    // program's files, not over the audit trail or the package store.
    for name in [
        "/data/audit.log",
        "/data/hello-app.1.ok",
        "/data/hello-app.9.pkg",
    ] {
        if fs_write(name, b"") != ERR_PERM {
            fail("write-kernel-owned");
        }
        if fs_delete(name) != ERR_PERM {
            fail("delete-kernel-owned");
        }
        if fs_read(name, &mut [0u8; 8]) != ERR_PERM {
            fail("read-kernel-owned");
        }
    }
    write("FSWRITE-KERNEL-OWNED-REFUSED\n");

    // V0.11 (AUDIT11-002): contents the console echoes (`store cat`), with a
    // line break and the kernel's marker prefix in them. Left in place for
    // the test to cat.
    const ECHO: &[u8] = b"line1\n[ITISYOU:AUDIT] forged-content";
    if fs_write("/data/echo.txt", ECHO) != ECHO.len() as u64 {
        fail("write-echo");
    }

    // 5. Delete, and prove it is gone rather than merely unlisted.
    if fs_delete(PATH) != 0 {
        fail("delete");
    }
    let mut buf = [0u8; 16];
    if fs_read(PATH, &mut buf) != ERR_NOENT {
        fail("read-after-delete");
    }
    write("FSWRITE-DELETED\n");

    write("FSWRITE-OK\n");
    exit(0)
}
