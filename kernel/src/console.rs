//! Who owns the console's input (V0.10, SHELL10-001).
//!
//! The kernel console reads the UART itself (`shell::read_line`). While a
//! Ring 3 program owns the input — the kernel's `rsh` hands it to `/bin/sh`
//! — the kernel still reads the bytes, but only at its job-wait point and
//! only while the owner is waiting for a line: one line at a time, edited by the
//! same host-tested line discipline, echoed as they are consumed. The rest
//! of the input stays in the UART and QEMU's buffer, so nothing the owner
//! has not yet asked for is ever taken from the kernel console.
//!
//! The owner reads a line with `console_read` (syscall 40). Ownership is the
//! authority: only the kernel grants it (`rsh`), and any other process is
//! refused. It returns to the kernel on every exit path of the owner
//! (`proc::release_owned`).

use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use kernel_core::linedisc::{Event, LineDisc};

/// The input owner: 0 = the kernel console, otherwise a pid.
static OWNER: AtomicU64 = AtomicU64::new(0);

struct Rx {
    ld: LineDisc,
    /// A complete line is waiting for the owner.
    ready: bool,
}

static RX: Mutex<Rx> = Mutex::new(Rx {
    ld: LineDisc::new(),
    ready: false,
});

pub fn owner() -> u64 {
    OWNER.load(Ordering::SeqCst)
}

/// Hand the console's input to `pid` (the kernel's `rsh`).
pub fn hand_to(pid: u64) {
    {
        let mut rx = RX.lock();
        rx.ld.reset();
        rx.ready = false;
    }
    OWNER.store(pid, Ordering::SeqCst);
    crate::serial_println!("[ITISYOU:CONSOLE] owner={pid} reason=granted");
}

/// `pid` is going away: if it owned the input, the kernel takes it back.
pub fn release(pid: u64) {
    if pid == 0 || OWNER.load(Ordering::SeqCst) != pid {
        return;
    }
    OWNER.store(0, Ordering::SeqCst);
    {
        let mut rx = RX.lock();
        rx.ld.reset();
        rx.ready = false;
    }
    crate::serial_println!("[ITISYOU:CONSOLE] owner=kernel reason=owner_exit");
}

/// Move input bytes into the owner's line, while it waits for one
/// (called from the console's job-wait loop). Wakes the owner when a line is
/// complete.
pub fn pump_rx() {
    let owner = OWNER.load(Ordering::SeqCst);
    if owner == 0 {
        return;
    }
    // Only once the owner has asked (it is blocked in `console_read`, its
    // prompt shown): the echo then follows the prompt, as on a terminal.
    if crate::proc::state_of(owner) != Some(crate::proc::ProcState::ConsoleWait) {
        return;
    }
    let mut wake = false;
    {
        let mut rx = RX.lock();
        while !rx.ready {
            let Some(byte) = crate::serial::try_read_byte() else {
                break;
            };
            let (echo, event) = rx.ld.feed(byte);
            let mut scratch = [0u8; 3];
            let bytes = echo.bytes(&mut scratch);
            if !bytes.is_empty() {
                if let Ok(s) = core::str::from_utf8(bytes) {
                    crate::serial_print!("{s}");
                }
            }
            match event {
                Event::Pending => {}
                Event::Line => {
                    rx.ready = true;
                    wake = true;
                }
                Event::Overflow => {
                    rx.ld.reset();
                    crate::serial_println!(
                        "error: input line exceeded {} bytes; discarded",
                        kernel_core::linedisc::MAX_LINE
                    );
                }
            }
        }
    }
    // Lock order: the process table is never taken under RX.
    if wake {
        crate::proc::wake_console(owner);
    }
}

/// console_read(buf_ptr, len) — the input owner reads one complete line
/// (V0.10, syscall 40). Returns the line's length; `ERR_2BIG` (the line is
/// kept) if `len` is too small; `ERR_PERM` for anyone but the owner
/// (audited). With no line yet, the caller blocks (R4: marked, not waiting
/// inside the syscall) and resumes with `ERR_AGAIN` when one arrives — the
/// ulib wrapper then asks again.
pub fn sys_console_read(buf_ptr: u64, len: u64) -> u64 {
    use crate::syscall::{ERR_2BIG, ERR_AGAIN, ERR_PERM};
    let caller = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
    if caller == 0 || OWNER.load(Ordering::SeqCst) != caller {
        crate::audit::denied_reason("console_read", 0, "not_owner");
        return ERR_PERM;
    }
    // The prompt the program wrote without a newline must show before the
    // program waits for the answer.
    crate::console_out::flush_prompt(caller);
    let mut rx = RX.lock();
    if rx.ready {
        let n = rx.ld.line().len();
        if (len as usize) < n {
            return ERR_2BIG;
        }
        let result =
            crate::syscall::copy_to_user(buf_ptr, rx.ld.line()).map_or_else(|e| e, |_| n as u64);
        if result == n as u64 {
            rx.ld.reset();
            rx.ready = false;
        }
        return result;
    }
    drop(rx);
    crate::proc::block_on_console(caller);
    crate::user::transition::save_yield_context(ERR_AGAIN);
    crate::user::transition::abort_block();
}
