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
    storage_tests(suite);
}

/// V0.3: PCI enumeration, block abstraction (RAM disk), NVMe read-only.
fn storage_tests(suite: &mut Suite) {
    use crate::device::block::{BlockDevice, BlockError, RamDisk, BLOCK_SIZE};
    use crate::device::{nvme::Nvme, pci};

    // Block abstraction on a deterministic RAM disk: read + content + errors.
    let disk = RamDisk::patterned(8);
    let mut blk = [0u8; BLOCK_SIZE];
    let read_ok = disk.read_block(3, &mut blk).is_ok() && blk[0] == (3u8 ^ 0x5A);
    suite.check("blk_ramdisk_read", read_ok);
    suite.check(
        "blk_out_of_range_rejected",
        matches!(
            disk.read_block(99, &mut blk),
            Err(BlockError::OutOfRange { .. })
        ),
    );
    let mut small = [0u8; 8];
    suite.check(
        "blk_bad_buffer_rejected",
        matches!(
            disk.read_block(0, &mut small),
            Err(BlockError::BadBufferLen { .. })
        ),
    );

    // PCI enumeration must find the host bridge and the NVMe controller
    // QEMU attaches (class 0x01 subclass 0x08).
    let devices = pci::enumerate(0);
    suite.check("pci_enumerated", devices.len() >= 2);
    let nvme_dev = pci::find_nvme(&devices);
    suite.check("pci_found_nvme", nvme_dev.is_some());

    // NVMe: bring up the controller, read LBA 0, assert the harness magic.
    match nvme_dev {
        Some(dev) => match Nvme::init(&dev) {
            Ok(nvme) => {
                crate::serial_println!(
                    "[ITISYOU:INFO] nvme_ready blocks={} lba_bytes={}",
                    nvme.block_count(),
                    BLOCK_SIZE,
                );
                suite.check("nvme_init", nvme.block_count() > 0);
                let mut sector = [0u8; BLOCK_SIZE];
                let read = nvme.read_block(0, &mut sector);
                suite.check("nvme_read_block0", read.is_ok());
                suite.check("nvme_disk_magic", &sector[..16] == b"ITISYOU-OS-DISK1");
                // Out-of-range LBA must be rejected before any I/O.
                suite.check(
                    "nvme_out_of_range_rejected",
                    matches!(
                        nvme.read_block(u64::MAX, &mut sector),
                        Err(BlockError::OutOfRange { .. })
                    ),
                );
            }
            Err(e) => {
                crate::serial_println!("[ITISYOU:INFO] nvme_init_failed err={e:?}");
                suite.check("nvme_init", false);
                suite.check("nvme_read_block0", false);
                suite.check("nvme_disk_magic", false);
                suite.check("nvme_out_of_range_rejected", false);
            }
        },
        None => {
            suite.check("nvme_init", false);
            suite.check("nvme_read_block0", false);
            suite.check("nvme_disk_magic", false);
            suite.check("nvme_out_of_range_rejected", false);
        }
    }
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

    // V0.3: two loaded processes have structurally distinct address spaces —
    // the same user vaddr maps to different physical frames, and the kernel
    // (boot) table has no mapping there at all.
    let free_before_spaces = memory::stats().map(|(f, _)| f).unwrap_or(0);
    match (user::load("/bin/init"), user::load("/bin/init")) {
        (Ok(a), Ok(b)) => {
            let probe = 0x0020_0000; // both images link their code here
            let pa = a.space.translate(probe);
            let pb = b.space.translate(probe);
            suite.check(
                "aspace_distinct_frames",
                pa.is_some() && pb.is_some() && pa != pb,
            );
            suite.check(
                "aspace_kernel_table_untouched",
                crate::memory::paging::translate(x86_64::VirtAddr::new(probe)).is_none(),
            );
            user::discard(a);
            user::discard(b);
        }
        _ => {
            suite.check("aspace_distinct_frames", false);
            suite.check("aspace_kernel_table_untouched", false);
        }
    }
    // Full teardown returns every frame (leaf + intermediate tables + L4).
    let free_after_spaces = memory::stats().map(|(f, _)| f).unwrap_or(0);
    suite.check(
        "aspace_teardown_no_leaks",
        free_after_spaces == free_before_spaces,
    );

    // init: full syscall ABI round-trip in Ring 3 ending in exit(0), with
    // frame-exact leak accounting across the whole load/run/teardown cycle.
    // (Its RING3-* output lines are asserted by the harness.)
    let free_before_run = memory::stats().map(|(f, _)| f).unwrap_or(0);
    suite.check(
        "usr_init_clean_exit",
        matches!(user::run_path("/bin/init"), Ok(UserExit::Exit(0))),
    );
    let free_after_run = memory::stats().map(|(f, _)| f).unwrap_or(0);
    suite.check("usr_run_no_frame_leaks", free_after_run == free_before_run);

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

    // V0.3 cross-process memory isolation (physical proof): write a sentinel
    // into process A's frame for a user vaddr, then confirm process B's frame
    // for the SAME vaddr does not contain it (independent physical memory).
    match (user::load("/bin/init"), user::load("/bin/init")) {
        (Ok(a), Ok(b)) => {
            let probe = crate::user::USER_STACK_TOP - 0x1000; // a mapped stack page
            let (pa, pb) = (a.space.translate(probe), b.space.translate(probe));
            let isolated = match (pa, pb) {
                (Some(pa), Some(pb)) => {
                    // SAFETY: both are frames owned by the respective loaded
                    // processes, aliased through the physical-memory map.
                    unsafe {
                        let va = crate::memory::paging::phys_to_virt(pa) as *mut u64;
                        let vb = crate::memory::paging::phys_to_virt(pb) as *const u64;
                        va.write_volatile(0xA11C_E5EA_u64);
                        // B's same-vaddr frame is a different physical frame
                        // and must be unaffected (still zero).
                        pa != pb && vb.read_volatile() == 0
                    }
                }
                _ => false,
            };
            suite.check("aspace_cross_process_isolation", isolated);
            user::discard(a);
            user::discard(b);
        }
        _ => suite.check("aspace_cross_process_isolation", false),
    }

    // V0.3 concurrent processes: parent spawns two children, interleaves via
    // yield, waits for both (7,7), round-trips IPC, exits 0. All RING3-*
    // lines are harness-asserted; here we assert the terminal state.
    let free_before_conc = memory::stats().map(|(f, _)| f).unwrap_or(0);
    match user::load("/bin/parent") {
        Ok(parent) => {
            let ppid = crate::proc::admit(parent);
            let completed = crate::proc::run_until_idle();
            let ok = completed >= 3
                && matches!(
                    crate::proc::state_of(ppid),
                    Some(crate::proc::ProcState::Exited(0)) | None
                );
            suite.check("proc_concurrent_spawn_wait_ipc", ok);
        }
        Err(_) => suite.check("proc_concurrent_spawn_wait_ipc", false),
    }
    // Whole concurrent run (parent + 2 children) leaked no frames.
    let free_after_conc = memory::stats().map(|(f, _)| f).unwrap_or(0);
    suite.check(
        "proc_concurrent_no_leaks",
        free_after_conc == free_before_conc,
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
