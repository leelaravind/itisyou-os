//! `hello-app` — the first platform-managed application (V0.7). Shipped as
//! an ITPKG package, installed into the persistent store, and launched by
//! the platform with ONLY its manifest-requested capability (fs_read) under
//! the standard app sandbox. It exercises exactly that grant and nothing
//! else; the platform's tests verify both this success path and that the
//! same binary CANNOT act outside its manifest.

#![no_std]
#![no_main]

use ulib::{exit, fs_read, gui_create, write, ERR_PERM};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("HELLO-APP-START\n");
    // Granted by the manifest (caps=fs_read) + app sandbox includes /etc.
    let mut buf = [0u8; 256];
    let n = fs_read("/etc/version", &mut buf);
    if n == 0 || n > 256 {
        write("HELLO-APP-FSREAD-FAILED\n");
        exit(1);
    }
    // NOT in the manifest: gui must be denied even though the binary asks.
    if gui_create(10, 10, 0, 0, "x") != ERR_PERM {
        write("HELLO-APP-UNEXPECTED-GUI\n");
        exit(2);
    }
    write("HELLO-APP-OK\n");
    exit(0)
}
