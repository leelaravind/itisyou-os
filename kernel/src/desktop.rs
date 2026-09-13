//! Interactive desktop loop (V0.5, B180+).
//!
//! Renders a kernel-owned desktop window and reacts to real PS/2 keyboard and
//! mouse input (IRQ-driven) inside QEMU. Every received event is already
//! logged by the input layer as an `[ITISYOU:INPUT]` marker; this loop updates
//! the on-screen window + cursor and re-composites, so an automated test that
//! injects input through the QEMU monitor proves the full graphics+input path
//! end to end. ESC exits with a verification summary; a hard timeout fails
//! closed so a hang can never masquerade as success (plan §12.2).

use crate::gfx::compositor;
use crate::input::{self, InputEvent};
use crate::{interrupts, qemu, serial_println};
use core::fmt::Write;

const ESC_SCANCODE: u8 = 0x01;
const WIN_X: isize = 360;
const WIN_Y: isize = 250;
const WIN_W: usize = 560;
const WIN_H: usize = 210;
const BG: u32 = 0x0011_1622;
const FG: u32 = 0x00E5_E2E1;
const ACCENT: u32 = 0x0046_C8B4;

/// A fixed-capacity line buffer so per-frame text needs no heap allocation.
struct Line {
    buf: [u8; 96],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Line {
            buf: [0; 96],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct DeskState {
    keys: u64,
    mice: u64,
    last_key: u8,
    left: bool,
    cursor: (isize, isize),
}

/// Enter the desktop loop. Never returns: exits QEMU on ESC (success) or the
/// fail-closed timeout (failure).
pub fn run() -> ! {
    serial_println!("[ITISYOU:MODE] desktop");

    let win = match compositor::create_window(0, WIN_X, WIN_Y, WIN_W, WIN_H, "ITISYOU Desktop") {
        Ok(id) => id,
        Err(e) => {
            serial_println!("[ITISYOU:INFO] DESKTOP-FAILED err={e:?}");
            qemu::exit(qemu::ExitCode::Failed);
        }
    };

    let mut state = DeskState {
        cursor: compositor::cursor(),
        ..DeskState::default()
    };
    redraw(win, &state);
    compositor::composite();
    serial_println!("[ITISYOU:INFO] DESKTOP-READY win={win} w={WIN_W} h={WIN_H}");

    // Fail-closed guard: 60 s of real time with no exit is a failure, not a
    // pass. The harness timeout is a second, outer backstop.
    let deadline = interrupts::ticks() + interrupts::TICK_HZ * 60;

    loop {
        let mut dirty = false;

        // Poll USB HID into the unified input queue (no-op without a USB
        // device); PS/2 arrives via IRQ. The desktop consumes both identically.
        input::pump_usb();

        // Apply mouse motion accumulated by the IRQ handler (the IRQ never
        // touches the compositor lock; the desktop loop owns cursor updates).
        let (dx, dy) = input::take_mouse_motion();
        if dx != 0 || dy != 0 {
            compositor::move_cursor(dx, dy);
            state.cursor = compositor::cursor();
            dirty = true;
        }

        while let Some(ev) = input::poll() {
            match ev {
                InputEvent::Key(k) if k.pressed => {
                    // Exit on ESC from either source: PS/2 scancode 0x01 or a
                    // decoded ESC character (USB HID has no PS/2 scancode).
                    if k.scancode == ESC_SCANCODE || k.ascii == Some(0x1B) {
                        serial_println!(
                            "[ITISYOU:INFO] DESKTOP-INPUT-VERIFIED keys={} mouse={}",
                            state.keys,
                            state.mice
                        );
                        redraw(win, &state);
                        compositor::composite();
                        qemu::exit(qemu::ExitCode::Success);
                    }
                    state.keys += 1;
                    if let Some(a) = k.ascii {
                        state.last_key = a;
                    }
                    dirty = true;
                }
                InputEvent::Key(_) => {}
                InputEvent::Mouse(m) => {
                    state.mice += 1;
                    state.left = m.left;
                    dirty = true;
                }
            }
        }

        if dirty {
            redraw(win, &state);
            compositor::composite();
        }

        if interrupts::ticks() >= deadline {
            serial_println!(
                "[ITISYOU:INFO] DESKTOP-TIMEOUT keys={} mouse={}",
                state.keys,
                state.mice
            );
            qemu::exit(qemu::ExitCode::Failed);
        }

        // V0.10: the desktop's idle loop is a safe point, so background
        // processes keep running under the desktop.
        crate::sched::safe_point();
        // Sleep until the next interrupt (100 Hz timer or an input IRQ).
        x86_64::instructions::hlt();
    }
}

fn redraw(win: u32, s: &DeskState) {
    let _ = compositor::window_fill(0, win, 0, 0, WIN_W, WIN_H, BG);
    let _ = compositor::window_fill(0, win, 0, 0, WIN_W, 2, ACCENT);
    let _ = compositor::window_text(0, win, 12, 12, "ITISYOU OS desktop - live PS/2 input", FG);
    let _ = compositor::window_text(
        0,
        win,
        12,
        30,
        "keyboard + mouse via QEMU (IRQ1 / IRQ12)",
        FG,
    );

    let mut l = Line::new();
    let _ = write!(l, "keys pressed: {}", s.keys);
    let _ = compositor::window_text(0, win, 12, 66, l.as_str(), FG);

    let mut l = Line::new();
    let last = if s.last_key >= 0x20 && s.last_key < 0x7F {
        s.last_key as char
    } else {
        '-'
    };
    let _ = write!(l, "last key: {last}");
    let _ = compositor::window_text(0, win, 220, 66, l.as_str(), FG);

    let mut l = Line::new();
    let _ = write!(l, "mouse events: {}", s.mice);
    let _ = compositor::window_text(0, win, 12, 90, l.as_str(), FG);

    let mut l = Line::new();
    let _ = write!(
        l,
        "cursor: {},{}  button: {}",
        s.cursor.0,
        s.cursor.1,
        if s.left { "down" } else { "up" }
    );
    let _ = compositor::window_text(0, win, 12, 114, l.as_str(), FG);

    let _ = compositor::window_text(0, win, 12, 150, "press ESC to exit", ACCENT);
}
