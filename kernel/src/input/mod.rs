//! PS/2 keyboard + mouse input (V0.5, B180).
//!
//! Initializes the i8042 controller (enables the mouse, unmasks IRQ1/IRQ12),
//! decodes scancodes / mouse packets in the IRQ handlers via the host-tested
//! `kernel_core` decoders, and posts events to a bounded queue the desktop
//! loop drains. Every received event is also logged to serial as a
//! machine-parseable `[ITISYOU:INPUT]` marker so an automated test can drive
//! input through the QEMU monitor and assert the OS received it.

use crate::sync::Mutex;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use kernel_core::mouse::{Mouse, MouseEvent};
use kernel_core::scancode::{KeyEvent, Keyboard};
use x86_64::instructions::interrupts::without_interrupts;
use x86_64::instructions::port::Port;

const DATA: u16 = 0x60;
const STATUS_CMD: u16 = 0x64;

static KEYBOARD: Mutex<Keyboard> = Mutex::new(Keyboard::new());
static MOUSE: Mutex<Mouse> = Mutex::new(Mouse::new());
static QUEUE: Mutex<VecDeque<InputEvent>> = Mutex::new(VecDeque::new());
const QUEUE_MAX: usize = 256;

pub static KEY_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MOUSE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Accumulated mouse motion since the last `take_mouse_motion`. The IRQ
/// handler MUST NOT touch the compositor/framebuffer locks (a composite may
/// hold them); it only accumulates here, and the desktop loop applies it.
static ACCUM_DX: AtomicI32 = AtomicI32::new(0);
static ACCUM_DY: AtomicI32 = AtomicI32::new(0);

#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

fn wait_write() {
    let mut status = Port::<u8>::new(STATUS_CMD);
    for _ in 0..100_000 {
        // SAFETY: reading the i8042 status port is side-effect-free.
        if unsafe { status.read() } & 0x02 == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn wait_read() {
    let mut status = Port::<u8>::new(STATUS_CMD);
    for _ in 0..100_000 {
        if unsafe { status.read() } & 0x01 != 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

/// Write a command byte to the controller (port 0x64).
fn cmd(byte: u8) {
    wait_write();
    // SAFETY: standard i8042 command port.
    unsafe { Port::<u8>::new(STATUS_CMD).write(byte) };
}

/// Write a data byte to the aux (mouse) device.
fn mouse_write(byte: u8) {
    cmd(0xD4); // next byte goes to the auxiliary device
    wait_write();
    unsafe { Port::<u8>::new(DATA).write(byte) };
    // Consume the ACK (0xFA) if present.
    wait_read();
    let _ = unsafe { Port::<u8>::new(DATA).read() };
}

/// Initialize the PS/2 controller: enable the mouse and IRQs. Requires the
/// IDT/PIC already up (called at B180).
pub fn init() {
    // Enable the auxiliary (mouse) device.
    cmd(0xA8);

    // Read the controller config byte, enable IRQ1 (bit0) and IRQ12 (bit1),
    // ensure translation stays on for keyboard (bit6).
    cmd(0x20);
    wait_read();
    let mut config = unsafe { Port::<u8>::new(DATA).read() };
    config |= 0b0000_0011; // kbd + mouse interrupts
    config &= !0b0010_0000; // clear mouse clock disable
    cmd(0x60);
    wait_write();
    unsafe { Port::<u8>::new(DATA).write(config) };

    // Tell the mouse to use defaults and enable data reporting.
    mouse_write(0xF6); // set defaults
    mouse_write(0xF4); // enable data reporting

    // Drain stale bytes (ACKs, BAT results) still sitting in the output buffer
    // so the first real mouse packet frames on its own always-1 byte instead
    // of on a leftover byte (which would mis-decode the first movement).
    {
        let mut data = Port::<u8>::new(DATA);
        let mut status = Port::<u8>::new(STATUS_CMD);
        for _ in 0..32 {
            // SAFETY: reading the status/data ports is side-effect-free.
            if unsafe { status.read() } & 0x01 == 0 {
                break;
            }
            let _ = unsafe { data.read() };
        }
    }

    // Unmask IRQ1 (keyboard), IRQ2 (cascade) and IRQ12 (mouse) at the PICs,
    // keeping IRQ0 (timer) unmasked.
    crate::interrupts::set_input_irqs_enabled();

    crate::serial_println!("[ITISYOU:INFO] input ps2 keyboard+mouse enabled");
}

/// Keyboard IRQ (vector 33) body.
pub fn on_keyboard_irq() {
    let scancode = unsafe { Port::<u8>::new(DATA).read() };
    let event = KEYBOARD.lock().feed(scancode);
    if let Some(ev) = event {
        if ev.pressed {
            KEY_COUNT.fetch_add(1, Ordering::Relaxed);
            match ev.ascii {
                Some(a) if a.is_ascii_graphic() || a == b' ' => {
                    crate::serial_println!("[ITISYOU:INPUT] key={}", a as char);
                }
                Some(_) => {
                    crate::serial_println!("[ITISYOU:INPUT] key=<special> sc={:#x}", ev.scancode)
                }
                None => crate::serial_println!("[ITISYOU:INPUT] key=<none> sc={:#x}", ev.scancode),
            }
        }
        push(InputEvent::Key(ev));
    }
}

/// Mouse IRQ (vector 44) body.
pub fn on_mouse_irq() {
    let byte = unsafe { Port::<u8>::new(DATA).read() };
    let event = MOUSE.lock().feed(byte);
    if let Some(ev) = event {
        MOUSE_COUNT.fetch_add(1, Ordering::Relaxed);
        // Accumulate only — never lock the compositor/framebuffer here.
        ACCUM_DX.fetch_add(ev.dx, Ordering::Relaxed);
        ACCUM_DY.fetch_add(ev.dy, Ordering::Relaxed);
        crate::serial_println!(
            "[ITISYOU:INPUT] mouse dx={} dy={} l={} r={}",
            ev.dx,
            ev.dy,
            ev.left as u8,
            ev.right as u8
        );
        push(InputEvent::Mouse(ev));
    }
}

/// Feed a USB HID keyboard boot report into the unified input queue — the same
/// `InputEvent` stream PS/2 keys use, so the desktop/GUI is source-agnostic.
pub fn feed_usb_keyboard(report: &[u8]) {
    if let Some(a) = kernel_core::usb::hid_keyboard_ascii(report) {
        KEY_COUNT.fetch_add(1, Ordering::Relaxed);
        if a.is_ascii_graphic() || a == b' ' {
            crate::serial_println!("[ITISYOU:INPUT] key={} src=usb", a as char);
        } else {
            crate::serial_println!("[ITISYOU:INPUT] key=<special> src=usb");
        }
        push(InputEvent::Key(KeyEvent {
            scancode: 0,
            pressed: true,
            ascii: Some(a),
        }));
    }
}

/// Feed a USB HID mouse boot report into the unified input queue.
pub fn feed_usb_mouse(report: &[u8]) {
    if let Some(m) = kernel_core::usb::hid_mouse(report) {
        if m.dx == 0 && m.dy == 0 && !m.left && !m.right && !m.middle {
            return;
        }
        MOUSE_COUNT.fetch_add(1, Ordering::Relaxed);
        ACCUM_DX.fetch_add(m.dx as i32, Ordering::Relaxed);
        ACCUM_DY.fetch_add(m.dy as i32, Ordering::Relaxed);
        crate::serial_println!(
            "[ITISYOU:INPUT] mouse dx={} dy={} l={} src=usb",
            m.dx,
            m.dy,
            m.left as u8
        );
        push(InputEvent::Mouse(MouseEvent {
            dx: m.dx as i32,
            dy: m.dy as i32,
            left: m.left,
            right: m.right,
            middle: m.middle,
        }));
    }
}

/// Poll any attached USB HID device once and feed decoded events into the
/// unified queue. Safe to call when no USB device exists (no-op). This is how
/// the (polled) USB stack joins the same input path as the (IRQ) PS/2 stack.
pub fn pump_usb() {
    if !crate::device::uhci::has_hid() {
        return;
    }
    let mut report = [0u8; 8];
    let n = crate::device::uhci::poll_hid_report(&mut report);
    if n >= 3 {
        match crate::device::uhci::device_kind() {
            Some("mouse") => feed_usb_mouse(&report[..n]),
            _ => feed_usb_keyboard(&report[..n]),
        }
    }
}

/// Take and reset the accumulated mouse motion (desktop loop, main context).
pub fn take_mouse_motion() -> (i32, i32) {
    (
        ACCUM_DX.swap(0, Ordering::Relaxed),
        ACCUM_DY.swap(0, Ordering::Relaxed),
    )
}

fn push(ev: InputEvent) {
    let mut q = QUEUE.lock();
    if q.len() < QUEUE_MAX {
        q.push_back(ev);
    }
}

/// Drain one queued event (interrupt-safe for the desktop loop).
pub fn poll() -> Option<InputEvent> {
    without_interrupts(|| QUEUE.lock().pop_front())
}
