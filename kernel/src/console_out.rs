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
//! evidence prefix belongs to the kernel. Since V0.11 its control
//! characters are shown escaped as well (`\x1b`, `\x0d`, ...), so it cannot
//! move the cursor to redraw the kernel's lines on the operator's terminal.

use crate::sync::Mutex;
use kernel_core::linebuf::{neutralize_markers, LineBuf};

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
/// kernel markers neutralized, invalid UTF-8 shown as U+FFFD, and (V0.11,
/// OUT11-001) every control character but the line feed and the tab shown
/// escaped rather than performed.
fn emit(chunk: &[u8]) {
    let mut line = [0u8; LINE + 1];
    let n = chunk.len().min(line.len());
    line[..n].copy_from_slice(&chunk[..n]);
    neutralize_markers(&mut line[..n]);
    crate::serial::write_fmt(format_args!("{}", Escaped(&line[..n])));
}

/// Process output as the console shows it (`linebuf::write_escaped`).
struct Escaped<'a>(&'a [u8]);

impl core::fmt::Display for Escaped<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        kernel_core::linebuf::write_escaped(self.0, f)
    }
}

/// Per-process line buffers in use (V1.0 leak accounting).
pub fn slots_in_use() -> usize {
    OUT.lock().iter().filter(|s| s.used).count()
}

/// Append `bytes` written by `pid`; complete lines go out immediately.
pub fn write(pid: u64, bytes: &[u8]) {
    let mut slots = OUT.lock();
    let idx = slots
        .iter()
        .position(|s| s.used && s.pid == pid)
        .or_else(|| slots.iter().position(|s| !s.used));
    let Some(idx) = idx else {
        // No buffer free: write through rather than lose output, through a
        // buffer of this write's own (so a marker split across its chunks
        // is still caught), ended with a newline if the write leaves a line
        // open - the next write cannot complete a marker this one started
        // (V0.11; v0.10.0 emitted each 256-byte chunk and each write on its
        // own, and a split marker reached the port whole).
        drop(slots);
        let mut once = LineBuf::<LINE>::new();
        once.push(bytes, emit);
        once.flush(emit);
        return;
    };
    let slot = &mut slots[idx];
    slot.used = true;
    slot.pid = pid;
    slot.buf.push(bytes, emit);
}

/// Emit `pid`'s partial line without a newline (V0.10): the prompt a
/// program shows before it reads a line. Markers are neutralized as always.
pub fn flush_prompt(pid: u64) {
    let mut slots = OUT.lock();
    for slot in slots.iter_mut().filter(|s| s.used && s.pid == pid) {
        slot.buf.flush_partial(emit);
    }
}

/// Flush `pid`'s partial line (a newline is appended) and release its slot.
pub fn flush_owner(pid: u64) {
    let mut slots = OUT.lock();
    for slot in slots.iter_mut().filter(|s| s.used && s.pid == pid) {
        slot.buf.flush(emit);
        *slot = FREE;
    }
}
