//! Program arguments for Ring 3 programs (V0.10, PROC10-001).
//!
//! A process carries one immutable ARGUMENT BLOCK, fixed when it is created
//! and never changed afterwards. The block is the arguments in order, each
//! followed by a single NUL byte:
//!
//! ```text
//! "alpha\0beta\0gamma\0"   three arguments, 17 bytes
//! ""                       no arguments
//! ```
//!
//! The same encoding is used in both directions — the console builds a block
//! for `run … -- args`, a parent hands one to `spawn_args`, and `args` copies
//! the caller's block back out — so there is exactly one format and one
//! validator. Every block is validated when a process is created from it;
//! nothing downstream has to re-check it.
//!
//! The rules are deliberately narrow:
//!
//! * at most [`MAX_ARGS`] arguments and at most [`MAX_BLOCK`] encoded bytes
//!   (terminators included), so a hostile caller cannot make the kernel hold
//!   an unbounded amount of memory on a process's behalf;
//! * every argument is non-empty printable ASCII WITHOUT spaces
//!   (`0x21..=0x7E`). No control bytes, no escape sequences to replay onto the
//!   console, no bytes that could be mistaken for the terminator, and no
//!   quoting rules — an argument is exactly one console token.
//!
//! Pure and allocation-free: the kernel wraps a validated block in its own
//! reference-counted storage.

/// Most arguments one process can carry.
pub const MAX_ARGS: usize = 16;

/// Largest encoded block, NUL terminators included.
pub const MAX_BLOCK: usize = 512;

/// Why an argument list or block was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgError {
    /// More than [`MAX_ARGS`] arguments.
    TooMany,
    /// The encoded block would exceed [`MAX_BLOCK`] bytes.
    TooLong,
    /// A zero-length argument (two terminators in a row, or a leading one).
    Empty,
    /// A byte outside printable ASCII, or a space.
    BadByte,
    /// A non-empty block whose last argument has no NUL terminator.
    Unterminated,
}

impl ArgError {
    /// Stable short name, used in console messages and serial evidence.
    pub fn name(self) -> &'static str {
        match self {
            ArgError::TooMany => "too_many",
            ArgError::TooLong => "too_long",
            ArgError::Empty => "empty",
            ArgError::BadByte => "bad_byte",
            ArgError::Unterminated => "unterminated",
        }
    }
}

/// True for a byte an argument may contain: printable ASCII other than space.
pub const fn is_arg_byte(b: u8) -> bool {
    b > b' ' && b < 0x7F
}

/// Validate one argument's bytes.
fn check_arg(arg: &[u8]) -> Result<(), ArgError> {
    if arg.is_empty() {
        return Err(ArgError::Empty);
    }
    if arg.iter().any(|&b| !is_arg_byte(b)) {
        return Err(ArgError::BadByte);
    }
    Ok(())
}

/// Encode `args` into `out`, returning the block length.
///
/// All-or-nothing: on any error the returned `Err` is the only result — the
/// caller must not use `out`, which may hold a partial block.
pub fn encode<A: AsRef<[u8]>>(args: &[A], out: &mut [u8; MAX_BLOCK]) -> Result<usize, ArgError> {
    if args.len() > MAX_ARGS {
        return Err(ArgError::TooMany);
    }
    let mut len = 0usize;
    for arg in args {
        let bytes = arg.as_ref();
        check_arg(bytes)?;
        let end = len
            .checked_add(bytes.len())
            .and_then(|n| n.checked_add(1))
            .ok_or(ArgError::TooLong)?;
        if end > MAX_BLOCK {
            return Err(ArgError::TooLong);
        }
        out[len..len + bytes.len()].copy_from_slice(bytes);
        out[len + bytes.len()] = 0;
        len = end;
    }
    Ok(len)
}

/// Validate a block that arrived already encoded (a parent's `spawn_args`
/// request). Returns the argument count.
pub fn validate_block(block: &[u8]) -> Result<usize, ArgError> {
    if block.len() > MAX_BLOCK {
        return Err(ArgError::TooLong);
    }
    let mut count = 0usize;
    let mut start = 0usize;
    for (i, &b) in block.iter().enumerate() {
        if b == 0 {
            if i == start {
                return Err(ArgError::Empty);
            }
            count += 1;
            if count > MAX_ARGS {
                return Err(ArgError::TooMany);
            }
            start = i + 1;
        } else if !is_arg_byte(b) {
            return Err(ArgError::BadByte);
        }
    }
    if start != block.len() {
        return Err(ArgError::Unterminated);
    }
    Ok(count)
}

/// Iterate over the arguments of a block.
///
/// Meant for blocks [`validate_block`] accepted. On an unvalidated block it
/// still terminates and never reads out of bounds: an unterminated tail is
/// yielded as a final argument.
pub fn split(block: &[u8]) -> Split<'_> {
    Split { rest: block }
}

/// Iterator returned by [`split`].
#[derive(Debug, Clone)]
pub struct Split<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Split<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        match self.rest.iter().position(|&b| b == 0) {
            Some(i) => {
                let arg = &self.rest[..i];
                self.rest = &self.rest[i + 1..];
                Some(arg)
            }
            None => {
                let arg = self.rest;
                self.rest = &[];
                Some(arg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(args: &[&str]) -> Result<([u8; MAX_BLOCK], usize), ArgError> {
        let mut out = [0u8; MAX_BLOCK];
        let n = encode(args, &mut out)?;
        Ok((out, n))
    }

    #[test]
    fn encodes_nul_terminated_in_order() {
        let (out, n) = enc(&["alpha", "beta", "gamma"]).unwrap();
        assert_eq!(&out[..n], b"alpha\0beta\0gamma\0");
        assert_eq!(validate_block(&out[..n]), Ok(3));
        let parts: Vec<&[u8]> = split(&out[..n]).collect();
        assert_eq!(parts, [&b"alpha"[..], b"beta", b"gamma"]);
    }

    #[test]
    fn empty_list_is_an_empty_block() {
        let (_, n) = enc(&[]).unwrap();
        assert_eq!(n, 0);
        assert_eq!(validate_block(&[]), Ok(0));
        assert_eq!(split(&[]).count(), 0);
    }

    #[test]
    fn argument_count_bound_is_exact() {
        let sixteen = ["a"; MAX_ARGS];
        let (out, n) = enc(&sixteen).unwrap();
        assert_eq!(validate_block(&out[..n]), Ok(MAX_ARGS));
        let seventeen = ["a"; MAX_ARGS + 1];
        assert_eq!(enc(&seventeen).unwrap_err(), ArgError::TooMany);
        // The same bound holds for a block that arrives pre-encoded.
        let block = b"a\0".repeat(MAX_ARGS + 1);
        assert_eq!(validate_block(&block), Err(ArgError::TooMany));
    }

    #[test]
    fn block_size_bound_is_exact() {
        // One argument of 511 bytes + its terminator = exactly 512.
        let max = "x".repeat(MAX_BLOCK - 1);
        let (out, n) = enc(&[max.as_str()]).unwrap();
        assert_eq!(n, MAX_BLOCK);
        assert_eq!(validate_block(&out[..n]), Ok(1));
        let over = "x".repeat(MAX_BLOCK);
        assert_eq!(enc(&[over.as_str()]).unwrap_err(), ArgError::TooLong);
        let mut block = over.into_bytes();
        block.push(0);
        assert_eq!(validate_block(&block), Err(ArgError::TooLong));
        // The terminators count: two 255-byte args + 2 NULs = 512 fits,
        // one more byte anywhere does not.
        let a = "y".repeat(255);
        assert_eq!(enc(&[a.as_str(), a.as_str()]).unwrap().1, MAX_BLOCK);
        let b = "y".repeat(256);
        assert_eq!(
            enc(&[a.as_str(), b.as_str()]).unwrap_err(),
            ArgError::TooLong
        );
    }

    #[test]
    fn refuses_space_control_and_non_ascii_bytes() {
        for bad in ["two words", "tab\there", "bell\u{7}", "del\u{7f}", "é"] {
            assert_eq!(enc(&[bad]).unwrap_err(), ArgError::BadByte, "{bad:?}");
            let mut block = bad.as_bytes().to_vec();
            block.push(0);
            assert_eq!(validate_block(&block), Err(ArgError::BadByte), "{bad:?}");
        }
        // Every printable non-space byte is accepted.
        let all: Vec<u8> = (0x21u8..=0x7E).collect();
        let mut block = all.clone();
        block.push(0);
        assert_eq!(validate_block(&block), Ok(1));
        assert_eq!(split(&block).next(), Some(&all[..]));
    }

    #[test]
    fn refuses_empty_arguments() {
        assert_eq!(enc(&["a", ""]).unwrap_err(), ArgError::Empty);
        assert_eq!(validate_block(b"\0"), Err(ArgError::Empty));
        assert_eq!(validate_block(b"a\0\0b\0"), Err(ArgError::Empty));
    }

    #[test]
    fn refuses_unterminated_block() {
        assert_eq!(validate_block(b"abc"), Err(ArgError::Unterminated));
        assert_eq!(validate_block(b"a\0bc"), Err(ArgError::Unterminated));
    }

    #[test]
    fn split_is_total_on_unvalidated_input() {
        let parts: Vec<&[u8]> = split(b"a\0bc").collect();
        assert_eq!(parts, [&b"a"[..], b"bc"]);
        let parts: Vec<&[u8]> = split(b"\0\0").collect();
        assert_eq!(parts, [&b""[..], b""]);
    }

    #[test]
    fn error_names_are_stable() {
        assert_eq!(ArgError::TooMany.name(), "too_many");
        assert_eq!(ArgError::TooLong.name(), "too_long");
        assert_eq!(ArgError::Empty.name(), "empty");
        assert_eq!(ArgError::BadByte.name(), "bad_byte");
        assert_eq!(ArgError::Unterminated.name(), "unterminated");
    }
}
