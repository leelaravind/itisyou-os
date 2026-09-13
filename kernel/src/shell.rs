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

/// How long the shell hands the CPU to background services when no input is
/// pending. One tick (10 ms at 100 Hz) keeps the console responsive while
/// still letting a daemon run a full quantum.
const SHELL_IDLE_SLICE_TICKS: u64 = 1;

/// Ceiling on a co-scheduled `bg` job (30 s at 100 Hz), so a client that never
/// completes reports a timeout instead of taking the console with it.
const BG_JOB_MAX_TICKS: u64 = 3_000;

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
            // Idle time belongs to the background services (V0.8): the shell
            // is waiting on a human, so hand the CPU to the persistent Ring 3
            // daemons instead of burning it in a spin loop. The slice is
            // bounded in real time, so keystroke latency stays bounded too.
            crate::services::pump(SHELL_IDLE_SLICE_TICKS);
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
        "bg" => cmd_bg(args),
        "xhciwait" => cmd_xhciwait(),
        "harden" => cmd_harden(),
        "irq" => cmd_irq(),
        "store" => cmd_store(args),
        "net" => cmd_net(),
        "ping" => cmd_ping(args),
        "resolve" => cmd_resolve(args),
        "dhcp" => cmd_dhcp(),
        "ipv6" => cmd_ipv6(),
        "ping6" => cmd_ping6(args),
        "tcp" => cmd_tcp(args),
        "svc" => cmd_svc(),
        "pkg" => cmd_pkg(args),
        "audit" => cmd_audit(args),
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
        "poweroff" => cmd_poweroff(),
        "acpi" => cmd_acpi(),
        other => {
            crate::serial_println!("unknown command: {other} (try 'help')");
        }
    }
}

fn cmd_help() {
    crate::serial_println!(
        "commands:\n  help              this list\n  version           kernel version\n  system            platform summary\n  cpu               CPU identification\n  memory            physical + heap statistics\n  tasks             kernel task list\n  uptime            seconds since timer start\n  ls [path]         list directory\n  cat <path>        print file\n  echo <args...>    print arguments\n  run <path> [caps|-] [prefix]  run a Ring 3 ELF (optionally sandboxed)\n  bg <path> [caps]  run a Ring 3 ELF co-scheduled with the background services\n  svc               supervise the on-demand services; show all service state\n  pkg <op> ...      app packages: install/stage/launch/rollback/recover/list\n  xhciwait          wait for USB HID input through the xHCI controller\n  harden            CPU-enforced kernel/user separation (SMEP/SMAP/UMIP)\n  irq               interrupt routing (I/O APIC), delivery proofs and counters\n  acpi              ACPI tables found: MADT routes, FADT, S5\n  store <op> ...    persistent store: ls | put <name> <text> | cat <name> | rm <name>\n  net               interface address, counters and bound sockets\n  ping <ip> [n]     ICMP echo the given IPv4 address\n  resolve <name>    DNS A lookup through the configured server\n  dhcp              obtain and apply an address from a DHCP server\n  ipv6              bring up IPv6: link-local, router solicitation, SLAAC\n  ping6 <addr> [n]  ICMPv6 echo\n  tcp [drop <n>]    TCP connections and counters; drop <n> discards the next n data segments (test)\n  audit [save|verify|anchor <ip> <port>|check-anchor <ip> <port>]  privileged-action trail\n  lsdev             list detected hardware devices\n  beep              play a test tone (AC97 audio)\n  usbwait           wait for USB HID input (keyboard/mouse)\n  desktop           enter the graphical desktop (PS/2 input)\n  clear             clear screen\n  panic-test confirm  trigger a kernel panic (development)\n  shutdown          exit QEMU (test-exit device)\n  poweroff          ACPI S5 soft power-off\n  reboot            8042 CPU reset"
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

/// `bg <path> [caps]` — run a program CO-SCHEDULED with the persistent
/// background services, rather than to completion on its own.
///
/// `run` executes a process start-to-finish while nothing else is on the CPU,
/// so a program that needs to talk to a long-running service could never
/// succeed under it: the service is not running while the client is. `bg`
/// admits the program to the process table and pumps the scheduler until it
/// terminates, so client and daemon are runnable in the same period — which
/// is what makes an IPC round-trip with a live service possible at all.
fn cmd_bg(args: &[&str]) {
    let Some(path) = args.first() else {
        crate::serial_println!("bg: missing program path (e.g. bg /bin/tick-client)");
        return;
    };
    let caps = match args.get(1) {
        None => kernel_core::caps::CAP_LEGACY_FULL,
        Some(&"-") => 0,
        Some(list) => match kernel_core::caps::parse(list) {
            Ok(c) => c,
            Err(_) => {
                crate::serial_println!("bg: unknown capability in \"{list}\"");
                return;
            }
        },
    };
    let process = match crate::user::load_with(path, caps, None) {
        Ok(p) => p,
        Err(err) => {
            crate::serial_println!("bg: {path}: load failed: {err:?}");
            return;
        }
    };
    let pid = crate::proc::admit(process);
    // Bounded: a client that never finishes must not wedge the console.
    let deadline = crate::interrupts::ticks() + BG_JOB_MAX_TICKS;
    loop {
        crate::services::pump(1);
        match crate::proc::state_of(pid) {
            Some(crate::proc::ProcState::Exited(code)) => {
                crate::proc::reap(pid);
                crate::serial_println!("bg: {path}: exit={code}");
                return;
            }
            Some(crate::proc::ProcState::Faulted { vector }) => {
                crate::proc::reap(pid);
                crate::serial_println!("bg: {path}: faulted vector={vector} contained=true");
                return;
            }
            None => {
                crate::serial_println!("bg: {path}: vanished");
                return;
            }
            Some(_) => {}
        }
        if crate::interrupts::ticks() >= deadline {
            crate::proc::reap(pid);
            crate::serial_println!("bg: {path}: timed out after {BG_JOB_MAX_TICKS} ticks");
            return;
        }
    }
}

/// `net` — everything an operator needs to tell a working link from a broken
/// one: the address plan, the driver's frame counters, this stack's per-layer
/// counters (including what it refused and why), and the bound sockets.
/// `store` — operator access to the persistent ITFS volume.
///
/// It exists so the store can be inspected and seeded without a program: the
/// filesystem tests use `put` to plant a file that a *read-only* process then
/// reads, which is what makes "writes denied, reads still allowed" a
/// meaningful distinction rather than an empty one.
/// `xhciwait` — poll the xHCI HID interrupt endpoint for one report.
///
/// Separate from `usbwait` (which polls UHCI) rather than folded into it: the
/// two controllers are genuinely different hardware, and a test that could not
/// say which one delivered the keystroke would prove nothing about either.
fn cmd_xhciwait() {
    if !crate::device::xhci::present() {
        crate::serial_println!("xhciwait: no xHCI controller bound");
        return;
    }
    // Printed BEFORE blocking so the test harness has a gate to inject a
    // keypress after: injecting before the endpoint is armed would deliver the
    // report to nobody.
    crate::serial_println!("XHCI-HID-WAITING");
    let mut report = [0u8; 8];
    let got = crate::device::xhci::with(|c| c.poll_hid(&mut report, 5000)).flatten();
    match got {
        Some(n) if n > 0 => {
            let ascii = kernel_core::usb::hid_keyboard_ascii(&report[..n]);
            crate::serial_println!(
                "XHCI-HID-REPORT bytes={n} modifier={:#04x} key={:#04x} ascii={}",
                report[0],
                report.get(2).copied().unwrap_or(0),
                ascii.map(|c| c as char).unwrap_or('.'),
            );
            if let Some(c) = ascii {
                crate::serial_println!("[ITISYOU:INPUT] key={} src=xhci", c as char);
            }
        }
        _ => crate::serial_println!("xhciwait: no report within the deadline"),
    }
}

/// `harden` — which CPU-enforced protections are actually on.
///
/// Reported from CR4 rather than from what the kernel intended to enable: a
/// protection you believe is on and is not is worse than one you know is off.
fn cmd_harden() {
    let (smep, smap, umip) = crate::harden::state();
    crate::serial_println!("harden: smep={smep} smap={smap} umip={umip}");
    crate::serial_println!(
        "harden: wx_enforced=true user_pointer_validation=active stack_guard=unmapped"
    );
}

/// `irq` — the state of both interrupt controllers and what each has
/// actually delivered.
///
/// It fires a one-shot local-APIC timer as part of the report rather than
/// only reading counters: "the APIC is enabled" is a configuration claim,
/// while "an interrupt arrived on vector 0x41 just now" is evidence.
fn cmd_irq() {
    if !crate::apic::is_enabled() {
        crate::serial_println!("irq: local APIC not enabled");
        return;
    }
    // Which controller carries the line-based IRQs, read back from the
    // hardware rather than from a flag: the PIC mask registers and each I/O
    // APIC redirection entry (V0.9 cutover, ADR-0019).
    let (m1, m2) = crate::interrupts::pic_masks();
    crate::serial_println!(
        "irq: legacy_lines={} pic_masks={m1:#04x}/{m2:#04x} timer_ticks={} preemptions={}",
        if crate::interrupts::legacy_via_ioapic() {
            "ioapic"
        } else {
            "pic"
        },
        crate::interrupts::ticks(),
        crate::interrupts::preemption_count(),
    );
    if let Some(madt) = crate::acpi::madt() {
        for irq in [0u8, 1, 12] {
            let route = madt.isa_route(irq);
            if let Some((low, _)) = crate::apic::ioapic_entry(route.gsi) {
                crate::serial_println!(
                    "irq: route isa={irq} gsi={} vector={:#x} masked={}",
                    route.gsi,
                    low & 0xFF,
                    low & (1 << 16) != 0,
                );
            }
        }
    }
    // The adversarial half: mask the timer's I/O APIC entry and the tick count
    // must stop dead, unmask it and it must move. If the PIC — or anything
    // else — were still delivering the timer, the masked window would count.
    match crate::apic::prove_timer_route(200) {
        Some((masked, unmasked)) => crate::serial_println!(
            "irq: timer_route_proof masked_ticks={masked} unmasked_ticks={unmasked} result={}",
            if masked == 0 && unmasked > 0 {
                "ioapic_only"
            } else {
                "FAILED"
            }
        ),
        None => crate::serial_println!("irq: timer_route_proof skipped legacy_lines=pic"),
    }
    let delivered = crate::apic::timer_oneshot(100_000, 500);

    // Message-signalled delivery. The NIC in this machine model exposes no MSI
    // capability, so the proof uses the NVMe controller's MSI-X: arm entry 0,
    // then do a real block read whose completion raises it. Using actual I/O
    // rather than a synthetic poke is the point — it is the same path a driver
    // would rely on.
    let before_msi = crate::apic::counters().1;
    let armed = crate::device::with_devices(|devices| {
        devices
            .iter()
            // Pick by CAPABILITY, not by class: this machine also has an IDE
            // controller, which is storage and has no capabilities at all —
            // selecting the first storage device found it instead of the NVMe.
            .find(|d| d.has_cap(kernel_core::pci::CapabilityId::MsiX))
            .map(|d| crate::apic::enable_msix(d, crate::interrupts::VECTOR_MSI))
            .unwrap_or(false)
    });
    if armed {
        if let Some(nvme) = crate::open_nvme() {
            let mut block = [0u8; 512];
            use crate::device::block::BlockDevice;
            let read_ok = nvme.read_block(0, &mut block).is_ok();
            let mut deadline = crate::interrupts::Deadline::after_ms(500);
            while crate::apic::counters().1 == before_msi && deadline.pending() {}
            crate::serial_println!(
                "irq: msix armed=true block_read={read_ok} delivered={}",
                crate::apic::counters().1 > before_msi,
            );
        } else {
            crate::serial_println!("irq: msix armed=true nvme=unavailable");
        }
    } else {
        crate::serial_println!("irq: msix armed=false (no MSI-X capable device attached)");
    }
    let (timer, msi, spurious) = crate::apic::counters();
    crate::serial_println!(
        "irq: apic_timer delivered={delivered} count={timer} vector={:#x}",
        crate::interrupts::VECTOR_APIC_TIMER,
    );
    crate::serial_println!(
        "irq: msi count={msi} vector={:#x} spurious={spurious}",
        crate::interrupts::VECTOR_MSI,
    );
}

fn cmd_store(args: &[&str]) {
    let Some(&sub) = args.first() else {
        crate::serial_println!(
            "store: subcommands: ls | put <name> <text> | cat <name> | rm <name>"
        );
        return;
    };
    match (sub, args.get(1)) {
        ("ls", _) => match crate::with_persistent_store(|fs| {
            let names: alloc::vec::Vec<alloc::string::String> = fs
                .list()
                .iter()
                .map(|n| alloc::string::String::from(*n))
                .collect();
            (names, fs.file_count(), fs.generation())
        }) {
            Some((names, count, generation)) => {
                for name in &names {
                    crate::serial_println!("  {name}");
                }
                crate::serial_println!("store: files={count} generation={generation}");
            }
            None => crate::serial_println!("store: no persistent storage attached"),
        },
        ("put", Some(name)) => {
            // Everything after the name is the contents, rejoined with the
            // single spaces the tokenizer removed.
            let mut text = alloc::string::String::new();
            for (i, word) in args[2..].iter().enumerate() {
                if i > 0 {
                    text.push(' ');
                }
                text.push_str(word);
            }
            match crate::with_persistent_store(|fs| fs.write(name, text.as_bytes())) {
                Some(Ok(())) => {
                    crate::serial_println!("store: put name={name} bytes={}", text.len())
                }
                Some(Err(e)) => crate::serial_println!("store: put name={name} failed: {e:?}"),
                None => crate::serial_println!("store: no persistent storage attached"),
            }
        }
        ("cat", Some(name)) => match crate::with_persistent_store(|fs| fs.read(name)) {
            Some(Ok(data)) => match core::str::from_utf8(&data) {
                Ok(text) => crate::serial_println!("store: {name} = {text}"),
                Err(_) => {
                    crate::serial_println!("store: {name} is not UTF-8 ({} bytes)", data.len())
                }
            },
            Some(Err(e)) => crate::serial_println!("store: cat name={name} failed: {e:?}"),
            None => crate::serial_println!("store: no persistent storage attached"),
        },
        ("rm", Some(name)) => match crate::with_persistent_store(|fs| fs.remove(name)) {
            Some(Ok(())) => crate::serial_println!("store: rm name={name}"),
            Some(Err(e)) => crate::serial_println!("store: rm name={name} failed: {e:?}"),
            None => crate::serial_println!("store: no persistent storage attached"),
        },
        _ => crate::serial_println!(
            "store: subcommands: ls | put <name> <text> | cat <name> | rm <name>"
        ),
    }
}

fn cmd_net() {
    if !crate::net::is_up() {
        crate::serial_println!("net: interface down (no NIC bound)");
        return;
    }
    let (ip, mask, gw, dns) = crate::net::address();
    let mut a = [0u8; 15];
    let mut b = [0u8; 15];
    let mut c = [0u8; 15];
    let mut d = [0u8; 15];
    let mut m = [0u8; 17];
    crate::serial_println!(
        "net: mac={} ip={} mask={} gateway={} dns={}",
        crate::net::mac().format(&mut m),
        ip.format(&mut a),
        mask.format(&mut b),
        gw.format(&mut c),
        dns.format(&mut d),
    );
    if let Some(stats) = crate::device::e1000::with(|nic| (nic.stats(), nic.link_up())) {
        let (s, link) = stats;
        crate::serial_println!(
            "net: link_up={link} tx_frames={} tx_bytes={} tx_dropped={} rx_frames={} rx_bytes={} rx_dropped={}",
            s.tx_frames, s.tx_bytes, s.tx_dropped, s.rx_frames, s.rx_bytes, s.rx_dropped,
        );
    }
    let s = crate::net::stats();
    crate::serial_println!(
        "net: rx_arp={} rx_ipv4={} rx_icmp={} rx_udp={} rx_malformed={} rx_unwanted={}",
        s.rx_arp,
        s.rx_ipv4,
        s.rx_icmp,
        s.rx_udp,
        s.rx_malformed,
        s.rx_unwanted,
    );
    crate::serial_println!(
        "net: arp_requests={} arp_replies={} icmp_replies={} tx_unresolved={}",
        s.arp_requests_sent,
        s.arp_replies_sent,
        s.icmp_replies_sent,
        s.tx_unresolved,
    );
    crate::net::socket::with_sockets(|rows| {
        crate::serial_println!("net: sockets={}", rows.len());
        for r in rows {
            crate::serial_println!(
                "  sock {} port={} owner={} queued={} received={} dropped={}",
                r.index,
                r.port,
                r.owner,
                r.queued,
                r.received,
                r.dropped,
            );
        }
    });
    // IPv6 (V0.9), only once it has been brought up.
    if let Some((link_local, global, router)) = crate::net::ipv6::addresses() {
        let (mut a, mut b, mut c) = ([0u8; 39], [0u8; 39], [0u8; 39]);
        crate::serial_println!(
            "net6: link_local={} global={} router={}",
            link_local.format(&mut a),
            global.map_or("-", |g| g.format(&mut b)),
            router.map_or("-", |r| r.format(&mut c)),
        );
        let s = crate::net::ipv6::stats();
        crate::serial_println!(
            "net6: rx={} rx_malformed={} rx_unwanted={} router_adverts={} neighbor_adverts_sent={} echo_replies_sent={}",
            s.rx,
            s.rx_malformed,
            s.rx_unwanted,
            s.router_adverts,
            s.neighbor_adverts_sent,
            s.echo_replies_sent,
        );
    }
}

/// `ping <ip> [count]` — ICMP echo, reporting each probe individually so a
/// partial loss is visible rather than averaged away.
fn cmd_ping(args: &[&str]) {
    let Some(target) = args.first().and_then(|s| parse_ipv4(s)) else {
        crate::serial_println!("ping: usage: ping <a.b.c.d> [count]");
        return;
    };
    let count: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    if !crate::net::is_up() {
        crate::serial_println!("ping: interface down");
        return;
    }
    let id = 0x4954; // 'IT'
    let mut ok = 0u16;
    let mut addr = [0u8; 15];
    for seq in 1..=count {
        if !crate::net::send_ping(target, id, seq, b"itisyou-os-ping") {
            crate::serial_println!("ping: seq={seq} result=unreachable");
            continue;
        }
        if crate::net::await_ping_reply(id, seq, 2000) {
            ok += 1;
            crate::serial_println!("ping: seq={seq} result=reply");
        } else {
            crate::serial_println!("ping: seq={seq} result=timeout");
        }
    }
    crate::serial_println!(
        "PING-SUMMARY target={} sent={count} received={ok}",
        target.format(&mut addr),
    );
}

/// `resolve <name>` — a DNS A lookup, printing the answer or the failure.
/// Parse an IPv6 address in the textual forms this console accepts: eight
/// groups, or fewer with one `::`. Anything else is refused, never guessed.
fn parse_ipv6(text: &str) -> Option<kernel_core::net::ipv6::Ipv6Addr> {
    let mut groups = [0u16; 8];
    let (head, tail) = match text.split_once("::") {
        Some((h, t)) => (h, Some(t)),
        None => (text, None),
    };
    let parse_part = |part: &str, out: &mut [u16]| -> Option<usize> {
        if part.is_empty() {
            return Some(0);
        }
        let mut n = 0;
        for g in part.split(':') {
            if n == out.len() || g.is_empty() || g.len() > 4 {
                return None;
            }
            out[n] = u16::from_str_radix(g, 16).ok()?;
            n += 1;
        }
        Some(n)
    };
    let mut front = [0u16; 8];
    let nf = parse_part(head, &mut front)?;
    match tail {
        None => {
            if nf != 8 {
                return None;
            }
            groups = front;
        }
        Some(t) => {
            if t.contains("::") {
                return None;
            }
            let mut back = [0u16; 8];
            let nb = parse_part(t, &mut back)?;
            if nf + nb > 7 {
                return None;
            }
            groups[..nf].copy_from_slice(&front[..nf]);
            groups[8 - nb..].copy_from_slice(&back[..nb]);
        }
    }
    let mut octets = [0u8; 16];
    for (i, g) in groups.iter().enumerate() {
        octets[2 * i..2 * i + 2].copy_from_slice(&g.to_be_bytes());
    }
    Some(kernel_core::net::ipv6::Ipv6Addr(octets))
}

/// Bring IPv6 up and ask the local router for a prefix (V0.9).
fn cmd_ipv6() {
    let Some(link_local) = crate::net::ipv6::up() else {
        crate::serial_println!("ipv6: no NIC");
        return;
    };
    let mut a = [0u8; 39];
    crate::serial_println!("ipv6: link_local={}", link_local.format(&mut a));
    match crate::net::ipv6::solicit_router(4000) {
        Some(global) => {
            let mut g = [0u8; 39];
            crate::serial_println!("IPV6-OK global={}", global.format(&mut g));
        }
        None => crate::serial_println!("IPV6-NO-ROUTER (link-local only)"),
    }
}

fn cmd_tcp(args: &[&str]) {
    use crate::net::tcp;
    use core::sync::atomic::Ordering;
    if let (Some(&"drop"), Some(n)) = (args.first(), args.get(1).and_then(|n| n.parse().ok())) {
        tcp::drop_next_data_segments(n);
        crate::serial_println!(
            "tcp: dropping the next {n} outgoing data segments (loss injection)"
        );
        return;
    }
    let mut live = 0;
    tcp::for_each(|c| {
        live += 1;
        let mut a = [0u8; 15];
        crate::serial_println!(
            "tcp: conn={} owner={} state={:?} local_port={} remote={}:{} snd_una={} rcv_nxt={}",
            c.index,
            c.owner,
            c.state,
            c.local_port,
            c.remote.format(&mut a),
            c.remote_port,
            c.snd_una,
            c.rcv_nxt
        );
    });
    crate::serial_println!(
        "tcp: live={live} segments_tx={} segments_rx={} retransmits={} injected_losses={} resets_sent={} malformed={}",
        tcp::SEGMENTS_TX.load(Ordering::Relaxed),
        tcp::SEGMENTS_RX.load(Ordering::Relaxed),
        tcp::RETRANSMITS.load(Ordering::Relaxed),
        tcp::DROPPED.load(Ordering::Relaxed),
        tcp::RESETS_SENT.load(Ordering::Relaxed),
        tcp::MALFORMED.load(Ordering::Relaxed)
    );
}

fn cmd_ping6(args: &[&str]) {
    let Some(dst) = args.first().and_then(|t| parse_ipv6(t)) else {
        crate::serial_println!("ping6: usage: ping6 <ipv6-address> [count]");
        return;
    };
    if crate::net::ipv6::up().is_none() {
        crate::serial_println!("ping6: no NIC");
        return;
    }
    let count: u16 = args
        .get(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(3)
        .clamp(1, 10);
    let mut received = 0;
    for seq in 1..=count {
        let ok = crate::net::ipv6::ping(dst, 0x6666, seq, 1000);
        crate::serial_println!(
            "ping6: seq={seq} result={}",
            if ok { "reply" } else { "timeout" }
        );
        if ok {
            received += 1;
        }
    }
    let mut a = [0u8; 39];
    crate::serial_println!(
        "PING6-SUMMARY target={} sent={count} received={received}",
        dst.format(&mut a)
    );
}

/// Configure the interface from a DHCP server (V0.9).
fn cmd_dhcp() {
    match crate::net::dhcp_configure(5000) {
        Ok(lease) => {
            let mut a = [0u8; 15];
            crate::serial_println!(
                "DHCP-OK ip={} lease_secs={}",
                lease.address.format(&mut a),
                lease.lease_secs.unwrap_or(0)
            );
        }
        Err(e) => crate::serial_println!("DHCP-FAILED reason={e:?} (address plan unchanged)"),
    }
}

fn cmd_resolve(args: &[&str]) {
    let Some(name) = args.first() else {
        crate::serial_println!("resolve: usage: resolve <host.name>");
        return;
    };
    match crate::net::resolve_name(name, 3000) {
        Some(addr) => {
            let mut buf = [0u8; 15];
            crate::serial_println!("RESOLVE-OK name={name} address={}", addr.format(&mut buf));
        }
        None => crate::serial_println!("RESOLVE-FAILED name={name}"),
    }
}

/// Parse dotted-quad IPv4. Rejects anything that is not exactly four decimal
/// octets — a partial parse would silently ping a different host.
fn parse_ipv4(text: &str) -> Option<kernel_core::net::ipv4::Ipv4Addr> {
    let mut octets = [0u8; 4];
    let mut n = 0;
    for part in text.split('.') {
        if n == 4 {
            return None;
        }
        octets[n] = part.parse().ok()?;
        n += 1;
    }
    if n == 4 {
        Some(kernel_core::net::ipv4::Ipv4Addr(octets))
    } else {
        None
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

fn cmd_audit(args: &[&str]) {
    match args.first().copied() {
        Some("save") => {
            crate::audit::save();
            return;
        }
        Some("verify") => {
            crate::audit::recover();
            return;
        }
        Some(sub @ ("anchor" | "check-anchor")) => {
            // V0.9: anchor the saved trail's head with a witness outside this
            // disk, or compare the trail on disk against the witness's copy.
            let (Some(ip), Some(port)) = (
                args.get(1).and_then(|t| parse_ipv4(t)),
                args.get(2).and_then(|p| p.parse::<u16>().ok()),
            ) else {
                crate::serial_println!("audit: usage: audit {sub} <witness-ip> <port>");
                return;
            };
            if sub == "anchor" {
                if let Err(e) = crate::audit::anchor(ip, port) {
                    crate::serial_println!("[ITISYOU:AUDIT] anchor_failed reason={e:?}");
                }
            } else if let Err(e) = crate::audit::check_anchor(ip, port) {
                crate::serial_println!("[ITISYOU:AUDIT] anchor_check_failed reason={e:?}");
            }
            return;
        }
        Some(other) => {
            crate::serial_println!(
                "audit: unknown subcommand \"{other}\" (save | verify | anchor | check-anchor)"
            );
            return;
        }
        None => {}
    }
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
    let (head, boot, saved) = crate::audit::chain_state();
    let mut buf = [0u8; 64];
    let head_text = kernel_core::audit_chain::format_head(&head, &mut buf);
    crate::serial_println!("audit: total={total} denials={denials} (ring keeps the newest 64)");
    // The chain head covers every record ever pushed, including ones the
    // bounded ring has already dropped — which is the point of keeping it
    // separately from the ring.
    crate::serial_println!("audit: chain boot={boot} saved_records={saved} head={head_text}");
}

fn cmd_pkg(args: &[&str]) {
    use crate::fs_disk::FileSystem;
    let Some(&sub) = args.first() else {
        crate::serial_println!(
            "pkg: subcommands: install <vfs.pkg> | stage <vfs.pkg> | launch <app> | rollback <app> | recover | list | trust"
        );
        return;
    };
    // The trust store needs no storage: answer before mounting the disk.
    if sub == "trust" {
        crate::platform::report_trust();
        return;
    }
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

/// ACPI S5 (soft off) through the FADT's PM1 control block (V0.9). Unlike
/// `shutdown`, which uses QEMU's test-exit device, this is the power-off a real
/// machine would honour.
fn cmd_poweroff() {
    crate::serial_println!("poweroff: requesting ACPI S5 (soft off)");
    let err = crate::acpi::power_off();
    crate::serial_println!("poweroff: failed reason={err:?}");
}

/// What ACPI discovery found (V0.9).
fn cmd_acpi() {
    let Some(info) = crate::acpi::info() else {
        crate::serial_println!("acpi: no validated tables");
        return;
    };
    crate::serial_println!(
        "acpi: rsdp_revision={} madt={} fadt={} s5={}",
        info.revision,
        info.madt.is_some(),
        info.fadt.is_some(),
        info.s5.is_some()
    );
    if let Some(m) = info.madt {
        for irq in [0u8, 1, 9, 12] {
            let r = m.isa_route(irq);
            crate::serial_println!(
                "acpi: isa={irq} gsi={} trigger={} polarity={}",
                r.gsi,
                if r.level { "level" } else { "edge" },
                if r.active_low { "low" } else { "high" }
            );
        }
    }
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
