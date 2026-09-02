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
        "lsdev" => cmd_lsdev(),
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
        "commands:\n  help              this list\n  version           kernel version\n  system            platform summary\n  cpu               CPU identification\n  memory            physical + heap statistics\n  tasks             kernel task list\n  uptime            seconds since timer start\n  ls [path]         list directory\n  cat <path>        print file\n  echo <args...>    print arguments\n  run <path>        load + run a Ring 3 ELF program\n  lsdev             list detected hardware devices\n  desktop           enter the graphical desktop (PS/2 input)\n  clear             clear screen\n  panic-test confirm  trigger a kernel panic (development)\n  shutdown          exit QEMU\n  reboot            8042 CPU reset"
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
    match crate::user::run_path(path) {
        Ok(exit) => crate::serial_println!("run: {path}: {exit:?}"),
        Err(err) => crate::serial_println!("run: {path}: load failed: {err:?}"),
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
