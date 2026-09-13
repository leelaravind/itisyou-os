//! Console line discipline (V0.10, SHELL10-001): echo, backspace, the
//! 256-byte limit and discard-to-terminator — the editing rules the kernel
//! console has applied since V0.1, now host-tested and shared, so a Ring 3
//! program that owns the console's input gets exactly the same editing.
//!
//! * A printable ASCII byte (space to `~`) is appended and echoed, while the
//!   line has room; past the limit the line is marked overflowed and the
//!   byte is dropped without echo.
//! * Backspace (0x08) or DEL (0x7F) removes the last byte and echoes
//!   "back, space, back"; on an empty line it does nothing.
//! * CR or LF ends the line (echoed as a newline): a complete line, or an
//!   overflow — the whole line is then discarded, never truncated.
//! * Every other byte is ignored.
//!
//! After a line or an overflow the caller takes the line and calls
//! [`LineDisc::reset`].

/// Longest line, in bytes.
pub const MAX_LINE: usize = 256;

/// What to echo for one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    Nothing,
    /// The byte itself.
    Byte(u8),
    /// Back, space, back.
    Erase,
    Newline,
}

impl Echo {
    /// The bytes to write for this echo.
    pub fn bytes(self, scratch: &mut [u8; 3]) -> &[u8] {
        match self {
            Echo::Nothing => &[],
            Echo::Byte(b) => {
                scratch[0] = b;
                &scratch[..1]
            }
            Echo::Erase => b"\x08 \x08",
            Echo::Newline => b"\n",
        }
    }
}

/// Where the line stands after one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Still being typed.
    Pending,
    /// A complete line is ready ([`LineDisc::line`]).
    Line,
    /// The line exceeded [`MAX_LINE`] bytes and is discarded.
    Overflow,
}

/// One console line being typed.
pub struct LineDisc {
    buf: [u8; MAX_LINE],
    len: usize,
    overflow: bool,
}

impl Default for LineDisc {
    fn default() -> Self {
        Self::new()
    }
}

impl LineDisc {
    pub const fn new() -> Self {
        LineDisc {
            buf: [0; MAX_LINE],
            len: 0,
            overflow: false,
        }
    }

    /// Feed one input byte.
    pub fn feed(&mut self, byte: u8) -> (Echo, Event) {
        match byte {
            b'\r' | b'\n' => {
                let event = if self.overflow {
                    Event::Overflow
                } else {
                    Event::Line
                };
                (Echo::Newline, event)
            }
            0x08 | 0x7F => {
                if self.len > 0 {
                    self.len -= 1;
                    (Echo::Erase, Event::Pending)
                } else {
                    (Echo::Nothing, Event::Pending)
                }
            }
            b' '..=b'~' => {
                if self.len < MAX_LINE {
                    self.buf[self.len] = byte;
                    self.len += 1;
                    (Echo::Byte(byte), Event::Pending)
                } else {
                    self.overflow = true;
                    (Echo::Nothing, Event::Pending)
                }
            }
            _ => (Echo::Nothing, Event::Pending),
        }
    }

    /// The line typed so far (the complete line after [`Event::Line`]).
    pub fn line(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Start a new line.
    pub fn reset(&mut self) {
        self.len = 0;
        self.overflow = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_all(ld: &mut LineDisc, bytes: &[u8]) -> (std::vec::Vec<u8>, Event) {
        let mut echoed = std::vec::Vec::new();
        let mut last = Event::Pending;
        let mut scratch = [0u8; 3];
        for &b in bytes {
            let (echo, ev) = ld.feed(b);
            echoed.extend_from_slice(echo.bytes(&mut scratch));
            last = ev;
        }
        (echoed, last)
    }

    #[test]
    fn a_line_is_echoed_and_completed_by_cr_or_lf() {
        for term in *b"\r\n" {
            let mut ld = LineDisc::new();
            let mut input = b"echo hi".to_vec();
            input.push(term);
            let (echo, ev) = type_all(&mut ld, &input);
            assert_eq!(ev, Event::Line);
            assert_eq!(ld.line(), b"echo hi");
            assert_eq!(echo, b"echo hi\n");
        }
    }

    #[test]
    fn backspace_and_del_erase_one_byte_and_nothing_on_an_empty_line() {
        let mut ld = LineDisc::new();
        let (echo, _) = type_all(&mut ld, b"\x08ab\x08c\x7f\x7f\x7fd\n");
        assert_eq!(ld.line(), b"d");
        assert_eq!(echo, b"ab\x08 \x08c\x08 \x08\x08 \x08d\n");
    }

    #[test]
    fn control_bytes_are_ignored() {
        let mut ld = LineDisc::new();
        let (echo, ev) = type_all(&mut ld, b"a\x01\x1b\x80\xffb\n");
        assert_eq!(ev, Event::Line);
        assert_eq!(ld.line(), b"ab");
        assert_eq!(echo, b"ab\n");
    }

    #[test]
    fn exactly_the_limit_fits_and_one_more_discards_the_whole_line() {
        let mut ld = LineDisc::new();
        let mut input = [b'x'; MAX_LINE].to_vec();
        input.push(b'\n');
        assert_eq!(type_all(&mut ld, &input).1, Event::Line);
        assert_eq!(ld.line().len(), MAX_LINE);

        let mut ld = LineDisc::new();
        let mut input = [b'x'; MAX_LINE + 1].to_vec();
        input.push(b'\n');
        let (echo, ev) = type_all(&mut ld, &input);
        assert_eq!(ev, Event::Overflow);
        // The byte past the limit was not echoed.
        assert_eq!(echo.len(), MAX_LINE + 1);
    }

    #[test]
    fn reset_starts_a_fresh_line_after_an_overflow() {
        let mut ld = LineDisc::new();
        let mut input = [b'x'; MAX_LINE + 5].to_vec();
        input.push(b'\n');
        assert_eq!(type_all(&mut ld, &input).1, Event::Overflow);
        ld.reset();
        assert_eq!(type_all(&mut ld, b"ok\n").1, Event::Line);
        assert_eq!(ld.line(), b"ok");
    }

    #[test]
    fn an_empty_line_is_a_line() {
        let mut ld = LineDisc::new();
        assert_eq!(type_all(&mut ld, b"\r").1, Event::Line);
        assert!(ld.line().is_empty());
    }
}
