//! `inferd` - the local inference service (V0.11, INFER11-001, ADR-0024).
//!
//! The diagnostic model runs HERE, in Ring 3, outside the kernel. `/sbin/init`
//! starts inferd from `/etc/init.conf` with IPC and read access under `/etc`
//! (init's sandbox) and nothing else. It loads `/etc/ai/diag.model`, refuses
//! a hostile file by name, and answers classification requests on the
//! channels of `kernel_core::infer` with the host-tested
//! `kernel_core::model`. What it answers is a claim, not authority: the
//! agent that asked holds no power to act, and the kernel recomputes a
//! proposal's condition itself.
//!
//! Modes (first argument):
//! * none - serve: `INFERD-READY model=<sha256 prefix> channel=6`, then one
//!   `INFERD-SERVED n=` per answer. It sleeps a tick whenever there is
//!   nothing to do, so it never spins.
//! * `check <path>` - load and validate a model file, print
//!   `INFERD-MODEL-OK` or `INFERD-MODEL-REFUSED ... reason=`, and exit.

#![no_std]
#![no_main]

use kernel_core::infer::{self, Request};
use kernel_core::model::{self, Model};
use ulib::{
    args, exit, fs_read, msg_recv, msg_send, sleep_ticks, split_args, write, write_hex8, write_u64,
    ERR_2BIG, ERR_AGAIN,
};

const MODEL_PATH: &str = "/etc/ai/diag.model";

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

fn write_prefix(d: &[u8; 32]) {
    for b in &d[..8] {
        write_hex8(*b);
    }
}

/// Load and validate a model file: the model and its SHA-256, or the name
/// of the check it failed.
fn load(path: &str) -> Result<(Model, [u8; 32]), &'static str> {
    // One byte more than a model, so an oversized file is caught as such.
    let mut buf = [0u8; model::LEN + 1];
    let n = fs_read(path, &mut buf);
    if n == ERR_2BIG {
        return Err("too_large");
    }
    if is_err(n) {
        return Err("unreadable");
    }
    let bytes = &buf[..n as usize];
    let m = model::decode(bytes).map_err(|e| e.name())?;
    Ok((m, kernel_core::sha256::digest(bytes)))
}

fn refused(path: &str, reason: &str) {
    write("INFERD-MODEL-REFUSED path=");
    write(path);
    write(" reason=");
    write(reason);
    write("\n");
}

fn check(path: &str) -> ! {
    match load(path) {
        Ok((_, digest)) => {
            write("INFERD-MODEL-OK path=");
            write(path);
            write(" sha256=");
            write_prefix(&digest);
            write("\n");
        }
        Err(reason) => refused(path, reason),
    }
    exit(0)
}

fn serve() -> ! {
    let (m, digest) = match load(MODEL_PATH) {
        Ok(x) => x,
        Err(reason) => {
            refused(MODEL_PATH, reason);
            exit(1)
        }
    };
    write("INFERD-READY model=");
    write_prefix(&digest);
    write(" channel=");
    write_u64(infer::REQUEST_CHANNEL);
    write("\n");
    let mut served = 0u64;
    let mut buf = [0u8; 256];
    loop {
        let n = msg_recv(infer::REQUEST_CHANNEL, &mut buf);
        if n == ERR_AGAIN || is_err(n) {
            // Nothing to do: give the tick back rather than spin.
            sleep_ticks(1);
            continue;
        }
        match Request::decode(&buf[..n as usize]) {
            Ok(req) => {
                let reply = infer::answer(&m, &digest, &req).encode();
                // A full reply queue drops this answer; the client times out
                // and asks again rather than blocking the service.
                if msg_send(infer::REPLY_CHANNEL, &reply) != ERR_AGAIN {
                    served += 1;
                    write("INFERD-SERVED n=");
                    write_u64(served);
                    write("\n");
                }
            }
            Err(e) => {
                write("INFERD-BAD-REQUEST reason=");
                write(e.name());
                write("\n");
            }
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; 256];
    let len = args(&mut block);
    if is_err(len) || len == 0 {
        serve();
    }
    let mut it = split_args(&block[..len as usize]);
    match (it.next(), it.next()) {
        (Some(b"check"), Some(path)) => match core::str::from_utf8(path) {
            Ok(p) => check(p),
            Err(_) => refused("?", "bad_path"),
        },
        _ => {
            write("INFERD-USAGE inferd | inferd check <path>\n");
        }
    }
    exit(2)
}
