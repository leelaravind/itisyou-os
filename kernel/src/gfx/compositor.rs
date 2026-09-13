//! Compositor / minimal window system (V0.5, B180).
//!
//! Owns a list of windows (each with its own RGB backing buffer), a desktop
//! wallpaper, top-bar chrome, and a cursor. `composite()` renders desktop →
//! windows (back-to-front) → cursor into the graphics back buffer, then
//! presents. Windows are owned by a pid; GUI syscalls validate ownership and
//! bounds, so one process can never draw into another's window or outside it.

use crate::gfx;
use crate::sync::Mutex;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub const TITLE_BAR_H: usize = 12;
pub const TOP_BAR_H: usize = 16;
const BORDER: u32 = 0x00_2A_2A_2A;
const WALLPAPER: u32 = 0x00_0C_0C_14;
const TOPBAR_BG: u32 = 0x00_18_18_20;
const TITLE_BG: u32 = 0x00_38_49_55;
const TITLE_FG: u32 = 0x00_E5_E2_E1;
const CURSOR_FG: u32 = 0x00_FF_FF_FF;
const CURSOR_OUTLINE: u32 = 0x00_00_00_00;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinError {
    NotFound,
    NotOwner,
    OutOfBounds,
    TooMany,
    BadSize,
}

pub struct Window {
    pub id: u32,
    pub owner_pid: u64,
    pub x: isize,
    pub y: isize,
    pub w: usize,
    pub h: usize,
    pub title: String,
    /// Content backing store (w*h, 0x00RRGGBB).
    buffer: Vec<u32>,
}

struct Compositor {
    windows: Vec<Window>,
    next_id: u32,
    cursor_x: isize,
    cursor_y: isize,
    composites: u64,
}

static COMP: Mutex<Option<Compositor>> = Mutex::new(None);
const MAX_WINDOWS: usize = 16;
const MAX_WIN_DIM: usize = 1024;

pub fn init() {
    let (cx, cy) = gfx::info()
        .map(|i| (i.width as isize / 2, i.height as isize / 2))
        .unwrap_or((0, 0));
    *COMP.lock() = Some(Compositor {
        windows: Vec::new(),
        next_id: 1,
        cursor_x: cx,
        cursor_y: cy,
        composites: 0,
    });
}

/// Create a window owned by `pid`. Returns the window id.
pub fn create_window(
    pid: u64,
    x: isize,
    y: isize,
    w: usize,
    h: usize,
    title: &str,
) -> Result<u32, WinError> {
    if w == 0 || h == 0 || w > MAX_WIN_DIM || h > MAX_WIN_DIM {
        return Err(WinError::BadSize);
    }
    let mut guard = COMP.lock();
    let c = guard.as_mut().ok_or(WinError::NotFound)?;
    if c.windows.len() >= MAX_WINDOWS {
        return Err(WinError::TooMany);
    }
    let id = c.next_id;
    c.next_id += 1;
    c.windows.push(Window {
        id,
        owner_pid: pid,
        x,
        y,
        w,
        h,
        title: title.to_string(),
        buffer: vec![0x00_1C_1B_1B; w * h],
    });
    Ok(id)
}

fn window_mut(c: &mut Compositor, id: u32, pid: u64) -> Result<&mut Window, WinError> {
    let win = c
        .windows
        .iter_mut()
        .find(|w| w.id == id)
        .ok_or(WinError::NotFound)?;
    // pid 0 = kernel/compositor itself, may touch any window.
    if pid != 0 && win.owner_pid != pid {
        return Err(WinError::NotOwner);
    }
    Ok(win)
}

/// Fill a rectangle inside a window's content area (window-local coords).
/// Rejects a rectangle that would exceed the window bounds.
pub fn window_fill(
    pid: u64,
    id: u32,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
) -> Result<(), WinError> {
    let mut guard = COMP.lock();
    let c = guard.as_mut().ok_or(WinError::NotFound)?;
    let win = window_mut(c, id, pid)?;
    let x1 = x.checked_add(w).ok_or(WinError::OutOfBounds)?;
    let y1 = y.checked_add(h).ok_or(WinError::OutOfBounds)?;
    if x1 > win.w || y1 > win.h {
        return Err(WinError::OutOfBounds);
    }
    let color = color & 0x00FF_FFFF;
    for py in y..y1 {
        for px in x..x1 {
            win.buffer[py * win.w + px] = color;
        }
    }
    Ok(())
}

/// Draw text inside a window (window-local coords), clipped to the window.
pub fn window_text(
    pid: u64,
    id: u32,
    x: usize,
    y: usize,
    text: &str,
    color: u32,
) -> Result<(), WinError> {
    use kernel_core::font;
    let mut guard = COMP.lock();
    let c = guard.as_mut().ok_or(WinError::NotFound)?;
    let win = window_mut(c, id, pid)?;
    let color = color & 0x00FF_FFFF;
    let mut cx = x;
    for &ch in text.as_bytes() {
        for row in 0..font::GLYPH_HEIGHT {
            for col in 0..font::GLYPH_WIDTH {
                if font::pixel(ch, col, row) {
                    let px = cx + col;
                    let py = y + row;
                    if px < win.w && py < win.h {
                        win.buffer[py * win.w + px] = color;
                    }
                }
            }
        }
        cx += font::GLYPH_WIDTH;
    }
    Ok(())
}

/// Read a window content pixel (for verification).
pub fn window_pixel(id: u32, x: usize, y: usize) -> Option<u32> {
    let guard = COMP.lock();
    let c = guard.as_ref()?;
    let win = c.windows.iter().find(|w| w.id == id)?;
    (x < win.w && y < win.h).then(|| win.buffer[y * win.w + x])
}

/// Remove all windows owned by a pid (on process exit).
pub fn remove_owned(pid: u64) {
    if let Some(c) = COMP.lock().as_mut() {
        c.windows.retain(|w| w.owner_pid != pid);
    }
}

pub fn window_count() -> usize {
    COMP.lock().as_ref().map(|c| c.windows.len()).unwrap_or(0)
}

pub fn composites() -> u64 {
    COMP.lock().as_ref().map(|c| c.composites).unwrap_or(0)
}

pub fn cursor() -> (isize, isize) {
    COMP.lock()
        .as_ref()
        .map(|c| (c.cursor_x, c.cursor_y))
        .unwrap_or((0, 0))
}

/// Move the cursor by a relative delta, clamped to the screen.
pub fn move_cursor(dx: i32, dy: i32) {
    if let (Some(info), Some(c)) = (gfx::info(), COMP.lock().as_mut()) {
        c.cursor_x = (c.cursor_x + dx as isize).clamp(0, info.width as isize - 1);
        c.cursor_y = (c.cursor_y + dy as isize).clamp(0, info.height as isize - 1);
    }
}

/// Render the full scene to the graphics back buffer and present it.
pub fn composite() {
    let Some(info) = gfx::info() else {
        return;
    };
    let mut guard = COMP.lock();
    let Some(c) = guard.as_mut() else {
        return;
    };

    // Desktop wallpaper + top bar chrome.
    gfx::fill_rect(0, 0, info.width, info.height, WALLPAPER);
    gfx::fill_rect(0, 0, info.width, TOP_BAR_H, TOPBAR_BG);
    gfx::draw_string(6, 4, "ITISYOU OS", TITLE_FG, 1);

    // Windows, back to front.
    for win in &c.windows {
        // Border + title bar.
        gfx::fill_rect(
            win.x - 1,
            win.y - 1,
            win.w + 2,
            win.h + TITLE_BAR_H + 2,
            BORDER,
        );
        gfx::fill_rect(win.x, win.y, win.w, TITLE_BAR_H, TITLE_BG);
        gfx::draw_string(win.x + 2, win.y + 2, &win.title, TITLE_FG, 1);
        // Content.
        gfx::blit(
            win.x,
            win.y + TITLE_BAR_H as isize,
            win.w,
            win.h,
            &win.buffer,
        );
    }

    // Cursor (a small arrow: a filled triangle drawn as stepped rows).
    draw_cursor(c.cursor_x, c.cursor_y);

    c.composites += 1;
    drop(guard);
    gfx::present();
}

fn draw_cursor(x: isize, y: isize) {
    // 8-row arrow; each row is (offset, length) of the filled span. A 1px
    // black underlay makes it visible on any background.
    const ROWS: [(isize, usize); 8] = [
        (0, 1),
        (0, 2),
        (0, 3),
        (0, 4),
        (0, 5),
        (0, 3),
        (2, 2),
        (3, 2),
    ];
    for (row, (off, len)) in ROWS.iter().enumerate() {
        let py = y + row as isize;
        gfx::fill_rect(x + off - 1, py, len + 2, 1, CURSOR_OUTLINE);
    }
    for (row, (off, len)) in ROWS.iter().enumerate() {
        let py = y + row as isize;
        gfx::fill_rect(x + off, py, *len, 1, CURSOR_FG);
    }
}
