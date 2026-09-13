//! `ai-probe` - drives the AI-native system layer's kernel interfaces from
//! Ring 3 (V0.11, ADR-0024) and prints what the kernel answered. The first
//! argument picks the mode:
//!
//! * `view` - read the approved view (`sys_view`), decode it with the same
//!   host-tested `kernel_core::sysview` the agent uses, and print it:
//!   `AIPROBE-VIEW sched ...`, one `AIPROBE-VIEW service=...` per row, the
//!   audit counts and the 16 features. A buffer one byte too small must be
//!   refused (`ERR_2BIG`) before anything is copied.
//! * `view-denied` - run WITHOUT the view capability: the call must be
//!   refused (`AIPROBE-VIEW-DENIED err=perm`); if the kernel served a view
//!   anyway the probe prints `AIPROBE-VIEW-LEAK`, which the test forbids.
//! * `infer` (S5) - ask `/bin/inferd` about the real view and check the
//!   answer against the model computed here from `/etc/ai/diag.model`:
//!   `AIPROBE-INFER conditions=<set> match=true model_match=true ...`.

#![no_std]
#![no_main]

use kernel_core::sysview::{self, View};
use ulib::{args, exit, split_args, sys_view, write, write_u64, ERR_2BIG, ERR_PERM};

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

fn flag(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

fn fail(step: &str, v: u64) -> ! {
    write("AIPROBE-FAILED step=");
    write(step);
    write(" err=");
    write_u64(u64::MAX - v);
    write("\n");
    exit(1)
}

fn view() {
    // One byte short: refused whole, nothing copied.
    let mut small = [0u8; sysview::LEN - 1];
    let r = sys_view(&mut small);
    if r != ERR_2BIG {
        fail("small-buffer", r);
    }
    if small.iter().any(|&b| b != 0) {
        write("AIPROBE-FAILED step=small-buffer-written\n");
        exit(1);
    }
    write("AIPROBE-VIEW-SMALL-OK\n");

    let mut buf = [0u8; sysview::LEN + 8];
    let n = sys_view(&mut buf);
    if is_err(n) {
        fail("sys_view", n);
    }
    if n as usize != sysview::LEN {
        write("AIPROBE-FAILED step=length\n");
        exit(1);
    }
    let v = match View::decode(&buf[..sysview::LEN]) {
        Ok(v) => v,
        Err(e) => {
            write("AIPROBE-VIEW-DECODE-FAILED reason=");
            write(e.name());
            write("\n");
            exit(1)
        }
    };
    write("AIPROBE-VIEW sched paused=");
    write(flag(v.paused));
    write(" always_on=");
    write(flag(v.always_on));
    write(" processes=");
    write_u64(u64::from(v.processes));
    write(" runnable=");
    write_u64(u64::from(v.runnable));
    write(" waiting=");
    write_u64(u64::from(v.waiting));
    write(" ended=");
    write_u64(u64::from(v.ended));
    write(" truncated=");
    write(flag(v.truncated));
    write(" \n");
    for row in v.rows() {
        write("AIPROBE-VIEW service=");
        write(row.name_str());
        write(" state=");
        write(row.state.name());
        write(" restarts=");
        write_u64(u64::from(row.restarts));
        write(" by_init=");
        write(flag(row.by_init));
        write(" \n");
    }
    write("AIPROBE-VIEW audit records=");
    write_u64(u64::from(v.recent_records));
    write(" denials=");
    write_u64(u64::from(v.recent_denials));
    write(" \nAIPROBE-VIEW features=");
    for (i, f) in sysview::features(&v).iter().enumerate() {
        if i > 0 {
            write(",");
        }
        write_u64(*f as u64);
    }
    write("\nAIPROBE-VIEW-OK\n");
}

fn view_denied() {
    let mut buf = [0u8; sysview::LEN];
    let r = sys_view(&mut buf);
    if r == ERR_PERM {
        write("AIPROBE-VIEW-DENIED err=perm\n");
    } else if is_err(r) {
        fail("view-denied", r);
    } else {
        write("AIPROBE-VIEW-LEAK\n");
        exit(1);
    }
}

// --- `infer` (S5) -------------------------------------------------------------

/// A `core::fmt::Write` onto the console, for the kernel-core formatters.
struct Out;

impl core::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write(s);
        Ok(())
    }
}

fn write_i64(v: i64) {
    if v < 0 {
        write("-");
    }
    write_u64(v.unsigned_abs());
}

fn write_prefix(d: &[u8; 32]) {
    for b in &d[..8] {
        ulib::write_hex8(*b);
    }
}

/// The view, decoded (the `view` mode's first half, without printing).
fn read_view() -> View {
    let mut buf = [0u8; sysview::LEN];
    let n = sys_view(&mut buf);
    if is_err(n) {
        fail("sys_view", n);
    }
    match View::decode(&buf) {
        Ok(v) => v,
        Err(e) => {
            write("AIPROBE-VIEW-DECODE-FAILED reason=");
            write(e.name());
            write("\n");
            exit(1)
        }
    }
}

/// Ask inferd about the real view, and check its answer against the same
/// model computed here from the file the probe reads itself: Ring 3
/// inference must be exactly the host-tested arithmetic on the shipped bytes.
fn infer() {
    use kernel_core::infer::{self, Reply, Request};
    let x = sysview::features(&read_view());
    let mut file = [0u8; kernel_core::model::LEN + 1];
    let n = ulib::fs_read("/etc/ai/diag.model", &mut file);
    if is_err(n) {
        fail("read-model", n);
    }
    let bytes = &file[..n as usize];
    let m = match kernel_core::model::decode(bytes) {
        Ok(m) => m,
        Err(e) => {
            write("AIPROBE-FAILED step=decode-model reason=");
            write(e.name());
            write("\n");
            exit(1)
        }
    };
    let digest = kernel_core::sha256::digest(bytes);
    let nonce = (ulib::uptime_ticks() << 20) ^ ulib::getpid();
    let req = Request { nonce, x };
    if ulib::msg_send(infer::REQUEST_CHANNEL, &req.encode()) != infer::REQUEST_LEN as u64 {
        write("AIPROBE-INFER-SEND-FAILED\n");
        exit(1);
    }
    // Bounded wait: up to 3 s for inferd (a background service) to answer.
    let mut buf = [0u8; 256];
    let mut reply = None;
    for _ in 0..300 {
        let n = ulib::msg_recv(infer::REPLY_CHANNEL, &mut buf);
        if is_err(n) {
            ulib::sleep_ticks(1);
            continue;
        }
        match Reply::decode(&buf[..n as usize]) {
            Ok(r) if r.nonce == nonce => {
                reply = Some(r);
                break;
            }
            // Somebody else's answer (or garbage): not ours, keep waiting.
            _ => {
                write("AIPROBE-INFER-FOREIGN-REPLY\n");
            }
        }
    }
    let Some(r) = reply else {
        write("AIPROBE-INFER-TIMEOUT\n");
        exit(1)
    };
    let local = infer::answer(&m, &digest, &req);
    write("AIPROBE-INFER conditions=");
    let _ = r.conditions.write(&mut Out);
    write(" match=");
    write(if r == local { "true" } else { "false" });
    write(" model_match=");
    write(if r.model == digest { "true" } else { "false" });
    write(" scores=");
    for (i, s) in r.scores.iter().enumerate() {
        if i > 0 {
            write(",");
        }
        write_i64(*s);
    }
    write(" model=");
    write_prefix(&r.model);
    write(" \n");
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; 128];
    let len = args(&mut block);
    let mode = if is_err(len) {
        None
    } else {
        split_args(&block[..len as usize]).next()
    };
    match mode {
        Some(b"view") => view(),
        Some(b"view-denied") => view_denied(),
        Some(b"infer") => infer(),
        _ => {
            write("AIPROBE-FAILED step=mode\n");
            exit(2)
        }
    }
    exit(0)
}
