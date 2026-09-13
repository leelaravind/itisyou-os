//! PS/2 mouse packet decoding (V0.5). A 3-byte-packet state machine with
//! resynchronization on malformed packets (the first byte must have the
//! "always 1" bit set). Pure + host-tested with adversarial input; the
//! kernel's IRQ12 handler feeds it real bytes.

/// A completed mouse movement/button event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    /// Relative X movement (right positive).
    pub dx: i32,
    /// Relative Y movement (up positive in PS/2; the compositor flips it).
    pub dy: i32,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

/// 3-byte PS/2 mouse packet decoder.
#[derive(Debug, Default)]
pub struct Mouse {
    bytes: [u8; 3],
    index: usize,
    /// Count of discarded bytes during resync (diagnostic).
    pub resyncs: u32,
}

const ALWAYS_ONE: u8 = 1 << 3;
const X_SIGN: u8 = 1 << 4;
const Y_SIGN: u8 = 1 << 5;
const X_OVF: u8 = 1 << 6;
const Y_OVF: u8 = 1 << 7;

impl Mouse {
    pub const fn new() -> Self {
        Mouse {
            bytes: [0; 3],
            index: 0,
            resyncs: 0,
        }
    }

    /// Drop any partially received packet (V0.10): the next byte must start a
    /// new one. The kernel calls this after initialization — the controller
    /// can deliver a command ACK (0xFA, which happens to carry the always-1
    /// bit) to the interrupt handler, and without a reset that byte framed the
    /// first real packet off by one — and whenever a packet's bytes are not
    /// back to back, which a working device never does.
    pub fn resync(&mut self) {
        if self.index != 0 {
            self.resyncs += 1;
            self.index = 0;
        }
    }

    /// Feed one byte; returns an event when a valid 3-byte packet completes.
    pub fn feed(&mut self, byte: u8) -> Option<MouseEvent> {
        if self.index == 0 {
            // The first byte MUST have the always-1 bit; otherwise the stream
            // is out of sync — drop the byte and stay at index 0.
            if byte & ALWAYS_ONE == 0 {
                self.resyncs += 1;
                return None;
            }
        }
        self.bytes[self.index] = byte;
        self.index += 1;
        if self.index < 3 {
            return None;
        }
        self.index = 0;

        let flags = self.bytes[0];
        // Overflow bytes are unreliable; treat the movement as zero.
        let dx = if flags & X_OVF != 0 {
            0
        } else {
            sign_extend(self.bytes[1], flags & X_SIGN != 0)
        };
        let dy = if flags & Y_OVF != 0 {
            0
        } else {
            sign_extend(self.bytes[2], flags & Y_SIGN != 0)
        };
        Some(MouseEvent {
            dx,
            dy,
            left: flags & 0x01 != 0,
            right: flags & 0x02 != 0,
            middle: flags & 0x04 != 0,
        })
    }
}

/// Sign-extend a 9-bit PS/2 movement value (8 data bits + sign flag).
fn sign_extend(byte: u8, sign: bool) -> i32 {
    if sign {
        (byte as i32) - 256
    } else {
        byte as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_simple_movement() {
        let mut m = Mouse::new();
        assert_eq!(m.feed(ALWAYS_ONE), None); // flags: no buttons, +x +y
        assert_eq!(m.feed(5), None); // dx = +5
        assert_eq!(
            m.feed(3),
            Some(MouseEvent {
                dx: 5,
                dy: 3,
                left: false,
                right: false,
                middle: false
            })
        );
    }

    #[test]
    fn decodes_buttons_and_negative_movement() {
        let mut m = Mouse::new();
        let flags = ALWAYS_ONE | 0x01 | X_SIGN | Y_SIGN; // left down, x/y negative
        m.feed(flags);
        m.feed(0xFB); // -5 with sign
        let ev = m.feed(0xFE).unwrap(); // -2
        assert_eq!(ev.dx, -5);
        assert_eq!(ev.dy, -2);
        assert!(ev.left && !ev.right);
    }

    #[test]
    fn resyncs_on_malformed_first_byte() {
        let mut m = Mouse::new();
        // Garbage first bytes without the always-1 bit are dropped.
        assert_eq!(m.feed(0x00), None);
        assert_eq!(m.feed(0x04), None);
        assert_eq!(m.resyncs, 2);
        // Then a valid packet still decodes cleanly.
        m.feed(ALWAYS_ONE);
        m.feed(1);
        assert_eq!(m.feed(1).unwrap().dx, 1);
    }

    #[test]
    fn a_stray_ack_before_the_first_packet_is_dropped_by_resync() {
        // What V0.9 did: an ACK (0xFA) left in the decoder framed the first
        // real packet (flags 0x28, dx 0x28, dy 0xE7) off by one.
        let mut stale = Mouse::new();
        assert_eq!(stale.feed(0xFA), None);
        assert_eq!(stale.feed(0x28), None);
        let wrong = stale.feed(0x28).unwrap();
        assert!(wrong.right && wrong.dx == 0 && wrong.dy == 0, "garbage");

        let mut m = Mouse::new();
        assert_eq!(m.feed(0xFA), None);
        m.resync();
        assert_eq!(m.resyncs, 1);
        assert_eq!(m.feed(0x28), None);
        assert_eq!(m.feed(0x28), None);
        let ev = m.feed(0xE7).unwrap();
        assert_eq!((ev.dx, ev.dy), (40, -25));
        assert!(!ev.left && !ev.right);
        // A resync with nothing pending changes nothing.
        m.resync();
        assert_eq!(m.resyncs, 1);
    }

    #[test]
    fn overflow_movement_is_clamped_to_zero() {
        let mut m = Mouse::new();
        m.feed(ALWAYS_ONE | X_OVF | Y_OVF);
        m.feed(0x7F);
        let ev = m.feed(0x7F).unwrap();
        assert_eq!(ev.dx, 0);
        assert_eq!(ev.dy, 0);
    }
}
