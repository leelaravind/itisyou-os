//! A Ring 3 graphical application. It creates a window at a fixed position,
//! fills it with a distinctive colour, draws text, and presents — all through
//! validated GUI syscalls (no direct framebuffer access). The kernel selftest
//! reads back the composited pixel to prove a userspace process rendered.

#![no_std]
#![no_main]

use ulib::{exit, gui_create, gui_fill, gui_present, gui_text, write};

// Fixed geometry + colour the selftest asserts on.
const WIN_X: u32 = 100;
const WIN_Y: u32 = 100;
const WIN_W: u32 = 200;
const WIN_H: u32 = 120;
const FILL: u32 = 0x00FF_8800; // distinctive orange

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let win = gui_create(WIN_W, WIN_H, WIN_X, WIN_Y, "demo");
    if win == u64::MAX || win >= u64::MAX - 8 {
        write("RING3-GUI-CREATE-FAILED\n");
        exit(1);
    }
    // Fill the whole content area, then some accents + text.
    gui_fill(win, 0, 0, WIN_W, WIN_H, FILL);
    gui_fill(win, 8, 8, WIN_W - 16, 20, 0x0000_2A2A);
    gui_text(win, 12, 12, 0x00FF_FFFF, "RING3 GUI");
    gui_text(win, 12, 44, 0x0000_0000, "hello from userspace");
    gui_present(win);

    write("RING3-GUI-OK\n");
    exit(0)
}
