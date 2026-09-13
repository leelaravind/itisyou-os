//! Framebuffer graphics (V0.5, B170).
//!
//! The bootloader hands the kernel a linear framebuffer; this module wraps it
//! with a Vec<u32> back buffer (0x00RRGGBB) that all drawing targets. A
//! single `present()` converts the back buffer to the framebuffer's real
//! pixel format in one pass — flicker-free and giving a deterministic
//! in-memory image the selftests can hash / pixel-probe.

pub mod compositor;

use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;
use bootloader_api::info::{FrameBuffer, PixelFormat};

/// Framebuffer geometry (queried from the bootloader; never hardcoded).
#[derive(Debug, Clone, Copy)]
pub struct FbInfo {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub bgr: bool,
}

struct Gfx {
    fb_ptr: *mut u8,
    fb_len: usize,
    info: FbInfo,
    back: Vec<u32>,
}

// Single CPU; the framebuffer pointer is a fixed kernel mapping.
unsafe impl Send for Gfx {}

static GFX: Mutex<Option<Gfx>> = Mutex::new(None);

/// Initialize graphics from the bootloader framebuffer. Returns the geometry,
/// or None if no framebuffer was provided.
pub fn init(fb: &mut FrameBuffer) -> Option<FbInfo> {
    let fbi = fb.info();
    let info = FbInfo {
        width: fbi.width,
        height: fbi.height,
        stride: fbi.stride,
        bytes_per_pixel: fbi.bytes_per_pixel,
        bgr: matches!(fbi.pixel_format, PixelFormat::Bgr),
    };
    let buffer = fb.buffer_mut();
    let fb_ptr = buffer.as_mut_ptr();
    let fb_len = buffer.len();
    let back = vec![0u32; info.width * info.height];
    *GFX.lock() = Some(Gfx {
        fb_ptr,
        fb_len,
        info,
        back,
    });
    Some(info)
}

pub fn available() -> bool {
    GFX.lock().is_some()
}

pub fn info() -> Option<FbInfo> {
    GFX.lock().as_ref().map(|g| g.info)
}

/// Run `f` with the back buffer + geometry for direct drawing.
fn with<R>(f: impl FnOnce(&mut Vec<u32>, FbInfo) -> R) -> Option<R> {
    let mut guard = GFX.lock();
    let g = guard.as_mut()?;
    let info = g.info;
    Some(f(&mut g.back, info))
}

/// Set a back-buffer pixel (bounds-checked, no-op if out of range).
pub fn put_pixel(x: usize, y: usize, color: u32) {
    with(|back, info| {
        if x < info.width && y < info.height {
            back[y * info.width + x] = color & 0x00FF_FFFF;
        }
    });
}

/// Read a back-buffer pixel (for verification). None if out of range.
pub fn get_pixel(x: usize, y: usize) -> Option<u32> {
    with(|back, info| {
        if x < info.width && y < info.height {
            Some(back[y * info.width + x])
        } else {
            None
        }
    })
    .flatten()
}

/// Fill a clipped rectangle in the back buffer.
pub fn fill_rect(x: isize, y: isize, w: usize, h: usize, color: u32) {
    with(|back, info| {
        let color = color & 0x00FF_FFFF;
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w as isize).max(0) as usize).min(info.width);
        let y1 = ((y + h as isize).max(0) as usize).min(info.height);
        for py in y0..y1 {
            let row = py * info.width;
            for px in x0..x1 {
                back[row + px] = color;
            }
        }
    });
}

/// Draw an 8x8 glyph at (x,y), `scale`x, foreground on transparent (only set
/// pixels are drawn). Clipped.
pub fn draw_char(x: isize, y: isize, c: u8, color: u32, scale: usize) {
    use kernel_core::font;
    with(|back, info| {
        let color = color & 0x00FF_FFFF;
        for row in 0..font::GLYPH_HEIGHT {
            for col in 0..font::GLYPH_WIDTH {
                if !font::pixel(c, col, row) {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = x + (col * scale + sx) as isize;
                        let py = y + (row * scale + sy) as isize;
                        if px >= 0
                            && py >= 0
                            && (px as usize) < info.width
                            && (py as usize) < info.height
                        {
                            back[py as usize * info.width + px as usize] = color;
                        }
                    }
                }
            }
        }
    });
}

/// Draw a string left-to-right (no wrapping). Returns the x after the last
/// glyph.
pub fn draw_string(x: isize, y: isize, s: &str, color: u32, scale: usize) -> isize {
    let mut cx = x;
    for &b in s.as_bytes() {
        draw_char(cx, y, b, color, scale);
        cx += (kernel_core::font::GLYPH_WIDTH * scale) as isize;
    }
    cx
}

/// Blit a caller-provided RGB buffer (`src`, `sw`×`sh`) into the back buffer
/// at (x,y), clipped. Used to composite window backing stores.
pub fn blit(x: isize, y: isize, sw: usize, sh: usize, src: &[u32]) {
    with(|back, info| {
        for sy in 0..sh {
            let dy = y + sy as isize;
            if dy < 0 || dy as usize >= info.height {
                continue;
            }
            for sx in 0..sw {
                let dx = x + sx as isize;
                if dx < 0 || dx as usize >= info.width {
                    continue;
                }
                back[dy as usize * info.width + dx as usize] = src[sy * sw + sx] & 0x00FF_FFFF;
            }
        }
    });
}

/// CRC-32 of a clipped back-buffer region — deterministic evidence for tests.
pub fn hash_region(x: usize, y: usize, w: usize, h: usize) -> u32 {
    with(|back, info| {
        let mut crc = 0xFFFF_FFFFu32;
        let x1 = (x + w).min(info.width);
        let y1 = (y + h).min(info.height);
        for py in y.min(info.height)..y1 {
            for px in x.min(info.width)..x1 {
                for byte in back[py * info.width + px].to_le_bytes() {
                    crc ^= byte as u32;
                    for _ in 0..8 {
                        let mask = (crc & 1).wrapping_neg();
                        crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
                    }
                }
            }
        }
        !crc
    })
    .unwrap_or(0)
}

/// Convert the back buffer to the framebuffer's real format and display it.
pub fn present() {
    let mut guard = GFX.lock();
    let Some(g) = guard.as_mut() else {
        return;
    };
    let info = g.info;
    let bpp = info.bytes_per_pixel;
    // SAFETY: fb_ptr/fb_len describe the bootloader framebuffer mapping,
    // valid for the kernel's lifetime; single CPU serializes access.
    let fb = unsafe { core::slice::from_raw_parts_mut(g.fb_ptr, g.fb_len) };
    for y in 0..info.height {
        let src_row = y * info.width;
        let dst_row = y * info.stride * bpp;
        for x in 0..info.width {
            let px = g.back[src_row + x];
            let r = ((px >> 16) & 0xFF) as u8;
            let gc = ((px >> 8) & 0xFF) as u8;
            let b = (px & 0xFF) as u8;
            let off = dst_row + x * bpp;
            if off + 2 < fb.len() {
                if info.bgr {
                    fb[off] = b;
                    fb[off + 1] = gc;
                    fb[off + 2] = r;
                } else {
                    fb[off] = r;
                    fb[off + 1] = gc;
                    fb[off + 2] = b;
                }
            }
        }
    }
}
