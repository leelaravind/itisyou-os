//! `/bin/sh` — the Ring 3 shell (V0.10, SHELL10-001).
//!
//! Started by the kernel console's `rsh`, which hands it the console's input
//! (`console_read`, syscall 40) and waits until it exits. It reads a line,
//! splits it on spaces, and runs builtins or programs:
//!
//! * `echo <words>`, `help`, `pid`, `uptime`, `exit [code]`,
//!   `rsh-only-builtin` (a command only this shell knows, so a test can tell
//!   which shell read the line);
//! * `run <path> [caps|-] [-- args]` — spawn the program with the named
//!   capabilities (default: everything this shell holds; `-`: none) and the
//!   arguments, then wait for it: `rsh: <path>: exit=<n>` (or `fault=<v>`,
//!   `killed`).
//!
//! Kernel-only commands (`store`, `pkg`, `net`, …) stay on the kernel
//! console. No allocator.

#![no_std]
#![no_main]

use kernel_core::caps;
use kernel_core::procstatus::{self, Status};
use kernel_core::progargs::{self, MAX_BLOCK};
use ulib::{
    console_read, exit, getpid, spawn_args, uptime_ticks, wait, write, write_u64, ERR_2BIG,
};

/// Longest line (the console line discipline's limit).
const LINE: usize = 256;
/// Most tokens on a line.
const TOKENS: usize = 24;

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

fn write_err(v: u64) {
    write_u64(u64::MAX - v);
}

fn tokens<'a>(line: &'a str, out: &mut [&'a str; TOKENS]) -> Option<usize> {
    let mut n = 0;
    for t in line.split(' ').filter(|t| !t.is_empty()) {
        if n == TOKENS {
            return None;
        }
        out[n] = t;
        n += 1;
    }
    Some(n)
}

fn report_status(path: &str, status: u64) {
    write("rsh: ");
    write(path);
    match procstatus::decode(status) {
        Some(Status::Exited(code)) => {
            write(": exit=");
            write_u64(u64::from(code));
        }
        Some(Status::Faulted(v)) => {
            write(": fault=");
            write_u64(u64::from(v));
        }
        Some(Status::Killed) => {
            write(": killed");
        }
        None => {
            write(": wait failed err=");
            write_err(status);
        }
    }
    write("\n");
}

/// `run <path> [caps|-] [-- args]`.
fn run(tok: &[&str]) {
    let Some(&path) = tok.first() else {
        write("rsh: run: missing program path\n");
        return;
    };
    let rest = &tok[1..];
    let sep = rest.iter().position(|t| *t == "--");
    let (options, args) = match sep {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &rest[..0]),
    };
    if options.len() > 1 {
        write("rsh: run: unexpected argument (program arguments go after --)\n");
        return;
    }
    let requested = match options.first() {
        None => u64::MAX,
        Some(&"-") => 0,
        Some(list) => match caps::parse(list) {
            Ok(c) => c,
            Err(_) => {
                write("rsh: run: unknown capability in ");
                write(list);
                write("\n");
                return;
            }
        },
    };
    let mut block = [0u8; MAX_BLOCK];
    let n = match progargs::encode(args, &mut block) {
        Ok(n) => n,
        Err(e) => {
            write("rsh: run: arguments rejected: ");
            write(e.name());
            write("\n");
            return;
        }
    };
    let child = spawn_args(path, requested, &block[..n]);
    if is_err(child) {
        write("rsh: ");
        write(path);
        write(": spawn failed err=");
        write_err(child);
        write("\n");
        return;
    }
    report_status(path, wait(child));
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    write("RSH-READY pid=");
    write_u64(getpid());
    write("\n");
    let mut buf = [0u8; LINE];
    loop {
        write("rsh> ");
        let n = console_read(&mut buf);
        if n == ERR_2BIG || is_err(n) {
            write("RSH-READ-ERROR err=");
            write_err(n);
            write("\n");
            exit(1)
        }
        let Ok(line) = core::str::from_utf8(&buf[..n as usize]) else {
            continue;
        };
        let mut tok = [""; TOKENS];
        let Some(count) = tokens(line, &mut tok) else {
            write("rsh: too many words\n");
            continue;
        };
        let tok = &tok[..count];
        match tok.first().copied() {
            None => {}
            Some("echo") => {
                for (i, w) in tok[1..].iter().enumerate() {
                    if i > 0 {
                        write(" ");
                    }
                    write(w);
                }
                write("\n");
            }
            Some("help") => {
                write("rsh: echo <words> | pid | uptime | run <path> [caps|-] [-- args] | exit [code] | help\n");
            }
            Some("pid") => {
                write("rsh: pid=");
                write_u64(getpid());
                write("\n");
            }
            Some("uptime") => {
                write("rsh: ticks=");
                write_u64(uptime_ticks());
                write("\n");
            }
            Some("rsh-only-builtin") => write_ok(),
            Some("run") => run(&tok[1..]),
            Some("exit") => {
                let code = tok.get(1).and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
                write("RSH-EXIT code=");
                write_u64(code);
                write("\n");
                exit(code)
            }
            Some(other) => {
                write("rsh: unknown command: ");
                write(other);
                write("\n");
            }
        }
    }
}

fn write_ok() {
    write("RSH-BUILTIN-OK\n");
}
