//! `gui-echo` — a persistent Ring 3 desktop app (V0.10, DESK10-001).
//!
//! Opens a window, then waits for its events forever (`gui_event`): it shows
//! whether it has the focus and echoes every key it receives, both on screen
//! and on the serial log (`GUIECHO-FOCUS-IN`, `GUIECHO-FOCUS-OUT`,
//! `GUIECHO-KEY <c>`, `GUIECHO-CLICK x=… y=…`). The records are decoded here
//! by hand, so the app checks the ABI independently of the kernel's codec.

#![no_std]
#![no_main]

use ulib::{
    exit, gui_create, gui_event, gui_fill, gui_present, gui_text, write, write_u64, GUI_EV_CLICK,
    GUI_EV_FOCUS_IN, GUI_EV_FOCUS_OUT, GUI_EV_KEY,
};

const X: u32 = 100;
const Y: u32 = 100;
const W: u32 = 200;
const H: u32 = 120;
const BG: u32 = 0x0020_2830;
const FOCUS_BG: u32 = 0x0024_4A44;
const FG: u32 = 0x00E5_E2E1;

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

fn draw(win: u64, focused: bool, typed: &[u8]) {
    gui_fill(win, 0, 0, W, H, if focused { FOCUS_BG } else { BG });
    gui_text(win, 8, 8, FG, "gui-echo");
    gui_text(
        win,
        8,
        28,
        FG,
        if focused { "focus: yes" } else { "focus: no" },
    );
    if let Ok(s) = core::str::from_utf8(typed) {
        gui_text(win, 8, 52, FG, s);
    }
    gui_present(win);
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let win = gui_create(W, H, X, Y, "gui-echo");
    if is_err(win) {
        write("GUIECHO-FAILED step=create\n");
        exit(1)
    }
    let mut typed = [0u8; 24];
    let mut n = 0usize;
    let mut focused = false;
    draw(win, focused, &typed[..n]);
    write("GUIECHO-READY win=");
    write_u64(win);
    write("\n");
    loop {
        let mut rec = [0u8; 8];
        let r = gui_event(win, &mut rec);
        if r != 8 {
            write("GUIECHO-EVENT-ERROR err=");
            write_u64(u64::MAX - r);
            write("\n");
            exit(1)
        }
        match rec[0] {
            GUI_EV_FOCUS_IN => {
                focused = true;
                write("GUIECHO-FOCUS-IN\n");
            }
            GUI_EV_FOCUS_OUT => {
                focused = false;
                write("GUIECHO-FOCUS-OUT\n");
            }
            GUI_EV_KEY => {
                let k = rec[1];
                write("GUIECHO-KEY ");
                if k.is_ascii_graphic() {
                    let b = [k];
                    if let Ok(s) = core::str::from_utf8(&b) {
                        write(s);
                    }
                } else {
                    write("?");
                }
                write("\n");
                if n == typed.len() {
                    n = 0;
                }
                typed[n] = if k.is_ascii_graphic() { k } else { b'?' };
                n += 1;
            }
            GUI_EV_CLICK => {
                write("GUIECHO-CLICK x=");
                write_u64(u64::from(u16::from_le_bytes([rec[2], rec[3]])));
                write(" y=");
                write_u64(u64::from(u16::from_le_bytes([rec[4], rec[5]])));
                write("\n");
            }
            _ => {
                write("GUIECHO-EVENT-UNKNOWN\n");
            }
        }
        draw(win, focused, &typed[..n]);
    }
}
