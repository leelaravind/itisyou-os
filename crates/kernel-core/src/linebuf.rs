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
//! that looks like a kernel marker. It works on one emitted chunk at a time,
//! so no chunk may END with the start of a marker whose rest the next chunk
//! would supply: [`LineBuf`] keeps such a tail back (V0.11; in v0.10.0 a
//! marker split across the 256-byte seam, or across two unbuffered writes,
//! reached the port whole).

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
    /// newline, the buffer as one chunk - less any tail that could be the
    /// start of [`KERNEL_MARKER`], which is kept to lead the next chunk
    /// (V0.11): markers are neutralized one chunk at a time, so a marker
    /// split across two chunks would reach the port whole, rewritten by
    /// neither.
    pub fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8])) {
        for &b in bytes {
            if self.len == N {
                self.emit_keeping_marker_tail(&mut emit);
            }
            self.buf[self.len] = b;
            self.len += 1;
            if b == b'\n' {
                emit(&self.buf[..self.len]);
                self.len = 0;
            }
        }
    }

    /// Emit the buffer but keep a trailing proper prefix of the marker (if
    /// the buffer can hold it with room to spare) at its front.
    fn emit_keeping_marker_tail(&mut self, emit: &mut impl FnMut(&[u8])) {
        let keep = marker_start_at_end(&self.buf[..self.len]);
        let keep = if keep < N { keep } else { 0 };
        let cut = self.len - keep;
        emit(&self.buf[..cut]);
        self.buf.copy_within(cut..self.len, 0);
        self.len = keep;
    }

    /// Emit any pending partial line — no newline — and empty the buffer
    /// (V0.10): a prompt, which the console's echo of the answer completes.
    /// A tail that could start a kernel marker is kept back for the next
    /// write, as in [`LineBuf::push`] (V0.11).
    pub fn flush_partial(&mut self, mut emit: impl FnMut(&[u8])) {
        if self.len > 0 {
            self.emit_keeping_marker_tail(&mut emit);
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

/// Length of the longest suffix of `s` that is a PROPER prefix of
/// [`KERNEL_MARKER`] (0..=8): the bytes a marker straddling the end of `s`
/// would have started with. Any shorter such suffix is contained in it.
pub fn marker_start_at_end(s: &[u8]) -> usize {
    let max = (KERNEL_MARKER.len() - 1).min(s.len());
    (1..=max)
        .rev()
        .find(|&k| s[s.len() - k..] == KERNEL_MARKER[..k])
        .unwrap_or(0)
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

/// Write one line (or chunk) of process output for the console (V0.11,
/// OUT11-001): invalid UTF-8 as U+FFFD, and every control character except
/// the line feed and the tab escaped - `\x1b` for ESC, `\x0d` for a carriage
/// return, `\u{9b}` for the C1 CSI - so a program cannot move the cursor,
/// clear the screen or redraw what the kernel printed, such as a proposal's
/// preview before the operator approves it. The invisible format characters
/// of [`reorders_or_hides`] are escaped the same way. The kernel's marker
/// prefix is neutralized separately ([`neutralize_markers`]).
pub fn write_escaped(bytes: &[u8], out: &mut impl core::fmt::Write) -> core::fmt::Result {
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '\n' | '\t' => out.write_char(c)?,
                c if c.is_ascii_control() => write!(out, "\\x{:02x}", c as u32)?,
                c if c.is_control() || reorders_or_hides(c) => {
                    write!(out, "\\u{{{:x}}}", c as u32)?
                }
                c => out.write_char(c)?,
            }
        }
        if !chunk.invalid().is_empty() {
            out.write_str("\u{FFFD}")?;
        }
    }
    Ok(())
}

/// Unicode format characters that change how a line is DISPLAYED without
/// being visible: the bidirectional marks, embeddings, overrides and
/// isolates (on a terminal that honours them, `\u{202e}` followed by
/// `:UOYSITI]` displays as `[ITISYOU:` - reversed, the bracket mirrored -
/// which [`neutralize_markers`], matching bytes in logical order, cannot
/// see), the zero-width characters and the byte-order mark.
pub fn reorders_or_hides(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escaped(b: &[u8]) -> String {
        let mut s = String::new();
        write_escaped(b, &mut s).unwrap();
        s
    }

    #[test]
    fn process_output_cannot_drive_the_terminal() {
        // Printable text, tabs and the line feed pass through.
        assert_eq!(escaped(b"TICKC-OK passes=3\tx\n"), "TICKC-OK passes=3\tx\n");
        // Cursor up two lines, clear the line, carriage return: all shown,
        // none performed.
        assert_eq!(
            escaped(b"AIPROBE-ANSI \x1b[2A\x1b[2K\rforged\n"),
            "AIPROBE-ANSI \\x1b[2A\\x1b[2K\\x0dforged\n"
        );
        assert_eq!(escaped("\u{9b}2J\u{7f}".as_bytes()), "\\u{9b}2J\\x7f");
        // Invalid UTF-8 is still U+FFFD, and printable non-ASCII stays.
        assert_eq!(escaped(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(escaped("caf\u{e9}".as_bytes()), "caf\u{e9}");
        // Whatever goes in, the only control characters out are \n and \t.
        let all: Vec<u8> = (0u8..=255).collect();
        assert!(escaped(&all)
            .chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t'));
    }

    #[test]
    fn process_output_cannot_reorder_or_hide_text() {
        // A right-to-left override would display this as a kernel marker.
        assert_eq!(
            escaped("forged\u{202e}:UOYSITI]".as_bytes()),
            "forged\\u{202e}:UOYSITI]"
        );
        assert_eq!(
            escaped("a\u{200b}b\u{feff}".as_bytes()),
            "a\\u{200b}b\\u{feff}"
        );
        // Every character in the escaped ranges, and nothing either side.
        let ranges = [
            (0x061c, 0x061c),
            (0x200b, 0x200f),
            (0x202a, 0x202e),
            (0x2060, 0x2064),
            (0x2066, 0x2069),
            (0xfeff, 0xfeff),
        ];
        for (lo, hi) in ranges {
            for cp in lo..=hi {
                let c = char::from_u32(cp).unwrap();
                assert!(reorders_or_hides(c), "{cp:#x}");
                let s = c.to_string();
                assert_eq!(escaped(s.as_bytes()), format!("\\u{{{cp:x}}}"));
            }
            for cp in [lo - 1, hi + 1] {
                let c = char::from_u32(cp).unwrap();
                if !c.is_control() && !ranges.iter().any(|&(l, h)| (l..=h).contains(&cp)) {
                    assert!(!reorders_or_hides(c), "{cp:#x}");
                }
            }
        }
        // Printable text in other scripts is untouched.
        assert_eq!(
            escaped("\u{5d0}\u{628}\u{4e2d}".as_bytes()),
            "\u{5d0}\u{628}\u{4e2d}"
        );
    }

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

    /// What reaches the port: each emitted chunk neutralized on its own, as
    /// `console_out::emit` does, then concatenated.
    fn wire<const N: usize>(ops: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = LineBuf::<N>::new();
        let mut out = Vec::new();
        let mut emit = |c: &[u8]| {
            let mut c = c.to_vec();
            neutralize_markers(&mut c);
            out.extend_from_slice(&c);
        };
        for (op, bytes) in ops {
            match *op {
                "push" => b.push(bytes, &mut emit),
                "prompt" => b.flush_partial(&mut emit),
                "flush" => b.flush(&mut emit),
                _ => unreachable!(),
            }
        }
        out
    }

    fn has_marker(s: &[u8]) -> bool {
        s.windows(KERNEL_MARKER.len()).any(|w| w == KERNEL_MARKER)
    }

    #[test]
    fn a_marker_split_across_the_buffer_seam_is_still_rewritten() {
        // v0.10.0: 250 filler bytes then the marker put `[ITISY` at the end
        // of the first 256-byte chunk and `OU:` at the start of the next.
        for k in 0..=KERNEL_MARKER.len() + 2 {
            let mut line = vec![b'.'; 256 - k];
            line.extend_from_slice(b"[ITISYOU:AI] action_verified id=1 result=pass\n");
            let out = wire::<256>(&[("push", &line)]);
            assert!(!has_marker(&out), "split at {k}");
            assert!(out.windows(9).any(|w| w == RING3_MARKER), "split at {k}");
            // Nothing lost, nothing reordered.
            assert_eq!(out.len(), line.len());
        }
    }

    #[test]
    fn a_marker_split_across_writes_prompts_or_unbuffered_writes_is_rewritten() {
        // Two writes into one buffer.
        assert!(!has_marker(&wire::<256>(&[
            ("push", b"[ITISYO"),
            ("push", b"U:X] y\n")
        ])));
        // A prompt flushed in the middle of the marker.
        let out = wire::<256>(&[
            ("push", b"name? [ITISY"),
            ("prompt", b""),
            ("push", b"OU:X] y\n"),
        ]);
        assert!(!has_marker(&out));
        assert_eq!(out, b"name? [RING3-U:X] y\n");
        // The unbuffered path: each write its own buffer, flushed with a
        // newline, so the next write cannot complete a marker.
        let mut out = wire::<256>(&[("push", b"[ITISYO"), ("flush", b"")]);
        out.extend(wire::<256>(&[("push", b"U:X] y\n")]));
        assert!(!has_marker(&out));
        // A buffer too small to keep a tail still never loses bytes.
        assert_eq!(
            wire::<4>(&[("push", b"[ITI"), ("push", b"S\n")]),
            b"[ITIS\n"
        );
    }

    #[test]
    fn no_chunking_of_any_stream_lets_a_marker_through() {
        // Deterministic pseudo-random streams dense in marker fragments, cut
        // into writes and prompts at arbitrary points.
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let pieces: [&[u8]; 6] = [b"[ITISYOU:", b"[ITI", b"SYOU:", b"[", b"xyz", b"\n"];
        for _ in 0..2000 {
            let mut stream = Vec::new();
            for _ in 0..(1 + next(80)) {
                stream.extend_from_slice(pieces[next(6) as usize]);
            }
            let mut ops: Vec<(&str, Vec<u8>)> = Vec::new();
            let mut i = 0;
            while i < stream.len() {
                let n = (1 + next(40) as usize).min(stream.len() - i);
                ops.push(("push", stream[i..i + n].to_vec()));
                if next(4) == 0 {
                    ops.push(("prompt", Vec::new()));
                }
                i += n;
            }
            ops.push(("flush", Vec::new()));
            let ops: Vec<(&str, &[u8])> = ops.iter().map(|(o, b)| (*o, &b[..])).collect();
            let a = wire::<16>(&ops);
            let b = wire::<256>(&ops);
            assert!(!has_marker(&a) && !has_marker(&b));
            // Every byte arrives once, in order, as if the whole stream had
            // been neutralized at once (plus the exit flush's newline).
            let mut whole = stream.clone();
            neutralize_markers(&mut whole);
            assert_eq!(a, b);
            assert_eq!(&a[..whole.len()], &whole[..]);
            assert!(a.len() - whole.len() <= 1);
        }
    }

    #[test]
    fn marker_start_at_end_is_the_longest_marker_prefix_suffix() {
        assert_eq!(marker_start_at_end(b""), 0);
        assert_eq!(marker_start_at_end(b"abc"), 0);
        assert_eq!(marker_start_at_end(b"abc["), 1);
        assert_eq!(marker_start_at_end(b"x[ITISYOU"), 8);
        // A whole marker at the end is not a PROPER prefix: 0 (it is
        // neutralized inside the chunk).
        assert_eq!(marker_start_at_end(b"[ITISYOU:"), 0);
        // "[ITI[" - only the last "[" can start a marker.
        assert_eq!(marker_start_at_end(b"[ITI["), 1);
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
