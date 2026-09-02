//! Interactive kernel shell over the serial console (B120/B130).
//!
//! Input path: polled serial RX (the verified automation path per plan
//! §10.10 — the QEMU harness drives the shell through stdin). PS/2 keyboard
//! support is tracked separately as future work.
//!
//! Every command surfaces real kernel state — no fabricated output.

use crate::{fs, interrupts, memory, qemu, serial, task};
use kernel_core::shellparse;

const MAX_LINE: usize = 256;

/// Emit B120 (input path ready). Serial RX needs no extra initialization —
/// the UART was configured during early boot.
pub fn init_input() {
    crate::bootstage::emit(kernel_core::stage::Stage::B120InputReady);
}

/// Run the shell loop forever (B130 emitted by the caller beforehand).
pub fn run() -> ! {
    let _ = print_motd();
    loop {
        crate::serial_print!("itisyou> ");
        let mut buf = [0u8; MAX_LINE];
        let line = read_line(&mut buf);
        match line {
            Some(text) => execute(text),
            None => {
                crate::serial_println!("error: input line exceeded {MAX_LINE} bytes; discarded");
            }
        }
    }
}

fn print_motd() -> Result<(), fs::FsError> {
    let motd = fs::read("/etc/motd")?;
    if let Ok(text) = core::str::from_utf8(motd) {
        crate::serial_print!("{text}");
    }
    Ok(())
}

/// Read one CR/LF-terminated line with echo + backspace. Returns None on
/// overflow (input until terminator is discarded).
fn read_line(buf: &mut [u8; MAX_LINE]) -> Option<&str> {
    let mut len = 0usize;
    let mut overflow = false;
    loop {
        let Some(byte) = serial::try_read_byte() else {
            core::hint::spin_loop();
            continue;
        };
        match byte {
            b'\r' | b'\n' => {
                crate::serial_println!();
                if overflow {
                    return None;
                }
                let text = core::str::from_utf8(&buf[..len]).ok()?;
                return Some(text);
            }
            0x08 | 0x7F => {
                if len > 0 {
                    len -= 1;
                    crate::serial_print!("\x08 \x08");
                }
            }
            b' '..=b'~' => {
                if len < MAX_LINE {
                    buf[len] = byte;
                    len += 1;
                    crate::serial_print!("{}", byte as char);
                } else {
                    overflow = true;
                }
            }
            _ => {}
        }
    }
}

fn execute(line: &str) {
    let parsed = match shellparse::parse(line) {
        Ok(p) => p,
        Err(shellparse::ParseError::TooManyArgs) => {
            crate::serial_println!(
                "error: too many arguments (max {})",
                shellparse::MAX_ARGS - 1
            );
            return;
        }
    };
    let Some(command) = parsed.command() else {
        return;
    };
    let args = parsed.args();
    match command {
        "help" => cmd_help(),
        "version" => cmd_version(),
        "system" => cmd_system(),
        "cpu" => crate::cpu::report_baseline(),
        "memory" => cmd_memory(),
        "tasks" => cmd_tasks(),
        "uptime" => cmd_uptime(),
        "ls" => cmd_ls(args),
        "cat" => cmd_cat(args),
        "echo" => cmd_echo(args),
        "clear" => crate::serial_print!("\x1b[2J\x1b[H"),
        "run" => cmd_run(args),
        "svc" => cmd_svc(),
        "pkg" => cmd_pkg(args),
        "audit" => cmd_audit(),
        "lsdev" => cmd_lsdev(),
        "beep" => cmd_beep(),
        "usbwait" => cmd_usbwait(),
        "desktop" => crate::desktop::run(),
        "panic-test" => cmd_panic_test(args),
        "shutdown" => {
            crate::serial_println!("shutting down (QEMU exit)");
            qemu::exit(qemu::ExitCode::Success);
        }
        "reboot" => cmd_reboot(),
        other => {
            crate::serial_println!("unknown command: {other} (try 'help')");
        }
    }
}

fn cmd_help() {
    crate::serial_println!(
        "commands:\n  help              this list\n  version           kernel version\n  system            platform summary\n  cpu               CPU identification\n  memory            physical + heap statistics\n  tasks             kernel task list\n  uptime            seconds since timer start\n  ls [path]         list directory\n  cat <path>        print file\n  echo <args...>    print arguments\n  run <path> [caps|-] [prefix]  run a Ring 3 ELF (optionally sandboxed)\n  svc               start + supervise system services\n  pkg <op> ...      app packages: install/stage/launch/rollback/recover/list\n  audit             show the privileged-action audit trail\n  lsdev             list detected hardware devices\n  beep              play a test tone (AC97 audio)\n  usbwait           wait for USB HID input (keyboard/mouse)\n  desktop           enter the graphical desktop (PS/2 input)\n  clear             clear screen\n  panic-test confirm  trigger a kernel panic (development)\n  shutdown          exit QEMU\n  reboot            8042 CPU reset"
    );
}

fn cmd_version() {
    crate::serial_println!(
        "itisyou-os {} (x86_64, QEMU, pre-alpha)",
        env!("CARGO_PKG_VERSION")
    );
}

fn cmd_system() {
    crate::serial_println!(
        "kernel=itisyou-os version={} arch=x86_64 boot=bootloader-crate env=QEMU tick_hz={}",
        env!("CARGO_PKG_VERSION"),
        interrupts::TICK_HZ,
    );
}

fn cmd_memory() {
    match memory::stats() {
        Some((free, stats)) => {
            crate::serial_println!(
                "physical: usable_frames={} free_frames={} allocated={} ignored_high={}",
                stats.total_usable_frames,
                free,
                stats.allocated_frames,
                stats.ignored_high_frames,
            );
        }
        None => crate::serial_println!("physical: not initialized"),
    }
    let (used, free) = memory::heap::stats();
    crate::serial_println!(
        "heap: used={used} free={free} size={}",
        memory::heap::HEAP_SIZE
    );
}

fn cmd_tasks() {
    for (id, name, state) in task::task_list() {
        crate::serial_println!("task {id}: {name} {state:?}");
    }
}

fn cmd_uptime() {
    crate::serial_println!(
        "uptime: {}s (ticks={})",
        interrupts::uptime_seconds(),
        interrupts::ticks()
    );
}

fn cmd_ls(args: &[&str]) {
    let path = args.first().copied().unwrap_or("/");
    match fs::list(path) {
        Ok(entries) => {
            for e in entries {
                if e.is_dir {
                    crate::serial_println!("{}/", e.name);
                } else {
                    crate::serial_println!("{} ({} bytes)", e.name, e.size);
                }
            }
        }
        Err(err) => crate::serial_println!("ls: {path}: {err:?}"),
    }
}

fn cmd_cat(args: &[&str]) {
    let Some(path) = args.first() else {
        crate::serial_println!("cat: missing path argument");
        return;
    };
    match fs::read(path) {
        Ok(data) => match core::str::from_utf8(data) {
            Ok(text) => crate::serial_print!("{text}"),
            Err(_) => crate::serial_println!("cat: {path}: binary file ({} bytes)", data.len()),
        },
        Err(err) => crate::serial_println!("cat: {path}: {err:?}"),
    }
}

fn cmd_echo(args: &[&str]) {
    let mut first = true;
    for a in args {
        if !first {
            crate::serial_print!(" ");
        }
        crate::serial_print!("{a}");
        first = false;
    }
    crate::serial_println!();
}

fn cmd_run(args: &[&str]) {
    let Some(path) = args.first() else {
        crate::serial_println!("run: missing program path (e.g. run /bin/init)");
        return;
    };
    // Optional V0.7 sandboxing: `run <path> [caps|-] [prefix]` — "-" means
    // NO capabilities; a prefix restricts fs_read to that subtree. Without
    // the extra args the trusted legacy-full launch is unchanged.
    let caps = match args.get(1) {
        None => kernel_core::caps::CAP_LEGACY_FULL,
        Some(&"-") => 0,
        Some(list) => match kernel_core::caps::parse(list) {
            Ok(c) => c,
            Err(_) => {
                crate::serial_println!("run: unknown capability in \"{list}\"");
                return;
            }
        },
    };
    let sandbox = args
        .get(2)
        .map(|p| alloc::sync::Arc::new(alloc::vec![alloc::string::String::from(*p)]));
    match crate::user::run_path_with(path, caps, sandbox) {
        Ok(exit) => crate::serial_println!("run: {path}: {exit:?}"),
        Err(err) => crate::serial_println!("run: {path}: load failed: {err:?}"),
    }
}

fn cmd_svc() {
    match crate::services::run_supervised(600) {
        Ok(report) => {
            crate::services::with_status(|table| {
                for s in table {
                    crate::serial_println!(
                        "  {}  {:?}  pid={} restarts={}",
                        s.name,
                        s.state,
                        s.pid,
                        s.restarts
                    );
                }
            });
            crate::serial_println!(
                "svc: started={} done={} failed={} restarts={}",
                report.started,
                report.done,
                report.failed,
                report.restarts_performed
            );
        }
        Err(e) => crate::serial_println!("svc: startup ordering failed: {e:?}"),
    }
}

fn cmd_audit() {
    let (total, denials) = crate::audit::counts();
    crate::audit::with_records(|records| {
        for r in records {
            crate::serial_println!(
                "  #{} tick={} pid={} {} cap={:#x} {} {}",
                r.seq,
                r.tick,
                r.pid,
                r.action,
                r.cap,
                if r.ok { "ok" } else { "DENIED" },
                r.detail.as_deref().unwrap_or(""),
            );
        }
    });
    crate::serial_println!("audit: total={total} denials={denials} (ring keeps the newest 64)");
}

fn cmd_pkg(args: &[&str]) {
    use crate::fs_disk::FileSystem;
    let Some(&sub) = args.first() else {
        crate::serial_println!(
            "pkg: subcommands: install <vfs.pkg> | stage <vfs.pkg> | launch <app> | rollback <app> | recover | list"
        );
        return;
    };
    let Some(nvme) = crate::open_nvme() else {
        crate::serial_println!("pkg: no NVMe storage attached");
        return;
    };
    // Mount the persistent store; a fresh disk is formatted on first use.
    let mut fs = match FileSystem::mount(&nvme) {
        Ok(fs) => fs,
        Err(_) => match FileSystem::format(&nvme) {
            Ok(fs) => fs,
            Err(e) => {
                crate::serial_println!("pkg: storage unusable: {e:?}");
                return;
            }
        },
    };
    match (sub, args.get(1)) {
        ("install", Some(src)) | ("stage", Some(src)) => {
            let bytes = match crate::fs::read(src) {
                Ok(b) => b,
                Err(e) => {
                    crate::serial_println!("pkg: {src}: {e:?}");
                    return;
                }
            };
            let result = if sub == "install" {
                crate::platform::install(&mut fs, bytes)
            } else {
                crate::platform::stage(&mut fs, bytes)
            };
            match result {
                Ok((app, v)) => crate::serial_println!("pkg: {sub} {app} -> store v{v}"),
                Err(e) => crate::serial_println!("pkg: {sub} refused: {e:?}"),
            }
        }
        ("launch", Some(app)) => {
            match crate::platform::launch(&fs, app, kernel_core::caps::CAP_LEGACY_FULL) {
                Ok(code) => crate::serial_println!("pkg: launch {app}: exit={code}"),
                Err(e) => crate::serial_println!("pkg: launch {app}: {e:?}"),
            }
        }
        ("rollback", Some(app)) => match crate::platform::rollback(&mut fs, app) {
            Ok((from, to)) => crate::serial_println!("pkg: rollback {app}: v{from} -> v{to}"),
            Err(e) => crate::serial_println!("pkg: rollback {app}: {e:?}"),
        },
        ("recover", _) => match crate::platform::recover(&mut fs) {
            Ok(n) => crate::serial_println!("pkg: recovery findings={n}"),
            Err(e) => crate::serial_println!("pkg: recover: {e:?}"),
        },
        ("list", _) => {
            for app in crate::platform::apps(&fs) {
                let st = crate::platform::state(&fs, &app);
                crate::serial_println!(
                    "  {app}: active={:?} previous={:?} staged={:?}",
                    st.active,
                    st.previous,
                    st.orphan_staged
                );
            }
        }
        _ => crate::serial_println!("pkg: bad arguments (try `pkg`)"),
    }
}

fn cmd_usbwait() {
    use crate::device::uhci;
    if !uhci::available() {
        crate::serial_println!("usbwait: no USB controller");
        return;
    }
    if !uhci::has_hid() {
        crate::serial_println!("usbwait: no USB HID device");
        return;
    }
    let kind = uhci::device_kind().unwrap_or("usb");
    crate::serial_println!("[ITISYOU:INFO] USB-HID-READY kind={kind}");

    // Poll the interrupt-IN endpoint at the timer rate until a HID report
    // arrives or a bounded deadline (the harness injects input via monitor).
    let deadline = interrupts::ticks() + interrupts::TICK_HZ * 10;
    let mut report = [0u8; 8];
    let mut got = false;
    while interrupts::ticks() < deadline {
        let n = uhci::poll_hid_report(&mut report);
        if n >= 3 {
            if kind == "mouse" {
                if let Some(m) = kernel_core::usb::hid_mouse(&report[..n]) {
                    if m.dx != 0 || m.dy != 0 || m.left || m.right {
                        crate::serial_println!(
                            "[ITISYOU:INPUT] usb mouse dx={} dy={} l={} r={}",
                            m.dx,
                            m.dy,
                            m.left as u8,
                            m.right as u8
                        );
                        got = true;
                        break;
                    }
                }
            } else if let Some(a) = kernel_core::usb::hid_keyboard_ascii(&report[..n]) {
                if a.is_ascii_graphic() || a == b' ' {
                    crate::serial_println!("[ITISYOU:INPUT] usb key={}", a as char);
                } else {
                    crate::serial_println!("[ITISYOU:INPUT] usb key=<special>");
                }
                got = true;
                break;
            }
        }
        x86_64::instructions::hlt();
    }
    if got {
        crate::serial_println!("[ITISYOU:INFO] USB-HID-VERIFIED kind={kind}");
    } else {
        crate::serial_println!("[ITISYOU:INFO] USB-HID-NO-INPUT kind={kind}");
    }
}

fn cmd_beep() {
    if !crate::device::ac97::available() {
        crate::serial_println!("beep: no AC97 audio device");
        return;
    }
    match crate::device::ac97::play_test_tone() {
        Some(r) => crate::serial_println!(
            "beep: played {} buffers (civ={} halted={})",
            r.buffers,
            r.consumed,
            r.halted
        ),
        None => crate::serial_println!("beep: playback failed"),
    }
}

fn cmd_lsdev() {
    use kernel_core::pci::Bar;
    crate::device::with_devices(|devs| {
        crate::serial_println!("{} device(s):", devs.len());
        for d in devs {
            crate::serial_println!(
                "  {:02x}:{:02x}.{}  {:04x}:{:04x}  class {:#04x}/{:#04x}  {}",
                d.pci.bus,
                d.pci.slot,
                d.pci.func,
                d.id().vendor,
                d.id().device,
                d.id().class,
                d.id().subclass,
                d.id().class_name(),
            );
            for (i, b) in d.bars.iter().enumerate() {
                match b {
                    Bar::Io { port, size } if *size > 0 => {
                        crate::serial_println!("      bar{i}: io   port={port:#06x} size={size:#x}")
                    }
                    Bar::Memory {
                        addr, size, is_64, ..
                    } if *size > 0 => crate::serial_println!(
                        "      bar{i}: mem  addr={addr:#x} size={size:#x} {}",
                        if *is_64 { "64-bit" } else { "32-bit" }
                    ),
                    _ => {}
                }
            }
            if !d.caps.is_empty() {
                let mut caps = alloc::string::String::new();
                for (i, (c, off)) in d.caps.iter().enumerate() {
                    if i > 0 {
                        caps.push(' ');
                    }
                    let _ = core::fmt::Write::write_fmt(
                        &mut caps,
                        format_args!("{}@{:#x}", c.name(), off),
                    );
                }
                crate::serial_println!("      caps: {caps}");
            }
            if let Some(name) = d.driver {
                crate::serial_println!("      driver: {name}");
            }
        }
    });
}

fn cmd_panic_test(args: &[&str]) {
    if args.first() == Some(&"confirm") {
        panic!("panic-test invoked from shell");
    }
    crate::serial_println!("panic-test: pass 'confirm' to trigger a real kernel panic");
}

fn cmd_reboot() {
    crate::serial_println!("rebooting via 8042 reset pulse");
    // SAFETY: writing 0xFE to the keyboard controller command port pulses
    // the CPU reset line — the architectural soft-reset path on PC hardware.
    unsafe {
        let mut port = x86_64::instructions::port::Port::<u8>::new(0x64);
        port.write(0xFEu8);
    }
    qemu::halt_loop();
}
