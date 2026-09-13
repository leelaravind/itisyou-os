//! Ring 3 console output, line-atomic and unable to forge kernel markers
//! (V0.10, OUT10-001/002).
//!
//! `sys_write` hands its bytes to [`write`], which appends them to the
//! calling process's line buffer and sends every COMPLETE line to the serial
//! port in one locked write. The syscall runs with interrupts masked on a
//! single CPU, so a line reaches the port whole: no other process, and no
//! kernel message, can land inside it. A process that exits mid-line has
//! its partial line flushed (with a newline) by [`flush_owner`] before the
//! kernel reports the exit.
//!
//! Every `[ITISYOU:` a process prints becomes `[RING3-U:`: the kernel's
//! evidence prefix belongs to the kernel.

use kernel_core::linebuf::{neutralize_markers, LineBuf};
use spin::Mutex;

/// Bytes of partial line held per process.
const LINE: usize = 256;
/// Processes with a buffered partial line at once; beyond this, output is
/// written through unbuffered (the pre-V0.10 behaviour), never dropped.
const SLOTS: usize = 16;

struct Slot {
    pid: u64,
    used: bool,
    buf: LineBuf<LINE>,
}

const FREE: Slot = Slot {
    pid: 0,
    used: false,
    buf: LineBuf::new(),
};

static OUT: Mutex<[Slot; SLOTS]> = Mutex::new([FREE; SLOTS]);

/// Send one line (or chunk) to the serial port in a single locked write:
/// kernel markers neutralized, invalid UTF-8 shown as U+FFFD.
fn emit(chunk: &[u8]) {
    let mut line = [0u8; LINE + 1];
    let n = chunk.len().min(line.len());
    line[..n].copy_from_slice(&chunk[..n]);
    neutralize_markers(&mut line[..n]);
    crate::serial::write_fmt(format_args!("{}", Lossy(&line[..n])));
}

/// Display bytes as UTF-8, replacing each invalid sequence with U+FFFD.
struct Lossy<'a>(&'a [u8]);

impl core::fmt::Display for Lossy<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for chunk in self.0.utf8_chunks() {
            f.write_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                f.write_str("\u{FFFD}")?;
            }
        }
        Ok(())
    }
}

/// Append `bytes` written by `pid`; complete lines go out immediately.
pub fn write(pid: u64, bytes: &[u8]) {
    let mut slots = OUT.lock();
    let idx = slots
        .iter()
        .position(|s| s.used && s.pid == pid)
        .or_else(|| slots.iter().position(|s| !s.used));
    let Some(idx) = idx else {
        // No buffer free: write through rather than lose output. Still
        // neutralize markers, one bounded chunk at a time.
        drop(slots);
        for chunk in bytes.chunks(LINE) {
            emit(chunk);
        }
        return;
    };
    let slot = &mut slots[idx];
    slot.used = true;
    slot.pid = pid;
    slot.buf.push(bytes, emit);
}

/// Flush `pid`'s partial line (a newline is appended) and release its slot.
pub fn flush_owner(pid: u64) {
    let mut slots = OUT.lock();
    for slot in slots.iter_mut().filter(|s| s.used && s.pid == pid) {
        slot.buf.flush(emit);
        *slot = FREE;
    }
}
