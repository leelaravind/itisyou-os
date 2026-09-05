//! Test fixture: a fake `qemu-system-x86_64` for exercising the harness's
//! startup/timeout state machine deterministically.
//!
//! The real QEMU cannot be driven into the interesting failure modes on
//! demand (never connecting, connecting then staying mute, dying at launch),
//! so those paths would otherwise be untested — which is exactly how the V0.8
//! "empty serial" failures stayed misdiagnosed as harness flakiness for so
//! long. This binary speaks the same serial transport the runner expects (it
//! dials the runner's TCP listener, parsed out of `-serial tcp:HOST:PORT`) and
//! picks a behaviour from `STUB_QEMU_MODE`:
//!
//!   `exit-immediately` — die before connecting (bad command line)
//!   `never-connect`    — stay alive, never dial back (transport failure)
//!   `silent`           — connect, emit nothing (guest never boots)
//!   `stall`            — emit one marker, then go silent forever
//!   `markers`          — emit the full boot marker set, exit 33 like
//!                        `isa-debug-exit` does on a clean guest shutdown
//!
//! Any unrecognised argument is ignored, so it tolerates the full QEMU
//! command line the runner builds.

use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

/// Long enough that the runner's own deadline always fires first; the runner
/// kills this process, which is the behaviour under test.
const FOREVER: Duration = Duration::from_secs(600);

/// QEMU's `isa-debug-exit` success code: `(0x10 << 1) | 1`.
const EXIT_SUCCESS: i32 = 33;

fn main() {
    let mode = std::env::var("STUB_QEMU_MODE").unwrap_or_else(|_| "markers".into());
    if mode == "exit-immediately" {
        eprintln!("stub-qemu: refusing to start (simulated bad command line)");
        std::process::exit(3);
    }
    if mode == "never-connect" {
        std::thread::sleep(FOREVER);
        return;
    }

    let addr = serial_addr().expect("stub-qemu: no -serial tcp:HOST:PORT argument");
    let mut stream = TcpStream::connect(&addr).expect("stub-qemu: connecting to runner serial");
    stream.set_nodelay(true).ok();

    match mode.as_str() {
        "silent" => {
            std::thread::sleep(FOREVER);
        }
        "stall" => {
            writeln!(stream, "[ITISYOU:B010] kernel entry reached in 64-bit mode").ok();
            stream.flush().ok();
            std::thread::sleep(FOREVER);
        }
        "markers" => {
            for line in [
                "[ITISYOU:B010] kernel entry reached in 64-bit mode",
                "[ITISYOU:B020] early serial console ready",
                "[ITISYOU:B030] CPU baseline established",
            ] {
                writeln!(stream, "{line}").ok();
            }
            stream.flush().ok();
            // Let the runner drain the socket before the process exits.
            std::thread::sleep(Duration::from_millis(150));
            std::process::exit(EXIT_SUCCESS);
        }
        other => {
            eprintln!("stub-qemu: unknown STUB_QEMU_MODE {other}");
            std::process::exit(2);
        }
    }
}

/// Extract `HOST:PORT` from the `-serial tcp:127.0.0.1:PORT,nodelay` pair.
fn serial_addr() -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg != "-serial" {
            continue;
        }
        let value = args.next()?;
        let rest = value.strip_prefix("tcp:")?;
        let addr = rest.split(',').next()?;
        return Some(addr.to_string());
    }
    None
}
