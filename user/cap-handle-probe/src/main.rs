//! V0.8 ABI probe: proves opaque handles are issued and a forged generation
//! is denied by the kernel capability table.
#![no_std]
#![no_main]

use ulib::{cap_check, cap_list, exit, write, ERR_PERM};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut handles = [0u64; 16];
    let count = cap_list(&mut handles);
    if count == 0 || handles[0] == 0 {
        write("CAPH-LIST-FAILED\n");
        exit(1)
    }
    // No application gets to manufacture a valid generation. Flipping the
    // generation field must fail regardless of the handle's resource kind.
    let forged = handles[0] ^ (1u64 << 32);
    if cap_check(forged, 0, 0) != ERR_PERM {
        write("CAPH-FORGE-ESCAPE\n");
        exit(2)
    }
    write("CAPH-FORGED-DENIED\n");
    exit(0)
}
