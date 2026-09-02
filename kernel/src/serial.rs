//! Early serial console (COM1, 16550 UART).
//!
//! This is the kernel's primary observability channel: every boot stage,
//! panic, and test event goes through here. It must work before any memory
//! management exists, so it is allocation-free.

use core::fmt;
use spin::Mutex;
use uart_16550::SerialPort;

pub const COM1_PORT: u16 = 0x3F8;

static SERIAL1: Mutex<Option<SerialPort>> = Mutex::new(None);

/// Initialize COM1. Idempotent; later calls re-init the port harmlessly.
pub fn init() {
    // SAFETY: 0x3F8 is the standard COM1 I/O port on the QEMU pc/q35
    // machine models targeted by V0.1.
    let mut port = unsafe { SerialPort::new(COM1_PORT) };
    port.init();
    *SERIAL1.lock() = Some(port);
}

/// Write formatted output to the serial console.
///
/// Interrupts are disabled around the lock so an interrupt handler that logs
/// cannot deadlock against a preempted writer.
pub fn write_fmt(args: fmt::Arguments) {
    use fmt::Write;
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(port) = SERIAL1.lock().as_mut() {
            let _ = port.write_fmt(args);
        }
    });
}

/// Non-blocking read of one byte from COM1's receive buffer.
///
/// Polled input is the V0.1 input path (plan §10.10): the QEMU test harness
/// drives the shell through serial stdin.
pub fn try_read_byte() -> Option<u8> {
    use x86_64::instructions::port::Port;
    // SAFETY: LSR (base+5) and RBR (base) reads on the standard COM1 UART;
    // reading has no side effects beyond consuming the received byte.
    unsafe {
        let mut lsr = Port::<u8>::new(COM1_PORT + 5);
        if lsr.read() & 1 != 0 {
            let mut rbr = Port::<u8>::new(COM1_PORT);
            Some(rbr.read())
        } else {
            None
        }
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::write_fmt(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => { $crate::serial_print!("\n") };
    ($($arg:tt)*) => {{
        $crate::serial::write_fmt(core::format_args!($($arg)*));
        $crate::serial_print!("\n");
    }};
}
