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
/// Title bar of the window with the input focus (V0.10).
const TITLE_FOCUS_BG: u32 = 0x00_2C_7F_73;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinError {
    NotFound,
    NotOwner,
    OutOfBounds,
    TooMany,
    BadSize,
    /// The window's backing buffer could not be allocated.
    NoMemory,
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
    /// Unread events for the owner (`gui_event`, V0.10).
    events: kernel_core::wm::EventRing,
}

struct Compositor {
    /// Bottom to top: the last one is drawn last and hit first.
    windows: Vec<Window>,
    next_id: u32,
    cursor_x: isize,
    cursor_y: isize,
    composites: u64,
    /// The window with the input focus (V0.10).
    focus: Option<u32>,
    /// The scene changed since the last composite (V0.10).
    dirty: bool,
}

static COMP: Mutex<Option<Compositor>> = Mutex::new(None);
const MAX_WINDOWS: usize = kernel_core::wm::MAX_WINDOWS;
const MAX_WIN_DIM: usize = 1024;
/// Windows one process may hold (V0.10); the kernel (pid 0) is not limited.
pub const MAX_WINDOWS_PER_OWNER: usize = 4;
/// All windows' backing buffers together, in pixels (8 MiB of the heap).
const MAX_TOTAL_PIXELS: usize = 2 * 1024 * 1024;

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
        focus: None,
        dirty: false,
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
    // V0.10: a process cannot take every window, nor the heap.
    if pid != 0 && c.windows.iter().filter(|w| w.owner_pid == pid).count() >= MAX_WINDOWS_PER_OWNER
    {
        return Err(WinError::TooMany);
    }
    let used: usize = c.windows.iter().map(|w| w.w * w.h).sum();
    if used + w * h > MAX_TOTAL_PIXELS {
        return Err(WinError::NoMemory);
    }
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(w * h)
        .map_err(|_| WinError::NoMemory)?;
    buffer.resize(w * h, 0x00_1C_1B_1B);
    let id = c.next_id;
    c.next_id += 1;
    c.windows.push(Window {
        id,
        owner_pid: pid,
        x,
        // Below the desktop's top bar, border included.
        y: y.max(TOP_BAR_H as isize + 1),
        w,
        h,
        title: title.to_string(),
        buffer,
        events: kernel_core::wm::EventRing::new(),
    });
    c.dirty = true;
    Ok(id)
}

/// Does `pid` own window `id` (the kernel owns every one)?
pub fn check_owner(pid: u64, id: u32) -> Result<(), WinError> {
    let mut guard = COMP.lock();
    let c = guard.as_mut().ok_or(WinError::NotFound)?;
    window_mut(c, id, pid).map(|_| ())
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

/// Remove all windows owned by a pid (on process exit). The focus moves to
/// nothing if it was on one of them, and the scene is marked for redraw.
pub fn remove_owned(pid: u64) {
    if let Some(c) = COMP.lock().as_mut() {
        let before = c.windows.len();
        c.windows.retain(|w| w.owner_pid != pid);
        if c.windows.len() != before {
            c.dirty = true;
            if let Some(f) = c.focus {
                if !c.windows.iter().any(|w| w.id == f) {
                    c.focus = None;
                }
            }
        }
    }
}

/// Was the scene changed (a window created or removed) since the last call?
pub fn take_dirty() -> bool {
    COMP.lock()
        .as_mut()
        .map(|c| core::mem::take(&mut c.dirty))
        .unwrap_or(false)
}

/// The window with the input focus.
pub fn focus() -> Option<u32> {
    COMP.lock().as_ref().and_then(|c| c.focus)
}

/// Give `id` the focus directly (the desktop's own window at start).
pub fn set_focus(id: u32) {
    if let Some(c) = COMP.lock().as_mut() {
        if c.windows.iter().any(|w| w.id == id) {
            c.focus = Some(id);
            c.dirty = true;
        }
    }
}

/// What a click on the desktop did (V0.10, DESK10-001).
pub struct Clicked {
    /// The focus moved: the new window and its owner.
    pub focused: Option<(u32, u64)>,
    /// Owners to wake: they have new events.
    pub wake: [u64; 2],
}

/// A left click at the cursor: click-to-focus with raise (`kernel_core::wm`),
/// a focus-out event for the old window's owner and a focus-in event for the
/// new one's (Ring 3 owners only), and a click event for the window clicked.
pub fn click_at_cursor() -> Clicked {
    use kernel_core::wm::{self, Event, Frame};
    let mut out = Clicked {
        focused: None,
        wake: [0; 2],
    };
    let mut guard = COMP.lock();
    let Some(c) = guard.as_mut() else {
        return out;
    };
    let frames: Vec<Frame> = c
        .windows
        .iter()
        .map(|w| Frame {
            id: w.id,
            x: (w.x - 1) as i32,
            y: (w.y - 1) as i32,
            w: (w.w + 2) as u32,
            h: (w.h + TITLE_BAR_H + 2) as u32,
        })
        .collect();
    let (cx, cy) = (c.cursor_x, c.cursor_y);
    let hit = wm::hit(&frames, cx as i32, cy as i32);
    if let Some(change) = wm::click(c.focus, hit) {
        if let Some(old) = change.out {
            if let Some(w) = c.windows.iter_mut().find(|w| w.id == old) {
                if w.owner_pid != 0 {
                    w.events.push(Event::FocusOut);
                    out.wake[0] = w.owner_pid;
                }
            }
        }
        c.focus = Some(change.into);
        // Raise: move the window to the top of the stack.
        if let Some(pos) = c.windows.iter().position(|w| w.id == change.into) {
            let w = c.windows.remove(pos);
            c.windows.push(w);
        }
        if let Some(w) = c.windows.last_mut() {
            if w.owner_pid != 0 {
                w.events.push(Event::FocusIn);
                out.wake[1] = w.owner_pid;
            }
            out.focused = Some((w.id, w.owner_pid));
        }
        c.dirty = true;
    }
    // The click itself, in content coordinates, for a Ring 3 window.
    if let Some(id) = hit {
        if let Some(w) = c.windows.iter_mut().find(|w| w.id == id) {
            let (lx, ly) = (cx - w.x, cy - w.y - TITLE_BAR_H as isize);
            if w.owner_pid != 0 && lx >= 0 && ly >= 0 && (lx as usize) < w.w && (ly as usize) < w.h
            {
                w.events.push(Event::Click {
                    x: lx as u16,
                    y: ly as u16,
                });
                out.wake[1] = w.owner_pid;
            }
        }
    }
    out
}

/// A key press: delivered to the focused window if a Ring 3 process owns it
/// (returns the window and its owner, to wake), otherwise left for the
/// desktop itself (`None`).
pub fn deliver_key(ascii: u8) -> Option<(u32, u64)> {
    let mut guard = COMP.lock();
    let c = guard.as_mut()?;
    let focus = c.focus?;
    let w = c.windows.iter_mut().find(|w| w.id == focus)?;
    if w.owner_pid == 0 {
        return None;
    }
    w.events.push(kernel_core::wm::Event::Key(ascii));
    Some((w.id, w.owner_pid))
}

/// The next event for window `id`, which `pid` must own (V0.10,
/// `gui_event`): its 8-byte record, or `None` when there is none yet.
pub fn pop_event(pid: u64, id: u32) -> Result<Option<[u8; kernel_core::wm::RECORD_LEN]>, WinError> {
    let mut guard = COMP.lock();
    let c = guard.as_mut().ok_or(WinError::NotFound)?;
    let win = window_mut(c, id, pid)?;
    Ok(win.events.pop().map(kernel_core::wm::encode))
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
        // Border + title bar (the focused window's stands out).
        gfx::fill_rect(
            win.x - 1,
            win.y - 1,
            win.w + 2,
            win.h + TITLE_BAR_H + 2,
            BORDER,
        );
        let title_bg = if c.focus == Some(win.id) {
            TITLE_FOCUS_BG
        } else {
            TITLE_BG
        };
        gfx::fill_rect(win.x, win.y, win.w, TITLE_BAR_H, title_bg);
        // The title is clipped to the window (V0.10), whole glyphs only.
        let fit = win.w.saturating_sub(4) / kernel_core::font::GLYPH_WIDTH;
        let title = match win.title.char_indices().nth(fit) {
            Some((end, _)) => &win.title[..end],
            None => &win.title,
        };
        gfx::draw_string(win.x + 2, win.y + 2, title, TITLE_FG, 1);
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
    c.dirty = false;
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
