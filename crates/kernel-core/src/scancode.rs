//! PS/2 scancode set 1 decoding (V0.5). Pure state machine: bytes in, key
//! events out, with shift tracking. Host-unit-tested; the kernel's IRQ1
//! handler feeds it real scancodes.

/// A decoded key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub scancode: u8,
    pub pressed: bool,
    /// ASCII byte if this key maps to a printable character (with shift
    /// applied), else None.
    pub ascii: Option<u8>,
}

/// Scancode-set-1 keyboard decoder.
#[derive(Debug, Default)]
pub struct Keyboard {
    shift: bool,
    /// True after an 0xE0 prefix (extended keys); such keys carry no ASCII
    /// in V0.5 and are reported with ascii=None.
    extended: bool,
}

const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;

impl Keyboard {
    pub const fn new() -> Self {
        Keyboard {
            shift: false,
            extended: false,
        }
    }

    /// Feed one scancode byte; returns a key event when one completes.
    pub fn feed(&mut self, byte: u8) -> Option<KeyEvent> {
        if byte == 0xE0 {
            self.extended = true;
            return None;
        }
        let pressed = byte & 0x80 == 0;
        let code = byte & 0x7F;
        let was_extended = self.extended;
        self.extended = false;

        if code == SC_LSHIFT || code == SC_RSHIFT {
            self.shift = pressed;
            return Some(KeyEvent {
                scancode: code,
                pressed,
                ascii: None,
            });
        }

        let ascii = if was_extended {
            None
        } else {
            ascii_for(code, self.shift)
        };
        Some(KeyEvent {
            scancode: code,
            pressed,
            ascii,
        })
    }

    pub fn shift_held(&self) -> bool {
        self.shift
    }
}

/// Map a set-1 make code to ASCII (shifted or not), if printable.
fn ascii_for(code: u8, shift: bool) -> Option<u8> {
    let (lower, upper) = MAP
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, l, u)| (*l, *u))?;
    Some(if shift { upper } else { lower })
}

/// (scancode, unshifted, shifted) for the printable keys we support.
#[rustfmt::skip]
const MAP: &[(u8, u8, u8)] = &[
    (0x02, b'1', b'!'), (0x03, b'2', b'@'), (0x04, b'3', b'#'), (0x05, b'4', b'$'),
    (0x06, b'5', b'%'), (0x07, b'6', b'^'), (0x08, b'7', b'&'), (0x09, b'8', b'*'),
    (0x0A, b'9', b'('), (0x0B, b'0', b')'), (0x0C, b'-', b'_'), (0x0D, b'=', b'+'),
    (0x0F, b'\t', b'\t'),
    (0x10, b'q', b'Q'), (0x11, b'w', b'W'), (0x12, b'e', b'E'), (0x13, b'r', b'R'),
    (0x14, b't', b'T'), (0x15, b'y', b'Y'), (0x16, b'u', b'U'), (0x17, b'i', b'I'),
    (0x18, b'o', b'O'), (0x19, b'p', b'P'), (0x1A, b'[', b'{'), (0x1B, b']', b'}'),
    (0x1C, b'\n', b'\n'),
    (0x1E, b'a', b'A'), (0x1F, b's', b'S'), (0x20, b'd', b'D'), (0x21, b'f', b'F'),
    (0x22, b'g', b'G'), (0x23, b'h', b'H'), (0x24, b'j', b'J'), (0x25, b'k', b'K'),
    (0x26, b'l', b'L'), (0x27, b';', b':'), (0x28, b'\'', b'"'), (0x29, b'`', b'~'),
    (0x2B, b'\\', b'|'),
    (0x2C, b'z', b'Z'), (0x2D, b'x', b'X'), (0x2E, b'c', b'C'), (0x2F, b'v', b'V'),
    (0x30, b'b', b'B'), (0x31, b'n', b'N'), (0x32, b'm', b'M'), (0x33, b',', b'<'),
    (0x34, b'.', b'>'), (0x35, b'/', b'?'),
    (0x39, b' ', b' '),
    (0x0E, 0x08, 0x08), // backspace
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_letter_press_and_release() {
        let mut kb = Keyboard::new();
        assert_eq!(
            kb.feed(0x1E),
            Some(KeyEvent {
                scancode: 0x1E,
                pressed: true,
                ascii: Some(b'a')
            })
        );
        assert_eq!(
            kb.feed(0x9E),
            Some(KeyEvent {
                scancode: 0x1E,
                pressed: false,
                ascii: Some(b'a')
            })
        );
    }

    #[test]
    fn shift_produces_uppercase() {
        let mut kb = Keyboard::new();
        kb.feed(0x2A); // left shift down
        assert_eq!(kb.feed(0x1E).unwrap().ascii, Some(b'A'));
        kb.feed(0xAA); // left shift up
        assert_eq!(kb.feed(0x1E).unwrap().ascii, Some(b'a'));
    }

    #[test]
    fn digits_and_symbols() {
        let mut kb = Keyboard::new();
        assert_eq!(kb.feed(0x02).unwrap().ascii, Some(b'1'));
        kb.feed(0x2A);
        assert_eq!(kb.feed(0x02).unwrap().ascii, Some(b'!'));
    }

    #[test]
    fn extended_keys_have_no_ascii() {
        let mut kb = Keyboard::new();
        assert_eq!(kb.feed(0xE0), None); // prefix
        let ev = kb.feed(0x48).unwrap(); // "up arrow" make in extended
        assert_eq!(ev.ascii, None);
        assert!(ev.pressed);
    }

    #[test]
    fn unknown_scancode_yields_event_without_ascii() {
        let mut kb = Keyboard::new();
        let ev = kb.feed(0x59).unwrap();
        assert_eq!(ev.ascii, None);
    }

    #[test]
    fn enter_backspace_space() {
        let mut kb = Keyboard::new();
        assert_eq!(kb.feed(0x1C).unwrap().ascii, Some(b'\n'));
        assert_eq!(kb.feed(0x0E).unwrap().ascii, Some(0x08));
        assert_eq!(kb.feed(0x39).unwrap().ascii, Some(b' '));
    }
}
