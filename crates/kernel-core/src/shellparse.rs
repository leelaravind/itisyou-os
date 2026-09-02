//! Shell command-line tokenization (requirement SH-001).
//!
//! Bounded, allocation-free parsing: at most [`MAX_ARGS`] whitespace-separated
//! tokens. Anything beyond the bound is an explicit error, never silent
//! truncation.

pub const MAX_ARGS: usize = 8;

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

    /// Arguments after the command name.
    pub fn args(&self) -> &[&'a str] {
        &self.args[1..self.len]
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

    #[test]
    fn collapses_repeated_whitespace() {
        let cl = parse("  echo   a\t b ").unwrap();
        assert_eq!(cl.command(), Some("echo"));
        assert_eq!(cl.args(), &["a", "b"]);
    }

    #[test]
    fn enforces_arg_bound() {
        let ok = "c 1 2 3 4 5 6 7";
        assert_eq!(parse(ok).unwrap().args().len(), 7);
        let too_many = "c 1 2 3 4 5 6 7 8";
        assert_eq!(parse(too_many), Err(ParseError::TooManyArgs));
    }
}
