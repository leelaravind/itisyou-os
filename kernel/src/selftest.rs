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
    graphics_tests(suite);
    device_tests(suite);
}

/// V0.6: device model — enumeration, BAR sizing, capability walking, and the
/// non-destructiveness of the BAR-probe procedure.
fn device_tests(suite: &mut Suite) {
    use crate::device;
    use kernel_core::pci::Bar;

    suite.check("device_model_enumerated", device::device_count() > 0);

    device::with_devices(|devs| {
        // Every QEMU pc topology decodes at least one known class (host bridge).
        let classified = devs.iter().any(|d| d.id().class_name() != "device");
        suite.check("device_class_decoded", classified);

        // The NVMe controller (attached via --nvme) exposes a sized MMIO BAR.
        let nvme_bar = devs
            .iter()
            .find(|d| d.id().class == 0x01 && d.id().subclass == 0x08)
            .and_then(|d| d.first_mem_bar());
        suite.check(
            "device_nvme_bar_sized",
            matches!(nvme_bar, Some((addr, size)) if addr != 0 && size >= 0x1000),
        );

        // At least one device advertises a PCI capability (NVMe: MSI-X + PM).
        suite.check(
            "device_caps_walked",
            devs.iter().any(|d| !d.caps.is_empty()),
        );

        // BAR sizing must restore config space: a probed memory BAR must still
        // read back its original base address (the probe writes all-ones then
        // restores). Proves probe_bars is non-destructive.
        let restored = devs
            .iter()
            .find_map(|d| {
                d.bars.iter().enumerate().find_map(|(i, b)| match b {
                    Bar::Memory {
                        addr, is_64: false, ..
                    } if *addr != 0 => Some((d, i, *addr)),
                    _ => None,
                })
            })
            .map(|(d, i, addr)| {
                let live = (d.pci.read_config(0x10 + (i as u8) * 4) as u64) & 0xFFFF_FFF0;
                live == (addr & 0xFFFF_FFF0)
            })
            .unwrap_or(false);
        suite.check("device_bar_probe_nondestructive", restored);
    });

    // Ring 3 device access via the devinfo syscall only (no hardware authority):
    // the userspace lsdev tool enumerates the kernel device table and exits 0.
    let lsdev_ok = matches!(
        crate::user::run_path("/bin/lsdev"),
        Ok(crate::user::UserExit::Exit(0))
    );
    suite.check("device_userspace_lsdev", lsdev_ok);
}

/// V0.5: framebuffer, compositor, GUI syscalls, input decoding.
fn graphics_tests(suite: &mut Suite) {
    use crate::gfx::{self, compositor};

    // Framebuffer is present with sane geometry.
    suite.check("gfx_available", gfx::available());
    let info = gfx::info();
    suite.check(
        "gfx_info_sane",
        info.map(|i| i.width >= 640 && i.height >= 480 && i.bytes_per_pixel >= 3)
            .unwrap_or(false),
    );

    // Direct back-buffer drawing: fill + pixel read.
    gfx::fill_rect(10, 10, 20, 20, 0x0012_3456);
    suite.check(
        "gfx_fill_pixel",
        gfx::get_pixel(15, 15) == Some(0x0012_3456),
    );
    suite.check(
        "gfx_clip_out_of_range",
        gfx::get_pixel(1_000_000, 0).is_none(),
    );

    // Glyph rendering: 'A' row 0 (0x0C) sets columns 2,3. Draw at (40,40),
    // scale 1 → those pixels are the fg colour, col 0 is not.
    gfx::fill_rect(40, 40, 8, 8, 0x0000_0000);
    gfx::draw_char(40, 40, b'A', 0x00AB_CDEF, 1);
    suite.check(
        "gfx_draw_glyph",
        gfx::get_pixel(42, 40) == Some(0x00AB_CDEF) && gfx::get_pixel(40, 40) == Some(0x0000_0000),
    );

    // Region hash is deterministic.
    let h1 = gfx::hash_region(0, 0, 64, 64);
    let h2 = gfx::hash_region(0, 0, 64, 64);
    suite.check("gfx_hash_deterministic", h1 == h2 && h1 != 0);

    // Compositor: a kernel-owned window, filled, composited to the screen.
    match compositor::create_window(0, 300, 300, 100, 60, "k") {
        Ok(id) => {
            let filled = compositor::window_fill(0, id, 0, 0, 100, 60, 0x0000_FF00).is_ok();
            suite.check("comp_window_fill", filled);
            // Out-of-bounds fill is rejected.
            suite.check(
                "comp_fill_out_of_bounds_rejected",
                compositor::window_fill(0, id, 0, 0, 200, 200, 0).is_err(),
            );
            // A different pid cannot draw into this window.
            suite.check(
                "comp_cross_owner_rejected",
                matches!(
                    compositor::window_fill(999, id, 0, 0, 1, 1, 0),
                    Err(compositor::WinError::NotOwner)
                ),
            );
            compositor::composite();
            // The window content colour is visible on screen below its title bar.
            let cx = 300 + 5;
            let cy = 300 + compositor::TITLE_BAR_H + 5;
            suite.check(
                "comp_window_composited",
                gfx::get_pixel(cx, cy) == Some(0x0000_FF00),
            );
            compositor::remove_owned(0);
        }
        Err(_) => {
            suite.check("comp_window_fill", false);
            suite.check("comp_fill_out_of_bounds_rejected", false);
            suite.check("comp_cross_owner_rejected", false);
            suite.check("comp_window_composited", false);
        }
    }
    // Bad window size rejected.
    suite.check(
        "comp_bad_size_rejected",
        matches!(
            compositor::create_window(0, 0, 0, 0, 10, "x"),
            Err(compositor::WinError::BadSize)
        ),
    );

    // In-kernel confirmation of the host-tested input decoders.
    {
        use kernel_core::mouse::Mouse;
        use kernel_core::scancode::Keyboard;
        let mut kb = Keyboard::new();
        let key_a = kb.feed(0x1E).and_then(|e| e.ascii) == Some(b'a');
        let mut ms = Mouse::new();
        ms.feed(0x08); // always-1
        ms.feed(3);
        let mouse_ok = ms.feed(2).map(|e| e.dx == 3 && e.dy == 2).unwrap_or(false);
        suite.check("input_decoders", key_a && mouse_ok);
    }

    // Ring 3 graphical application: gui-demo creates a window at (100,100),
    // fills it 0xFF8800, and presents — all via validated GUI syscalls. After
    // it exits we read the composited back-buffer pixel to prove a userspace
    // process rendered through the compositor (no direct framebuffer access).
    let gui_ran = matches!(
        crate::user::run_path("/bin/gui-demo"),
        Ok(crate::user::UserExit::Exit(0))
    );
    suite.check("gui_ring3_app_exit", gui_ran);
    // Content pixel = the app's fill colour, composited below its title bar.
    let px = gfx::get_pixel(100 + 4, 100 + compositor::TITLE_BAR_H + 40);
    suite.check("gui_ring3_app_rendered", px == Some(0x00FF_8800));
    // Its window was released on exit.
    suite.check(
        "gui_window_released_on_exit",
        compositor::window_count() == 0,
    );
}

/// V0.4: ITFS filesystem over a real block device — format/create/read/list,
/// strict metadata validation, and crash consistency (torn superblock).
fn filesystem_tests(suite: &mut Suite, dev: &dyn crate::device::block::BlockDevice) {
    use crate::fs_disk::FileSystem;
    use kernel_core::itfs::FsError;

    // Format + create two files + read them back + list.
    let ok = (|| -> Result<bool, crate::fs_disk::Error> {
        let mut fs = FileSystem::format(dev)?;
        fs.create("greeting.txt", b"hello from ITFS")?;
        fs.create("numbers.bin", &[1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10])?;
        let a = fs.read("greeting.txt")?;
        let b = fs.read("numbers.bin")?;
        let list = fs.list();
        Ok(a == b"hello from ITFS"
            && b == [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
            && fs.file_count() == 2
            && list.len() == 2)
    })()
    .unwrap_or(false);
    suite.check("fs_format_create_read_list", ok);

    // Missing file is rejected.
    let missing = {
        let fs = FileSystem::mount(dev).unwrap();
        matches!(
            fs.read("nope"),
            Err(crate::fs_disk::Error::Fs(FsError::NotFound))
        )
    };
    suite.check("fs_missing_file_rejected", missing);

    // Re-mount sees the committed state (persistence within this boot).
    let remount_ok = {
        match FileSystem::mount(dev) {
            Ok(fs) => {
                fs.file_count() == 2
                    && fs
                        .read("greeting.txt")
                        .map(|d| d == b"hello from ITFS")
                        .unwrap_or(false)
            }
            Err(_) => false,
        }
    };
    suite.check("fs_remount_reads_committed", remount_ok);

    // Crash consistency: corrupt the CURRENT superblock slot, then mount must
    // fall back to the other (older-but-consistent) slot. We commit a 2nd
    // file first so the two slots differ; corrupting the newer one drops us
    // to the state after the 1st file.
    let recovered = (|| -> Option<bool> {
        let mut fs = FileSystem::format(dev).ok()?; // gen1 in both slots
        fs.create("first", b"AAAA").ok()?; // commit -> slot 1 (gen2)
        fs.create("second", b"BBBB").ok()?; // commit -> slot 0 (gen3)
                                            // Torn write: corrupt slot 0 (the newest commit) after the fact.
        let mut garbage = [0xEEu8; crate::device::block::BLOCK_SIZE];
        garbage[0] = b'I'; // keep it clearly not a valid superblock
        dev.write_block(0, &garbage).ok()?;
        dev.flush().ok()?;
        // Mount must recover slot 1 (gen2): "first" present, "second" gone.
        let fs = FileSystem::mount(dev).ok()?;
        Some(
            fs.read("first").map(|d| d == b"AAAA").unwrap_or(false)
                && matches!(
                    fs.read("second"),
                    Err(crate::fs_disk::Error::Fs(FsError::NotFound))
                ),
        )
    })()
    .unwrap_or(false);
    suite.check("fs_crash_consistency_torn_superblock", recovered);

    // Both superblocks corrupt -> mount fails safely (no panic, no bogus fs).
    let both_bad = {
        let mut g = [0xEEu8; crate::device::block::BLOCK_SIZE];
        g[0] = b'X';
        let _ = dev.write_block(0, &g);
        let _ = dev.write_block(1, &g);
        let _ = dev.flush();
        matches!(
            FileSystem::mount(dev),
            Err(crate::fs_disk::Error::Fs(FsError::NoValidSuperblock))
        )
    };
    suite.check("fs_both_superblocks_corrupt_rejected", both_bad);
}

/// V0.4: timer-driven preemptive scheduling of Ring 3 processes.
fn preemption_tests(suite: &mut Suite) {
    use crate::proc::{self, ProcState};
    use crate::{interrupts, user};

    let free_before = memory::stats().map(|(f, _)| f).unwrap_or(0);
    let preempts_before = interrupts::preemption_count();

    // 1) Two CPU-bound processes that never yield both finish correctly under
    //    preemption — proving register + address-space preservation across
    //    involuntary switches (each verifies its own sum and stack sentinel,
    //    exiting nonzero on any corruption).
    let (a, b) = (
        user::load("/bin/spin-finite"),
        user::load("/bin/spin-finite"),
    );
    match (a, b) {
        (Ok(a), Ok(b)) => {
            let (pa, pb) = (proc::admit(a), proc::admit(b));
            let completed = proc::run_until_idle();
            let sa = proc::state_of(pa);
            let sb = proc::state_of(pb);
            suite.check(
                "preempt_two_cpu_bound_progress",
                completed >= 2
                    && matches!(sa, Some(ProcState::Exited(0)) | None)
                    && matches!(sb, Some(ProcState::Exited(0)) | None),
            );
        }
        _ => suite.check("preempt_two_cpu_bound_progress", false),
    }
    // Both non-yielding processes were actually preempted (not run to
    // completion cooperatively).
    suite.check(
        "preempt_actually_occurred",
        interrupts::preemption_count() > preempts_before,
    );

    // 2) A non-yielding INFINITE spinner cannot monopolize the CPU: a
    //    co-scheduled finite process still runs to completion, after which
    //    the spinner is still runnable (never exited). Bounded by a tick
    //    deadline so the test itself terminates; the spinner is then reaped.
    match (
        user::load("/bin/spin-forever"),
        user::load("/bin/spin-finite"),
    ) {
        (Ok(spinner), Ok(finite)) => {
            let spid = proc::admit(spinner);
            let fpid = proc::admit(finite);
            // Generous deadline (~15 s of ticks) so the finite job completes
            // even while sharing the CPU with the infinite spinner.
            let final_state = proc::run_until_pid_exits(fpid, 1500);
            let finite_done = matches!(final_state, Some(ProcState::Exited(0)) | None);
            let spinner_alive = matches!(proc::state_of(spid), Some(ProcState::Runnable));
            suite.check("preempt_no_monopoly", finite_done && spinner_alive);
            // Reap the still-running spinner (and anything else left).
            proc::drain_all();
        }
        _ => {
            suite.check("preempt_no_monopoly", false);
            proc::drain_all();
        }
    }

    // 3) Cooperative V0.3 scheduling still works under the preemptive timer
    //    (yield/wait/IPC), and repeated preemptive switching leaked no frames.
    match user::load("/bin/parent") {
        Ok(parent) => {
            let ppid = proc::admit(parent);
            let completed = proc::run_until_idle();
            suite.check(
                "preempt_coexists_with_cooperative",
                completed >= 3 && matches!(proc::state_of(ppid), Some(ProcState::Exited(0)) | None),
            );
        }
        Err(_) => suite.check("preempt_coexists_with_cooperative", false),
    }
    proc::drain_all();

    let free_after = memory::stats().map(|(f, _)| f).unwrap_or(0);
    suite.check("preempt_no_frame_leaks", free_after == free_before);
    suite.check("preempt_no_lost_processes", proc::live_count() == 0);
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

                // V0.4 NVMe write → flush → read round-trip on a scratch LBA
                // (block 100; the disk is regenerated each run so this is
                // safe and deterministic).
                let mut wbuf = [0u8; BLOCK_SIZE];
                for (i, b) in wbuf.iter_mut().enumerate() {
                    *b = (i as u8).wrapping_mul(3).wrapping_add(0x11);
                }
                let scratch: u64 = 100;
                let w = nvme.write_block(scratch, &wbuf);
                let f = nvme.flush();
                let mut rbuf = [0u8; BLOCK_SIZE];
                let r = nvme.read_block(scratch, &mut rbuf);
                suite.check(
                    "nvme_write_flush_read_roundtrip",
                    w.is_ok() && f.is_ok() && r.is_ok() && rbuf == wbuf,
                );
                // Out-of-range WRITE must be rejected before any I/O.
                suite.check(
                    "nvme_write_out_of_range_rejected",
                    matches!(
                        nvme.write_block(u64::MAX, &wbuf),
                        Err(BlockError::OutOfRange { .. })
                    ),
                );

                // V0.4 filesystem over the real NVMe device: format, create,
                // read back, list — then crash-consistency (torn superblock).
                filesystem_tests(suite, &nvme);
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

    // V0.4 preemptive scheduling.
    preemption_tests(suite);

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
