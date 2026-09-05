//! `stack-guard-probe` — walks off the bottom of its own stack.
//!
//! The user stack is a bounded mapped region; everything below it is unmapped.
//! A program that overruns its stack should therefore take a page fault and be
//! contained, rather than silently writing over whatever happened to be mapped
//! next.
//!
//! The probe walks DOWN one page at a time, printing each page it successfully
//! wrote, until it dies. The printed count is what makes this a real
//! assertion: it proves the stack was as large as the kernel says (so the
//! probe did not simply fault immediately for an unrelated reason) and that
//! the very next page was not writable.

#![no_std]
#![no_main]

use ulib::{exit, write, write_u64};

/// Must match `USER_STACK_PAGES` in the kernel. If the kernel grows the stack
/// and this is not updated, the probe reports a different count and the test
/// fails — which is the correct outcome for a drifted assumption.
const EXPECT_PAGES: u64 = 16;
const PAGE: u64 = 4096;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // Take the address of a local as a point inside the mapped stack, then
    // walk down from the page containing it.
    let anchor = 0u64;
    let base = (&anchor as *const u64 as u64) & !(PAGE - 1);
    write("STACKGUARD-START page_base=");
    write_u64(base / PAGE);
    write("\n");

    let mut page = 0u64;
    loop {
        let addr = base - page * PAGE;
        // SAFETY: deliberately unsafe — the whole point is to find where the
        // mapping ends. Every write below the mapped region must fault, and
        // that fault is the result being tested.
        unsafe { core::ptr::write_volatile(addr as *mut u8, 0xAA) };
        write("STACKGUARD-WROTE page=");
        write_u64(page);
        write("\n");
        page += 1;
        if page > EXPECT_PAGES + 2 {
            // The guard never arrived: the region below the stack is writable,
            // so an overflow would corrupt it instead of faulting.
            write("STACKGUARD-LEAK no_fault_after=");
            write_u64(page);
            write("\n");
            exit(1)
        }
    }
}
