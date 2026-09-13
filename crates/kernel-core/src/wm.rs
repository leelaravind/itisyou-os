//! Desktop window management (V0.10, DESK10-001): z-order, hit testing,
//! click-to-focus, and the per-window event ring a Ring 3 app reads with
//! `gui_event`. Pure and allocation-free; the kernel's compositor holds the
//! windows and applies these decisions.
//!
//! * Windows are kept bottom-to-top; a new window goes on top, and focusing
//!   one raises it.
//! * A hit test walks top-down and hits the frame (border and title bar
//!   included), so a click on a window's title focuses it.
//! * A click that lands on a different window moves the focus: exactly one
//!   focus-out for the old window and one focus-in for the new one. A click
//!   on the desktop itself, or on the window that already has the focus,
//!   changes nothing.
//! * Each window's events wait in a fixed ring; when it is full the OLDEST
//!   event is dropped and counted, so an app that stops reading loses
//!   history, never the latest input.

/// Most windows the desktop tracks.
pub const MAX_WINDOWS: usize = 16;
/// Events one window can hold unread.
pub const EVENT_RING: usize = 16;
/// Size of one event record as `gui_event` returns it.
pub const RECORD_LEN: usize = 8;

/// A window's frame on screen (border and title bar included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Frame {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        let (x0, y0) = (i64::from(self.x), i64::from(self.y));
        let (px, py) = (i64::from(px), i64::from(py));
        px >= x0 && py >= y0 && px < x0 + i64::from(self.w) && py < y0 + i64::from(self.h)
    }
}

/// Top-down hit test over frames given bottom-to-top. The topmost frame
/// containing the point wins.
pub fn hit(frames_bottom_to_top: &[Frame], x: i32, y: i32) -> Option<u32> {
    frames_bottom_to_top
        .iter()
        .rev()
        .find(|f| f.contains(x, y))
        .map(|f| f.id)
}

/// What a click did to the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusChange {
    /// The window that lost the focus, if any.
    pub out: Option<u32>,
    /// The window that gained it.
    pub into: u32,
}

/// Click-to-focus: `hit` is what the click landed on, `focus` the current
/// focus. Returns the change, or `None` when the focus stays.
pub fn click(focus: Option<u32>, hit: Option<u32>) -> Option<FocusChange> {
    match hit {
        Some(id) if focus != Some(id) => Some(FocusChange {
            out: focus,
            into: id,
        }),
        _ => None,
    }
}

/// Move `id` to the top of a bottom-to-top list (no-op if absent).
pub fn raise(order: &mut [u32], id: u32) {
    if let Some(pos) = order.iter().position(|&w| w == id) {
        order[pos..].rotate_left(1);
    }
}

/// One event for a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    FocusIn,
    FocusOut,
    /// A key press, as ASCII.
    Key(u8),
    /// A left click at window-content coordinates.
    Click {
        x: u16,
        y: u16,
    },
}

const K_FOCUS_IN: u8 = 1;
const K_FOCUS_OUT: u8 = 2;
const K_KEY: u8 = 3;
const K_CLICK: u8 = 4;

/// Encode an event as its 8-byte record: `[kind, key, x:u16 LE, y:u16 LE,
/// 0, 0]`.
pub fn encode(ev: Event) -> [u8; RECORD_LEN] {
    let mut r = [0u8; RECORD_LEN];
    match ev {
        Event::FocusIn => r[0] = K_FOCUS_IN,
        Event::FocusOut => r[0] = K_FOCUS_OUT,
        Event::Key(k) => {
            r[0] = K_KEY;
            r[1] = k;
        }
        Event::Click { x, y } => {
            r[0] = K_CLICK;
            r[2..4].copy_from_slice(&x.to_le_bytes());
            r[4..6].copy_from_slice(&y.to_le_bytes());
        }
    }
    r
}

/// Decode a record (`None` for anything [`encode`] never produces).
pub fn decode(r: &[u8]) -> Option<Event> {
    if r.len() != RECORD_LEN || r[6] != 0 || r[7] != 0 {
        return None;
    }
    let x = u16::from_le_bytes([r[2], r[3]]);
    let y = u16::from_le_bytes([r[4], r[5]]);
    let quiet = r[1] == 0 && x == 0 && y == 0;
    match r[0] {
        K_FOCUS_IN if quiet => Some(Event::FocusIn),
        K_FOCUS_OUT if quiet => Some(Event::FocusOut),
        K_KEY if x == 0 && y == 0 => Some(Event::Key(r[1])),
        K_CLICK if r[1] == 0 => Some(Event::Click { x, y }),
        _ => None,
    }
}

/// A window's unread events (fixed capacity; drops the oldest when full).
#[derive(Debug, Clone, Copy)]
pub struct EventRing {
    buf: [Option<Event>; EVENT_RING],
    head: usize,
    len: usize,
    dropped: u64,
}

impl Default for EventRing {
    fn default() -> Self {
        Self::new()
    }
}

impl EventRing {
    pub const fn new() -> Self {
        EventRing {
            buf: [None; EVENT_RING],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub fn push(&mut self, ev: Event) {
        if self.len == EVENT_RING {
            // Full: the oldest goes.
            self.head = (self.head + 1) % EVENT_RING;
            self.len -= 1;
            self.dropped += 1;
        }
        let tail = (self.head + self.len) % EVENT_RING;
        self.buf[tail] = Some(ev);
        self.len += 1;
    }

    pub fn pop(&mut self) -> Option<Event> {
        if self.len == 0 {
            return None;
        }
        let ev = self.buf[self.head].take();
        self.head = (self.head + 1) % EVENT_RING;
        self.len -= 1;
        ev
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Events dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: u32, x: i32, y: i32, w: u32, h: u32) -> Frame {
        Frame { id, x, y, w, h }
    }

    #[test]
    fn the_topmost_window_under_the_point_is_hit() {
        let frames = [f(1, 0, 0, 100, 100), f(2, 50, 50, 100, 100)];
        assert_eq!(hit(&frames, 60, 60), Some(2), "overlap: the top one");
        assert_eq!(hit(&frames, 10, 10), Some(1));
        assert_eq!(hit(&frames, 149, 149), Some(2), "last pixel inside");
        assert_eq!(hit(&frames, 150, 150), None, "one past the edge");
        assert_eq!(hit(&frames, -1, 0), None);
        assert_eq!(hit(&[], 0, 0), None);
    }

    #[test]
    fn a_click_moves_the_focus_once_and_only_to_another_window() {
        assert_eq!(
            click(Some(1), Some(2)),
            Some(FocusChange {
                out: Some(1),
                into: 2
            })
        );
        assert_eq!(
            click(None, Some(2)),
            Some(FocusChange { out: None, into: 2 })
        );
        assert_eq!(click(Some(2), Some(2)), None, "already focused");
        assert_eq!(click(Some(2), None), None, "the desktop keeps the focus");
    }

    #[test]
    fn raising_moves_a_window_to_the_top_and_keeps_the_rest_in_order() {
        let mut order = [1, 2, 3, 4];
        raise(&mut order, 2);
        assert_eq!(order, [1, 3, 4, 2]);
        raise(&mut order, 2);
        assert_eq!(order, [1, 3, 4, 2]);
        raise(&mut order, 9);
        assert_eq!(order, [1, 3, 4, 2]);
    }

    #[test]
    fn records_round_trip_and_junk_is_refused() {
        for ev in [
            Event::FocusIn,
            Event::FocusOut,
            Event::Key(b'a'),
            Event::Key(0x1B),
            Event::Click { x: 0, y: 0 },
            Event::Click { x: 199, y: 65535 },
        ] {
            assert_eq!(decode(&encode(ev)), Some(ev));
        }
        assert_eq!(decode(&[0; 8]), None, "kind 0");
        assert_eq!(decode(&[9, 0, 0, 0, 0, 0, 0, 0]), None, "unknown kind");
        assert_eq!(
            decode(&[1, 7, 0, 0, 0, 0, 0, 0]),
            None,
            "focus with a payload"
        );
        assert_eq!(decode(&[3, b'a', 0, 0, 0, 0, 0, 1]), None, "padding set");
        assert_eq!(decode(&[1, 0, 0, 0, 0, 0, 0]), None, "short");
    }

    #[test]
    fn the_ring_is_fifo_and_drops_the_oldest_when_full() {
        let mut r = EventRing::new();
        assert!(r.is_empty() && r.pop().is_none());
        for k in 0..EVENT_RING as u8 {
            r.push(Event::Key(k));
        }
        assert_eq!(r.len(), EVENT_RING);
        assert_eq!(r.dropped(), 0);
        r.push(Event::Key(100));
        r.push(Event::Key(101));
        assert_eq!(r.dropped(), 2);
        assert_eq!(r.len(), EVENT_RING);
        assert_eq!(r.pop(), Some(Event::Key(2)), "0 and 1 were dropped");
        let mut last = None;
        while let Some(ev) = r.pop() {
            last = Some(ev);
        }
        assert_eq!(last, Some(Event::Key(101)), "the newest is kept");
    }
}
