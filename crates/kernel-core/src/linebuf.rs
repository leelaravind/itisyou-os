//! Line-atomic process output (V0.10, OUT10-001).
//!
//! A program that prints one line in several `write` calls — a marker built
//! from a prefix, a number and a newline — must never have another process's
//! output, or the kernel's, spliced into the middle of it: the serial log is
//! the test evidence, and it is read line by line. Once background processes
//! run between a foreground program's syscalls (V0.10's always-on
//! scheduling), that splice stops being theoretical.
//!
//! [`LineBuf`] holds a process's partial line. [`LineBuf::push`] emits every
//! complete line (newline included) and, when a line outgrows the buffer, the
//! full buffer as one chunk; [`LineBuf::flush`] emits what is left plus a
//! newline, for a process that exits mid-line. Nothing allocates.
//!
//! [`neutralize_markers`] rewrites the kernel's evidence prefix `[ITISYOU:`
//! wherever it appears in process output, so no program can print a line
//! that looks like a kernel marker.

/// The prefix every kernel evidence marker starts with.
pub const KERNEL_MARKER: &[u8] = b"[ITISYOU:";
/// What that prefix becomes in process output: same length, visibly not
/// kernel-issued.
pub const RING3_MARKER: &[u8] = b"[RING3-U:";

const _: () = assert!(KERNEL_MARKER.len() == RING3_MARKER.len());

/// A fixed-capacity buffer for one process's partial line.
#[derive(Debug, Clone)]
pub struct LineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Default for LineBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> LineBuf<N> {
    pub const fn new() -> Self {
        LineBuf {
            buf: [0; N],
            len: 0,
        }
    }

    /// Bytes of partial line currently held.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append `bytes`. `emit` receives every complete line (ending in
    /// `\n`, which is included) and, whenever the buffer fills without a
    /// newline, the full buffer as one chunk.
    pub fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8])) {
        for &b in bytes {
            if self.len == N {
                emit(&self.buf[..self.len]);
                self.len = 0;
            }
            self.buf[self.len] = b;
            self.len += 1;
            if b == b'\n' {
                emit(&self.buf[..self.len]);
                self.len = 0;
            }
        }
    }

    /// Emit any pending partial line followed by a newline, and empty the
    /// buffer. Does nothing when there is no partial line.
    pub fn flush(&mut self, mut emit: impl FnMut(&[u8])) {
        if self.len == 0 {
            return;
        }
        if self.len < N {
            self.buf[self.len] = b'\n';
            emit(&self.buf[..self.len + 1]);
        } else {
            emit(&self.buf[..self.len]);
            emit(b"\n");
        }
        self.len = 0;
    }
}

/// Rewrite every occurrence of [`KERNEL_MARKER`] in `line` to
/// [`RING3_MARKER`], in place. Returns how many were rewritten.
pub fn neutralize_markers(line: &mut [u8]) -> usize {
    let n = KERNEL_MARKER.len();
    let mut count = 0;
    let mut i = 0;
    while i + n <= line.len() {
        if &line[i..i + n] == KERNEL_MARKER {
            line[i..i + n].copy_from_slice(RING3_MARKER);
            count += 1;
            i += n;
        } else {
            i += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect<const N: usize>(b: &mut LineBuf<N>, input: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for chunk in input {
            b.push(chunk, |l| out.push(l.to_vec()));
        }
        out
    }

    #[test]
    fn a_line_split_across_writes_is_emitted_once() {
        let mut b = LineBuf::<64>::new();
        let out = collect(&mut b, &[b"TICKD-", b"ALIVE ", b"tick=42", b"\n"]);
        assert_eq!(out, vec![b"TICKD-ALIVE tick=42\n".to_vec()]);
        assert!(b.is_empty());
    }

    #[test]
    fn two_lines_in_one_write_are_two_emits() {
        let mut b = LineBuf::<64>::new();
        let out = collect(&mut b, &[b"a\nbc\nd"]);
        assert_eq!(out, vec![b"a\n".to_vec(), b"bc\n".to_vec()]);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn an_overlong_line_is_emitted_in_full_buffer_chunks() {
        let mut b = LineBuf::<8>::new();
        let out = collect(&mut b, &[b"0123456789abcdef\n"]);
        assert_eq!(
            out,
            vec![b"01234567".to_vec(), b"89abcdef".to_vec(), b"\n".to_vec()]
        );
    }

    #[test]
    fn flush_appends_a_newline_only_when_a_partial_line_is_pending() {
        let mut b = LineBuf::<16>::new();
        let mut out = Vec::new();
        b.flush(|l| out.push(l.to_vec()));
        assert!(out.is_empty());
        b.push(b"partial", |l| out.push(l.to_vec()));
        b.flush(|l| out.push(l.to_vec()));
        assert_eq!(out, vec![b"partial\n".to_vec()]);
        // A full buffer flushes as the chunk plus a separate newline.
        let mut full = LineBuf::<4>::new();
        let mut out2 = Vec::new();
        full.push(b"abcd", |l| out2.push(l.to_vec()));
        assert!(out2.is_empty());
        full.flush(|l| out2.push(l.to_vec()));
        assert_eq!(out2, vec![b"abcd".to_vec(), b"\n".to_vec()]);
    }

    #[test]
    fn bytes_pass_through_unchanged() {
        let mut b = LineBuf::<32>::new();
        let out = collect(&mut b, &[&[0xff, 0x00, b'x', b'\n']]);
        assert_eq!(out, vec![vec![0xff, 0x00, b'x', b'\n']]);
    }

    #[test]
    fn kernel_markers_in_process_output_are_rewritten() {
        let mut line = b"[ITISYOU:SVC] bg_start and again [ITISYOU:AUDIT]\n".to_vec();
        assert_eq!(neutralize_markers(&mut line), 2);
        assert_eq!(
            &line[..],
            &b"[RING3-U:SVC] bg_start and again [RING3-U:AUDIT]\n"[..]
        );
        let mut plain = b"TCPPROBE-OK\n".to_vec();
        assert_eq!(neutralize_markers(&mut plain), 0);
        let mut short = b"[ITISYOU".to_vec();
        assert_eq!(neutralize_markers(&mut short), 0);
    }
}
