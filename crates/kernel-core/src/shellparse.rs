//! Shell command-line tokenization (requirement SH-001).
//!
//! Bounded, allocation-free parsing: at most [`MAX_ARGS`] whitespace-separated
//! tokens. Anything beyond the bound is an explicit error, never silent
//! truncation.

/// Token bound for one console line.
///
/// V0.10 raised it from 8 so `run <path> [caps|-] [prefix|-] -- <args>` can
/// carry a full program argument list: five tokens of launch options plus
/// `crate::progargs::MAX_ARGS` (16) arguments is 21, and the console must be
/// able to send one MORE than the argument limit so that limit is enforced by
/// the argument validator — with a clear message — rather than hidden behind
/// the tokenizer's. The line itself stays bounded at 256 bytes by the shell.
pub const MAX_ARGS: usize = 24;

// `run <path> <caps> <prefix> --` plus one argument past the program limit
// must still reach the argument validator (checked at compile time).
const _: () = assert!(5 + crate::progargs::MAX_ARGS < MAX_ARGS);

/// Parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// More than [`MAX_ARGS`] tokens.
    TooManyArgs,
}

/// A parsed command line borrowing from the input buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLine<'a> {
    args: [&'a str; MAX_ARGS],
    len: usize,
}

impl<'a> CommandLine<'a> {
    /// The command name (first token), if any.
    pub fn command(&self) -> Option<&'a str> {
        (self.len > 0).then(|| self.args[0])
    }

    /// Arguments after the command name (none for an empty line).
    pub fn args(&self) -> &[&'a str] {
        // An empty line has no command, so `1..0` would be an inverted range
        // (V1-REL-005 fuzzing found the panic).
        self.args.get(1..self.len).unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Tokenize a line on ASCII whitespace.
pub fn parse(line: &str) -> Result<CommandLine<'_>, ParseError> {
    let mut args = [""; MAX_ARGS];
    let mut len = 0usize;
    for token in line.split_ascii_whitespace() {
        if len == MAX_ARGS {
            return Err(ParseError::TooManyArgs);
        }
        args[len] = token;
        len += 1;
    }
    Ok(CommandLine { args, len })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_and_args() {
        let cl = parse("cat /etc/version").unwrap();
        assert_eq!(cl.command(), Some("cat"));
        assert_eq!(cl.args(), &["/etc/version"]);
    }

    #[test]
    fn empty_and_whitespace_lines() {
        assert!(parse("").unwrap().is_empty());
        assert!(parse("   \t  ").unwrap().is_empty());
        assert_eq!(parse("  ").unwrap().command(), None);
    }

    /// V1-REL-005 regression: `args()` of an empty or all-whitespace line
    /// sliced `args[1..0]` and panicked.
    #[test]
    fn args_of_an_empty_line_is_empty() {
        for line in ["", "   \t  "] {
            let cl = parse(line).unwrap();
            assert!(cl.is_empty());
            assert!(cl.args().is_empty());
        }
        assert!(parse("cmd").unwrap().args().is_empty());
    }

    #[test]
    fn collapses_repeated_whitespace() {
        let cl = parse("  echo   a\t b ").unwrap();
        assert_eq!(cl.command(), Some("echo"));
        assert_eq!(cl.args(), &["a", "b"]);
    }

    #[test]
    fn enforces_arg_bound() {
        // Exactly MAX_ARGS tokens parse; one more is an error, not a
        // truncation.
        let mut ok = String::from("c");
        for i in 1..MAX_ARGS {
            ok.push_str(&format!(" {i}"));
        }
        assert_eq!(parse(&ok).unwrap().args().len(), MAX_ARGS - 1);
        let too_many = format!("{ok} {MAX_ARGS}");
        assert_eq!(parse(&too_many), Err(ParseError::TooManyArgs));
    }
}
