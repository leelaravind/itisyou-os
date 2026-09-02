//! In-kernel selftest suite executed by the `itisyou-kernel-selftest` binary
//! inside QEMU. Each check emits a `[ITISYOU:TEST]` event; the harness
//! requires `fail=0` plus a clean QEMU exit.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::VirtAddr;

use crate::{fs, interrupts, memory, task};

pub struct Suite {
    pub pass: u32,
    pub fail: u32,
}

impl Suite {
    pub fn new() -> Self {
        Suite { pass: 0, fail: 0 }
    }

    pub fn check(&mut self, name: &str, ok: bool) {
        if ok {
            self.pass += 1;
            crate::serial_println!("[ITISYOU:TEST] name={name} result=pass");
        } else {
            self.fail += 1;
            crate::serial_println!("[ITISYOU:TEST] name={name} result=fail");
        }
    }
}

impl Default for Suite {
    fn default() -> Self {
        Self::new()
    }
}

/// Run every in-kernel selftest. Requires all subsystems initialized.
pub fn run_all(suite: &mut Suite) {
    pmm_tests(suite);
    paging_tests(suite);
    heap_tests(suite);
    exception_tests(suite);
    timer_tests(suite);
    scheduler_tests(suite);
    vfs_tests(suite);
    userspace_tests(suite);
}

/// V0.2: Ring 3 execution, syscalls, isolation, and rejection paths.
fn userspace_tests(suite: &mut Suite) {
    use crate::user::{self, LoadError, UserExit};
    use kernel_core::elf::ElfError;

    // Malformed ELF (corrupted magic) is rejected structurally.
    suite.check(
        "usr_elf_reject_malformed",
        matches!(
            user::load("/bin/broken"),
            Err(LoadError::Elf(ElfError::BadMagic))
        ),
    );

    // Writable+executable segment is rejected by W^X policy.
    suite.check(
        "usr_elf_reject_wx",
        matches!(user::load("/bin/wx-test"), Err(LoadError::WxSegment { .. })),
    );

    // init: full syscall ABI round-trip in Ring 3 ending in exit(0).
    // (Its RING3-* output lines are asserted by the harness.)
    suite.check(
        "usr_init_clean_exit",
        matches!(user::run_path("/bin/init"), Ok(UserExit::Exit(0))),
    );

    // Privileged instruction at CPL=3 → #GP contained, process terminated.
    suite.check(
        "usr_gp_contained",
        matches!(
            user::run_path("/bin/gp-test"),
            Ok(UserExit::Fault { vector: 13, .. })
        ),
    );

    // Kernel-half read at CPL=3 → #PF contained, process terminated.
    suite.check(
        "usr_pf_contained",
        matches!(
            user::run_path("/bin/pf-test"),
            Ok(UserExit::Fault {
                vector: 14,
                addr: Some(0xFFFF_8000_DEAD_0000)
            })
        ),
    );

    // The kernel survived two user crashes: allocator + timer still work.
    let alive_alloc = alloc::vec![0xEEu8; 4096];
    let t0 = crate::interrupts::ticks();
    let mut spins = 0u64;
    while crate::interrupts::ticks() < t0 + 2 && spins < 2_000_000_000 {
        x86_64::instructions::hlt();
        spins += 1;
    }
    suite.check(
        "usr_kernel_alive_after_faults",
        alive_alloc[0] == 0xEE && crate::interrupts::ticks() >= t0 + 2,
    );

    // A second process gets a fresh load at the same addresses (teardown
    // really unmapped the previous instance).
    suite.check(
        "usr_reload_after_teardown",
        matches!(user::run_path("/bin/init"), Ok(UserExit::Exit(0))),
    );
}

fn pmm_tests(suite: &mut Suite) {
    let a = memory::alloc_frame();
    let b = memory::alloc_frame();
    let distinct = match (&a, &b) {
        (Ok(fa), Ok(fb)) => fa.start_address() != fb.start_address(),
        _ => false,
    };
    suite.check("pmm_alloc_two_distinct", distinct);

    let freed = match (a, b) {
        (Ok(fa), Ok(fb)) => memory::free_frame(fa).is_ok() && memory::free_frame(fb).is_ok(),
        _ => false,
    };
    suite.check("pmm_free_ok", freed);

    let double_free_detected = match memory::alloc_frame() {
        Ok(frame) => {
            let first = memory::free_frame(frame);
            let second = memory::free_frame(frame);
            first.is_ok() && second.is_err()
        }
        Err(_) => false,
    };
    suite.check("pmm_double_free_detected", double_free_detected);
}

fn paging_tests(suite: &mut Suite) {
    const TEST_VADDR: u64 = 0x_5555_0000_0000;
    let page: Page = Page::containing_address(VirtAddr::new(TEST_VADDR));

    let outcome = (|| -> Result<(bool, bool, bool), ()> {
        let frame = memory::alloc_frame().map_err(|_| ())?;
        memory::paging::map_page(
            page,
            frame,
            PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
        )
        .map_err(|_| ())?;
        let translated =
            memory::paging::translate(VirtAddr::new(TEST_VADDR)) == Some(frame.start_address());
        // Write/read through the fresh mapping.
        let ptr = TEST_VADDR as *mut u64;
        // SAFETY: the page was just mapped writable and is unused elsewhere.
        let readback = unsafe {
            ptr.write_volatile(0xDEAD_BEEF_CAFE_F00D);
            ptr.read_volatile() == 0xDEAD_BEEF_CAFE_F00D
        };
        let unmapped_frame = memory::paging::unmap_page(page).map_err(|_| ())?;
        let translate_gone = memory::paging::translate(VirtAddr::new(TEST_VADDR)).is_none();
        memory::free_frame(unmapped_frame).map_err(|_| ())?;
        Ok((translated, readback, translate_gone))
    })();
    let (translated, readback, translate_gone) = outcome.unwrap_or((false, false, false));
    suite.check("paging_map_translate", translated);
    suite.check("paging_write_read", readback);
    suite.check("paging_unmap_clears", translate_gone);

    // Policy: writable without NO_EXECUTE must be rejected.
    let wx_rejected = match memory::alloc_frame() {
        Ok(frame) => {
            let result = memory::paging::map_page(page, frame, PageTableFlags::WRITABLE);
            let rejected = matches!(result, Err(memory::paging::PagingError::WxViolation));
            let _ = memory::free_frame(frame);
            rejected
        }
        Err(_) => false,
    };
    suite.check("paging_wx_rejected", wx_rejected);
}

fn heap_tests(suite: &mut Suite) {
    let boxed = Box::new(42u64);
    suite.check("heap_box", *boxed == 42);

    let mut v: Vec<u64> = Vec::new();
    for i in 0..10_000u64 {
        v.push(i);
    }
    let sum: u64 = v.iter().sum();
    suite.check("heap_vec_growth", sum == 10_000 * 9_999 / 2);

    let big = alloc::vec![0xA5u8; 128 * 1024];
    suite.check(
        "heap_large_alloc",
        big.len() == 128 * 1024 && big[0] == 0xA5 && big[big.len() - 1] == 0xA5,
    );
    drop(big);

    let aligned: Box<[u128; 8]> = Box::new([7; 8]);
    let addr = aligned.as_ptr() as usize;
    suite.check(
        "heap_alignment_u128",
        addr.is_multiple_of(core::mem::align_of::<u128>()),
    );

    let (used, free) = memory::heap::stats();
    suite.check("heap_stats_sane", used > 0 && free > 0);
}

fn exception_tests(suite: &mut Suite) {
    let before = interrupts::breakpoint_count();
    x86_64::instructions::interrupts::int3();
    let after = interrupts::breakpoint_count();
    suite.check("idt_breakpoint_resumes", after == before + 1);
}

fn timer_tests(suite: &mut Suite) {
    let start = interrupts::ticks();
    let mut spins: u64 = 0;
    // ~50 ms at 100 Hz. Bounded so a dead timer fails instead of hanging.
    const SPIN_BOUND: u64 = 2_000_000_000;
    while interrupts::ticks() < start + 5 && spins < SPIN_BOUND {
        x86_64::instructions::hlt();
        spins += 1;
    }
    let end = interrupts::ticks();
    suite.check("timer_ticks_advance", end >= start + 5);
    suite.check("timer_monotonic", end >= start);
}

static SEQUENCE: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static TASKS_DONE: AtomicUsize = AtomicUsize::new(0);

fn worker_a() {
    for _ in 0..3 {
        SEQUENCE.lock().push(b'a');
        task::yield_now();
    }
    TASKS_DONE.fetch_add(1, Ordering::SeqCst);
}

fn worker_b() {
    for _ in 0..3 {
        SEQUENCE.lock().push(b'b');
        task::yield_now();
    }
    TASKS_DONE.fetch_add(1, Ordering::SeqCst);
}

fn scheduler_tests(suite: &mut Suite) {
    SEQUENCE.lock().clear();
    TASKS_DONE.store(0, Ordering::SeqCst);

    let spawned =
        task::spawn("worker-a", worker_a).is_ok() && task::spawn("worker-b", worker_b).is_ok();
    suite.check("sched_spawn", spawned);

    // Drive the scheduler from the boot task until both workers finish.
    let mut rounds = 0;
    while !task::all_spawned_finished() && rounds < 1000 {
        task::yield_now();
        rounds += 1;
    }
    suite.check("sched_all_finished", task::all_spawned_finished());
    suite.check(
        "sched_both_completed",
        TASKS_DONE.load(Ordering::SeqCst) == 2,
    );

    // Cooperative round-robin gives a deterministic interleave.
    let seq = SEQUENCE.lock();
    suite.check(
        "sched_interleave_deterministic",
        seq.as_slice() == b"ababab",
    );
}

fn vfs_tests(suite: &mut Suite) {
    let root = fs::list("/");
    let has_dirs = match &root {
        Ok(entries) => {
            entries.iter().any(|e| e.name == "etc" && e.is_dir)
                && entries.iter().any(|e| e.name == "docs" && e.is_dir)
        }
        Err(_) => false,
    };
    suite.check("vfs_ls_root", has_dirs);

    let version_ok = match fs::read("/etc/version") {
        Ok(data) => data == alloc::format!("{}\n", env!("CARGO_PKG_VERSION")).as_bytes(),
        Err(_) => false,
    };
    suite.check("vfs_cat_version", version_ok);

    suite.check(
        "vfs_missing_path",
        fs::read("/does/not/exist") == Err(fs::FsError::NotFound),
    );
    suite.check(
        "vfs_traversal_rejected",
        matches!(
            fs::read("/../secret"),
            Err(fs::FsError::InvalidPath(
                kernel_core::path::PathError::EscapesRoot
            ))
        ),
    );
    suite.check(
        "vfs_relative_normalized",
        fs::read("/etc/../etc/version").is_ok(),
    );
    suite.check(
        "vfs_dir_not_file",
        fs::read("/etc") == Err(fs::FsError::NotAFile),
    );
}
