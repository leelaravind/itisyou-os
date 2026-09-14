//! `/etc/init.conf` (V0.10): the services `/sbin/init` (pid 1) starts at boot.
//!
//! ```text
//! line    := blank | '#' comment | service
//! service := 'service' SP name SP path (SP option)* [SP '--' (SP arg)+]
//! name    := [a-z0-9-]{1,15}
//! path    := ('/bin/' | '/sbin/') [A-Za-z0-9._-]+     ; <= 64 bytes, no '..' or '//'
//! option  := 'caps=' ('-' | capname(','capname)*)      ; caps names; default '-'
//!          | 'restart=' ('always'|'on-failure'|'never'); default on-failure
//!          | 'after=' name(','name)*                   ; <= 4 dependencies
//! arg     := progargs-valid token                      ; <= 16 args, <= 512 bytes encoded
//! ```
//!
//! Concrete rules, so every accept/refuse decision is predictable:
//!
//! * The file is UTF-8, at most [`MAX_FILE`] bytes. Lines end in LF or CRLF;
//!   the last line may omit its terminator. A CR anywhere else is an ordinary
//!   byte (and so never valid inside a token).
//! * A line's length excludes its terminator and is at most [`MAX_LINE`]
//!   bytes — comment and blank lines included.
//! * `SP` is a run of one or more spaces or tabs; whitespace at the start or
//!   end of a line is ignored. A line whose first token starts with `#` is a
//!   comment; `#` anywhere else has no special meaning.
//! * Keywords, option names, capability names and policies are
//!   case-sensitive. Each option appears at most once per line. Everything
//!   after a standalone `--` token is program arguments — at least one, each
//!   validated by [`progargs`] exactly as the kernel will validate it.
//! * A path's file name may not be `.` (that names the directory itself).
//! * At most [`MAX_SERVICES`] services; names are unique; dependencies may
//!   name services defined later in the file.
//!
//! Errors carry the 1-based line they were found on; whole-file errors
//! (`file_too_large`, `not_utf8`, `cycle`) carry line 0. Lines are checked in
//! order and the first error wins; `unknown_dependency` is checked once every
//! line has parsed (it is reported at the first line that references a
//! missing service), and `cycle` last. A config that parses is therefore
//! guaranteed to have a start order.
//!
//! Start order: dependencies first, then the lowest line (index) first — at
//! every step the lowest-index service whose dependencies have all started
//! starts next. Restart policy semantics live in [`crate::supervise`].
//!
//! Allocation-free: the parsed [`Config`] borrows every string from the
//! input text.

use crate::caps;
use crate::progargs;
use crate::service;
pub use crate::supervise::Restart;
use crate::svcreport::{self, ServiceName};

/// Largest accepted file.
pub const MAX_FILE: usize = 1024;
/// Longest line, terminator excluded.
pub const MAX_LINE: usize = 160;
/// Longest program path.
pub const MAX_PATH: usize = 64;
/// Most services one file may define.
pub const MAX_SERVICES: usize = service::MAX_SERVICES;
/// Most dependencies (`after=`) one service may name.
pub const MAX_DEPS: usize = service::MAX_DEPS;
/// Longest service name.
pub const MAX_NAME: usize = svcreport::MAX_NAME;

/// Why a config was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A non-blank, non-comment line that does not start with `service`.
    UnknownDirective,
    /// `service` with nothing after it.
    MissingName,
    /// `service <name>` with no path.
    MissingPath,
    /// A service or dependency name outside `[a-z0-9-]{1,15}`.
    BadName,
    /// A second service with a name already defined.
    DuplicateName,
    /// A path outside the `path` rule.
    BadPath,
    /// A token that is not `caps=`, `restart=`, `after=` or `--`.
    UnknownOption,
    /// The same option twice on one line.
    DuplicateOption,
    /// A `caps=` list naming no known capability (or malformed).
    UnknownCapability,
    /// A `caps=` list naming a capability only the kernel console may grant
    /// (V0.11: `sys_view`, `propose`) - a service is init's child, and that
    /// authority cannot be passed to a child.
    ConsoleOnlyCapability,
    /// A `restart=` value other than always/on-failure/never.
    BadRestart,
    /// More than [`MAX_DEPS`] dependencies.
    TooManyDeps,
    /// `--` with no arguments, or arguments progargs refuses.
    BadArgs,
    /// A line longer than [`MAX_LINE`] bytes.
    LineTooLong,
    /// More than [`MAX_SERVICES`] services.
    TooManyServices,
    /// `after=` names a service the file does not define.
    UnknownDependency,
    /// The dependency graph has a cycle, so no start order exists.
    Cycle,
    /// The file is larger than [`MAX_FILE`] bytes.
    FileTooLarge,
    /// The file is not UTF-8.
    NotUtf8,
}

impl Reason {
    /// Every reason, in declaration order.
    pub const ALL: [Reason; 19] = [
        Reason::UnknownDirective,
        Reason::MissingName,
        Reason::MissingPath,
        Reason::BadName,
        Reason::DuplicateName,
        Reason::BadPath,
        Reason::UnknownOption,
        Reason::DuplicateOption,
        Reason::UnknownCapability,
        Reason::ConsoleOnlyCapability,
        Reason::BadRestart,
        Reason::TooManyDeps,
        Reason::BadArgs,
        Reason::LineTooLong,
        Reason::TooManyServices,
        Reason::UnknownDependency,
        Reason::Cycle,
        Reason::FileTooLarge,
        Reason::NotUtf8,
    ];

    /// Stable snake_case name, used in console messages and serial evidence.
    pub const fn name(self) -> &'static str {
        match self {
            Reason::UnknownDirective => "unknown_directive",
            Reason::MissingName => "missing_name",
            Reason::MissingPath => "missing_path",
            Reason::BadName => "bad_name",
            Reason::DuplicateName => "duplicate_name",
            Reason::BadPath => "bad_path",
            Reason::UnknownOption => "unknown_option",
            Reason::DuplicateOption => "duplicate_option",
            Reason::UnknownCapability => "unknown_capability",
            Reason::ConsoleOnlyCapability => "console_only_capability",
            Reason::BadRestart => "bad_restart",
            Reason::TooManyDeps => "too_many_deps",
            Reason::BadArgs => "bad_args",
            Reason::LineTooLong => "line_too_long",
            Reason::TooManyServices => "too_many_services",
            Reason::UnknownDependency => "unknown_dependency",
            Reason::Cycle => "cycle",
            Reason::FileTooLarge => "file_too_large",
            Reason::NotUtf8 => "not_utf8",
        }
    }
}

/// A refused config: the reason and the 1-based line (0 = whole file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigError {
    pub line: usize,
    pub reason: Reason,
}

const fn err(line: usize, reason: Reason) -> ConfigError {
    ConfigError { line, reason }
}

/// One `service` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service<'a> {
    pub name: &'a str,
    pub path: &'a str,
    /// Exactly the capabilities the service runs with (default none).
    pub caps: u64,
    pub restart: Restart,
    /// Dependencies; the first `n_after` entries are meaningful.
    pub after: [&'a str; MAX_DEPS],
    pub n_after: usize,
    /// The raw argument text after `--` (trimmed; empty when there is none).
    /// Use [`Service::arg_tokens`] or [`Service::encode_args`].
    pub args: &'a str,
    /// The 1-based line the service was defined on (diagnostics).
    pub line: usize,
}

impl<'a> Service<'a> {
    /// The dependencies (`after=`), in the order written.
    pub fn deps(&self) -> &[&'a str] {
        &self.after[..self.n_after.min(MAX_DEPS)]
    }

    /// The program arguments, in order.
    pub fn arg_tokens(&self) -> impl Iterator<Item = &'a str> {
        let args: &'a str = self.args;
        args.split(is_sep_char).filter(|t| !t.is_empty())
    }

    /// Encode the arguments as a [`progargs`] block into `out`, returning its
    /// length (0 for no arguments). Same all-or-nothing contract as
    /// [`progargs::encode`]; never fails for a service [`parse`] accepted.
    pub fn encode_args(
        &self,
        out: &mut [u8; progargs::MAX_BLOCK],
    ) -> Result<usize, progargs::ArgError> {
        let mut tokens: [&str; progargs::MAX_ARGS] = [""; progargs::MAX_ARGS];
        let mut n = 0usize;
        for token in self.arg_tokens() {
            if n == progargs::MAX_ARGS {
                return Err(progargs::ArgError::TooMany);
            }
            tokens[n] = token;
            n += 1;
        }
        progargs::encode(&tokens[..n], out)
    }

    /// True for the long-running (`restart=always`) policy.
    pub fn long_running(&self) -> bool {
        crate::supervise::long_running(self.restart)
    }
}

/// A parsed config: `services[..len]` are `Some`, in file order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config<'a> {
    pub services: [Option<Service<'a>>; MAX_SERVICES],
    pub len: usize,
}

impl<'a> Config<'a> {
    /// The services, in file order.
    pub fn services(&self) -> impl Iterator<Item = &Service<'a>> {
        self.services[..self.len.min(MAX_SERVICES)].iter().flatten()
    }

    /// The service at `index` (file order), if any.
    pub fn get(&self, index: usize) -> Option<&Service<'a>> {
        if index < self.len {
            self.services.get(index)?.as_ref()
        } else {
            None
        }
    }

    /// Index of the service called `name`.
    pub fn find(&self, name: &str) -> Option<usize> {
        (0..self.len.min(MAX_SERVICES)).find(|&i| self.get(i).is_some_and(|s| s.name == name))
    }

    /// Start order: indices into `services`, dependencies first, then the
    /// lowest index first. Returns the order and its length.
    ///
    /// Always succeeds for a config [`parse`] returned. For a hand-built one
    /// it reports `too_many_services`/`cycle` at line 0 and
    /// `too_many_deps`/`unknown_dependency` at the service's line; `None`
    /// slots are skipped.
    pub fn order(&self) -> Result<([usize; MAX_SERVICES], usize), ConfigError> {
        let n = self.len;
        if n > MAX_SERVICES {
            return Err(err(0, Reason::TooManyServices));
        }
        // Resolve every dependency name to an index up front.
        let mut deps = [[0usize; MAX_DEPS]; MAX_SERVICES];
        let mut dep_count = [0usize; MAX_SERVICES];
        let mut present = [false; MAX_SERVICES];
        for (i, slot) in self.services[..n].iter().enumerate() {
            let Some(svc) = slot else { continue };
            present[i] = true;
            if svc.n_after > MAX_DEPS {
                return Err(err(svc.line, Reason::TooManyDeps));
            }
            for (k, dep) in svc.deps().iter().enumerate() {
                deps[i][k] = self
                    .find(dep)
                    .ok_or(err(svc.line, Reason::UnknownDependency))?;
            }
            dep_count[i] = svc.n_after;
        }
        // Kahn's algorithm, always taking the lowest ready index.
        let total = present.iter().filter(|&&p| p).count();
        let mut started = [false; MAX_SERVICES];
        let mut order = [0usize; MAX_SERVICES];
        for slot in order.iter_mut().take(total) {
            let next = (0..n).find(|&i| {
                present[i] && !started[i] && deps[i][..dep_count[i]].iter().all(|&d| started[d])
            });
            let Some(i) = next else {
                return Err(err(0, Reason::Cycle));
            };
            started[i] = true;
            *slot = i;
        }
        Ok((order, total))
    }
}

/// Parse a config that arrived as raw bytes (e.g. read from the VFS):
/// size first, then UTF-8, then [`parse`].
pub fn parse_bytes(bytes: &[u8]) -> Result<Config<'_>, ConfigError> {
    if bytes.len() > MAX_FILE {
        return Err(err(0, Reason::FileTooLarge));
    }
    let text = core::str::from_utf8(bytes).map_err(|_| err(0, Reason::NotUtf8))?;
    parse(text)
}

/// Parse and fully validate a config: syntax, limits, dependencies and
/// acyclicity.
pub fn parse(text: &str) -> Result<Config<'_>, ConfigError> {
    if text.len() > MAX_FILE {
        return Err(err(0, Reason::FileTooLarge));
    }
    let mut config = Config::default();
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        let line = index + 1;
        let content = match raw.strip_suffix('\n') {
            Some(body) => body.strip_suffix('\r').unwrap_or(body),
            None => raw,
        };
        if content.len() > MAX_LINE {
            return Err(err(line, Reason::LineTooLong));
        }
        if let Some(svc) = parse_line(content, line, &config)? {
            config.services[config.len] = Some(svc);
            config.len += 1;
        }
    }
    // Dependencies resolve and admit an order; the order itself is
    // recomputed on demand by `Config::order`.
    config.order()?;
    Ok(config)
}

const fn is_sep(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn is_sep_char(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// Whitespace tokenizer that can hand back the untokenized rest of a line.
struct Tokens<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Tokens<'a> {
    fn new(text: &'a str) -> Self {
        Tokens { text, pos: 0 }
    }

    /// Everything after the last token returned, separators trimmed.
    fn rest(&self) -> &'a str {
        self.text[self.pos..].trim_matches(is_sep_char)
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let bytes = self.text.as_bytes();
        while self.pos < bytes.len() && is_sep(bytes[self.pos]) {
            self.pos += 1;
        }
        if self.pos == bytes.len() {
            return None;
        }
        let start = self.pos;
        while self.pos < bytes.len() && !is_sep(bytes[self.pos]) {
            self.pos += 1;
        }
        // Separators are ASCII, so both ends are char boundaries.
        Some(&self.text[start..self.pos])
    }
}

/// Parse one line (terminator already removed). `Ok(None)` for blank and
/// comment lines.
fn parse_line<'a>(
    content: &'a str,
    line: usize,
    config: &Config<'a>,
) -> Result<Option<Service<'a>>, ConfigError> {
    let fail = |reason| err(line, reason);
    let mut tokens = Tokens::new(content);
    let Some(directive) = tokens.next() else {
        return Ok(None);
    };
    if directive.starts_with('#') {
        return Ok(None);
    }
    if directive != "service" {
        return Err(fail(Reason::UnknownDirective));
    }
    if config.len >= MAX_SERVICES {
        return Err(fail(Reason::TooManyServices));
    }
    let name = tokens.next().ok_or(fail(Reason::MissingName))?;
    if !ServiceName::is_valid(name.as_bytes()) {
        return Err(fail(Reason::BadName));
    }
    if config.find(name).is_some() {
        return Err(fail(Reason::DuplicateName));
    }
    let path = tokens.next().ok_or(fail(Reason::MissingPath))?;
    if !valid_path(path) {
        return Err(fail(Reason::BadPath));
    }
    let mut svc = Service {
        name,
        path,
        caps: 0,
        restart: Restart::default(),
        after: [""; MAX_DEPS],
        n_after: 0,
        args: "",
        line,
    };
    let (mut seen_caps, mut seen_restart, mut seen_after) = (false, false, false);
    while let Some(token) = tokens.next() {
        if token == "--" {
            svc.args = tokens.rest();
            let mut block = [0u8; progargs::MAX_BLOCK];
            if svc.args.is_empty() || svc.encode_args(&mut block).is_err() {
                return Err(fail(Reason::BadArgs));
            }
            break;
        }
        let (key, value) = token.split_once('=').ok_or(fail(Reason::UnknownOption))?;
        let seen = match key {
            "caps" => &mut seen_caps,
            "restart" => &mut seen_restart,
            "after" => &mut seen_after,
            _ => return Err(fail(Reason::UnknownOption)),
        };
        if *seen {
            return Err(fail(Reason::DuplicateOption));
        }
        *seen = true;
        match key {
            "caps" => svc.caps = parse_caps(value).map_err(fail)?,
            "restart" => svc.restart = Restart::parse(value).ok_or(fail(Reason::BadRestart))?,
            _ => {
                for dep in value.split(',') {
                    if svc.n_after == MAX_DEPS {
                        return Err(fail(Reason::TooManyDeps));
                    }
                    if !ServiceName::is_valid(dep.as_bytes()) {
                        return Err(fail(Reason::BadName));
                    }
                    svc.after[svc.n_after] = dep;
                    svc.n_after += 1;
                }
            }
        }
    }
    Ok(Some(svc))
}

/// `-` (no capabilities) or a comma-separated list of capability names, each
/// looked up by [`caps::parse`]. Empty elements and surrounding whitespace
/// (which `caps::parse` would forgive) are refused here.
fn parse_caps(value: &str) -> Result<u64, Reason> {
    if value == "-" {
        return Ok(0);
    }
    let mut bits = 0u64;
    for name in value.split(',') {
        if name.is_empty() || name.trim() != name {
            return Err(Reason::UnknownCapability);
        }
        bits |= caps::parse(name).map_err(|_| Reason::UnknownCapability)?;
    }
    // A service is init's child, and the console-only bits cannot be passed
    // to a child (V0.11): naming one is refused here rather than silently
    // dropped at spawn.
    if bits & caps::CAP_CONSOLE_ONLY != 0 {
        return Err(Reason::ConsoleOnlyCapability);
    }
    Ok(bits)
}

fn valid_path(path: &str) -> bool {
    if path.len() > MAX_PATH || path.contains("..") || path.contains("//") {
        return false;
    }
    let Some(file) = path
        .strip_prefix("/bin/")
        .or_else(|| path.strip_prefix("/sbin/"))
    else {
        return false;
    };
    !file.is_empty()
        && file != "."
        && file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CAP_FS_READ, CAP_GUI, CAP_IPC, CAP_SPAWN};

    const SHIPPED: &str = include_str!("../../../initramfs/root/etc/init.conf");
    const DEMO: &str = include_str!("../../../initramfs/root/etc/init-tests/demo.conf");
    const CYCLE: &str = include_str!("../../../initramfs/root/etc/init-tests/cycle.conf");
    const BADCAP: &str = include_str!("../../../initramfs/root/etc/init-tests/badcap.conf");

    fn fails(text: &str) -> (usize, Reason) {
        let e = parse(text).expect_err(text);
        (e.line, e.reason)
    }

    fn order_of(text: &str) -> Vec<usize> {
        let config = parse(text).unwrap();
        let (order, n) = config.order().unwrap();
        order[..n].to_vec()
    }

    fn only(text: &str) -> Service<'_> {
        let config = parse(text).unwrap();
        assert_eq!(config.len, 1);
        config.services[0].unwrap()
    }

    fn only_first(text: &str) -> Service<'_> {
        parse(text).unwrap().services[0].unwrap()
    }

    fn block(svc: &Service<'_>) -> Vec<u8> {
        let mut out = [0u8; progargs::MAX_BLOCK];
        let n = svc.encode_args(&mut out).unwrap();
        out[..n].to_vec()
    }

    #[test]
    fn shipped_init_conf_is_v08_background_plus_inferd() {
        let config = parse(SHIPPED).unwrap();
        assert_eq!(config.len, 4);
        let got: Vec<(&str, &str, u64, Restart)> = config
            .services()
            .map(|s| (s.name, s.path, s.caps, s.restart))
            .collect();
        // The V0.8 kernel BACKGROUND table - tickd holds IPC only, flapd
        // holds nothing - and, third so their pids stay 2 and 3, V0.11's
        // inferd with IPC and reads (under init's /etc sandbox).
        assert_eq!(
            got,
            [
                ("tickd", "/bin/tickd", 0x2, Restart::Always),
                ("flapd", "/bin/flapd", 0x0, Restart::Always),
                ("inferd", "/bin/inferd", 0x12, Restart::Always),
                ("flakyd", "/bin/flakyd", 0x0, Restart::Always),
            ]
        );
        assert_eq!(CAP_IPC, 0x2);
        assert!(config.services().all(|s| s.long_running()));
        assert!(config
            .services()
            .all(|s| s.n_after == 0 && s.args.is_empty()));
        assert_eq!(order_of(SHIPPED), [0, 1, 2, 3]);
        // Also valid when handed over as raw bytes, as init reads it.
        assert_eq!(parse_bytes(SHIPPED.as_bytes()).unwrap(), config);
    }

    #[test]
    fn demo_fixture_carries_arguments() {
        let svc = only(DEMO);
        assert_eq!(svc.name, "demo");
        assert_eq!(svc.path, "/bin/args-probe");
        assert_eq!(svc.caps, 0);
        assert_eq!(svc.restart, Restart::Never);
        assert_eq!(svc.line, 1);
        assert_eq!(svc.args, "from-config");
        assert_eq!(svc.arg_tokens().collect::<Vec<_>>(), ["from-config"]);
        assert_eq!(block(&svc), b"from-config\0");
    }

    #[test]
    fn cycle_fixture_is_a_whole_file_cycle() {
        assert_eq!(fails(CYCLE), (0, Reason::Cycle));
    }

    #[test]
    fn badcap_fixture_fails_on_line_2() {
        assert_eq!(fails(BADCAP), (2, Reason::UnknownCapability));
        assert!(BADCAP.starts_with('#'));
    }

    #[test]
    fn defaults_are_no_caps_on_failure_no_deps_no_args() {
        let svc = only("service a /bin/x");
        assert_eq!(svc.caps, 0);
        assert_eq!(svc.restart, Restart::OnFailure);
        assert!(!svc.long_running());
        assert_eq!(svc.n_after, 0);
        assert!(svc.deps().is_empty());
        assert_eq!(svc.args, "");
        assert_eq!(svc.arg_tokens().count(), 0);
        assert_eq!(block(&svc), b"");
        // `caps=-` is the explicit spelling of the default.
        assert_eq!(only("service a /bin/x caps=-").caps, 0);
        // Options in any order.
        let svc =
            only_first("service b /sbin/y restart=never after=b2 caps=gui\nservice b2 /bin/z");
        assert_eq!(svc.restart, Restart::Never);
        assert_eq!(svc.caps, CAP_GUI);
        assert_eq!(svc.deps(), ["b2"]);
    }

    #[test]
    fn comments_blanks_whitespace_and_crlf() {
        let text = "# header\r\n\r\n   \t\n  # indented comment\nservice a /bin/x caps=ipc\r\n\
                    \tservice\t b  /bin/y\t restart=always   \r\n# trailing comment";
        let config = parse(text).unwrap();
        assert_eq!(config.len, 2);
        let a = config.get(0).unwrap();
        assert_eq!(
            (a.name, a.path, a.caps, a.line),
            ("a", "/bin/x", CAP_IPC, 5)
        );
        let b = config.get(1).unwrap();
        assert_eq!(
            (b.name, b.path, b.restart, b.line),
            ("b", "/bin/y", Restart::Always, 6)
        );
        assert_eq!(config.get(2), None);
        // The same file with LF endings parses identically (bar nothing).
        assert_eq!(parse(&text.replace("\r\n", "\n")).unwrap(), config);
        // Empty and comment-only files are valid and define nothing.
        for empty in ["", "\n", "\r\n", "# nothing\n", "#"] {
            let c = parse(empty).unwrap();
            assert_eq!(c.len, 0);
            assert_eq!(c.order().unwrap().1, 0);
        }
        // Line numbers count every line, blank and comment included.
        assert_eq!(fails("\n#\r\n\nbogus"), (4, Reason::UnknownDirective));
        // A stray CR is not a line ending: it sticks to the token.
        assert_eq!(
            fails("service a /bin/x\rservice b /bin/y"),
            (1, Reason::BadPath)
        );
        assert_eq!(fails("service a /bin/x\r"), (1, Reason::BadPath));
        // `#` starts a comment only as the first token.
        assert_eq!(fails("service a /bin/x # note"), (1, Reason::UnknownOption));
        assert_eq!(
            only_first("service a /bin/x -- #not-a-comment").args,
            "#not-a-comment"
        );
    }

    #[test]
    fn every_reason_at_its_line() {
        let two = "service ok /bin/ok\n";
        let cases: &[(&str, usize, Reason)] = &[
            ("# c\nservices a /bin/x", 2, Reason::UnknownDirective),
            ("Service a /bin/x", 1, Reason::UnknownDirective),
            ("# c\nservice", 2, Reason::MissingName),
            ("service  \t ", 1, Reason::MissingName),
            ("service a", 1, Reason::MissingPath),
            ("service A /bin/x", 1, Reason::BadName),
            ("service a /bin/x after=B", 1, Reason::BadName),
            ("service a /bin/x after=", 1, Reason::BadName),
            ("service a /bin/x after=b,,c", 1, Reason::BadName),
            (
                "service a /bin/x\nservice a /bin/y",
                2,
                Reason::DuplicateName,
            ),
            ("service a bin/x", 1, Reason::BadPath),
            ("service a /usr/bin/x", 1, Reason::BadPath),
            ("service a /bin/x capz=-", 1, Reason::UnknownOption),
            ("service a /bin/x caps", 1, Reason::UnknownOption),
            ("service a /bin/x -x", 1, Reason::UnknownOption),
            ("service a /bin/x caps=- caps=-", 1, Reason::DuplicateOption),
            (
                "service a /bin/x restart=never restart=never",
                1,
                Reason::DuplicateOption,
            ),
            (
                "service a /bin/x after=b after=b\nservice b /bin/y",
                1,
                Reason::DuplicateOption,
            ),
            ("service a /bin/x caps=root", 1, Reason::UnknownCapability),
            // V0.11: console-only authority is not a service's to have.
            (
                "service a /bin/x caps=sys_view",
                1,
                Reason::ConsoleOnlyCapability,
            ),
            (
                "service a /bin/x caps=ipc,propose",
                1,
                Reason::ConsoleOnlyCapability,
            ),
            ("service a /bin/x restart=sometimes", 1, Reason::BadRestart),
            ("service a /bin/x after=b,c,d,e,f", 1, Reason::TooManyDeps),
            ("service a /bin/x --", 1, Reason::BadArgs),
            ("service a /bin/x -- \t ", 1, Reason::BadArgs),
            ("service a /bin/x -- caf\u{e9}", 1, Reason::BadArgs),
            ("service a /bin/x -- ok \u{7}", 1, Reason::BadArgs),
            (
                "service ok /bin/ok\nservice b /bin/x after=ghost",
                2,
                Reason::UnknownDependency,
            ),
            ("service a /bin/x after=a", 0, Reason::Cycle),
        ];
        let mut covered: Vec<Reason> = Vec::new();
        for (text, line, reason) in cases {
            assert_eq!(fails(text), (*line, *reason), "{text:?}");
            covered.push(*reason);
        }
        // Remaining reasons: the bounds (see the *_bound tests) and whole-file.
        let long = format!("{two}#{}", "x".repeat(MAX_LINE));
        assert_eq!(fails(&long), (2, Reason::LineTooLong));
        let nine: String = (0..9).map(|i| format!("service s{i} /bin/x\n")).collect();
        assert_eq!(fails(&nine), (9, Reason::TooManyServices));
        assert_eq!(fails(&"#".repeat(MAX_FILE + 1)), (0, Reason::FileTooLarge));
        let e = parse_bytes(b"service a /bin/\xFF").unwrap_err();
        assert_eq!((e.line, e.reason), (0, Reason::NotUtf8));
        covered.extend([
            Reason::LineTooLong,
            Reason::TooManyServices,
            Reason::FileTooLarge,
            Reason::NotUtf8,
        ]);
        for reason in Reason::ALL {
            assert!(covered.contains(&reason), "{} untested", reason.name());
        }
    }

    #[test]
    fn line_numbers_and_first_error_wins() {
        // The first bad line is reported even when a later line is also bad.
        assert_eq!(
            fails("service a /bin/x\nservice b /bin/y caps=nope\nservice B /bin/z"),
            (2, Reason::UnknownCapability)
        );
        // Syntax errors anywhere beat an unknown dependency on an earlier line.
        assert_eq!(
            fails("service a /bin/x after=ghost\nservice b /bin/y restart=x"),
            (2, Reason::BadRestart)
        );
        // Unknown dependency is reported at the first referencing line.
        assert_eq!(
            fails("service a /bin/x\nservice b /bin/y after=a,ghost\nservice c /bin/z after=ghost"),
            (2, Reason::UnknownDependency)
        );
        // An unknown dependency outranks a cycle elsewhere.
        assert_eq!(
            fails("service a /bin/x after=b\nservice b /bin/y after=a\nservice c /bin/z after=q"),
            (3, Reason::UnknownDependency)
        );
        // Checks run left to right within a line.
        assert_eq!(fails("service A bad/path"), (1, Reason::BadName));
        assert_eq!(
            fails("service a /bin/x\nservice a bad/path"),
            (2, Reason::DuplicateName)
        );
        assert_eq!(
            fails("service a /bin/x caps=root restart=x"),
            (1, Reason::UnknownCapability)
        );
    }

    #[test]
    fn service_count_bound_is_exact() {
        let eight: String = (0..8).map(|i| format!("service s{i} /bin/x\n")).collect();
        let config = parse(&eight).unwrap();
        assert_eq!(config.len, MAX_SERVICES);
        assert_eq!(order_of(&eight), [0, 1, 2, 3, 4, 5, 6, 7]);
        let nine = format!("{eight}service s8 /bin/x\n");
        assert_eq!(fails(&nine), (9, Reason::TooManyServices));
        // The ninth `service` line is refused whatever else is on it.
        assert_eq!(
            fails(&format!("{eight}service")),
            (9, Reason::TooManyServices)
        );
        // Comments do not count.
        assert!(parse(&format!("{eight}# more\n\n")).is_ok());
    }

    #[test]
    fn dependency_count_bound_is_exact() {
        let defs = "service a /bin/x\nservice b /bin/x\nservice c /bin/x\nservice d /bin/x\nservice e /bin/x\n";
        let four = format!("{defs}service z /bin/x after=a,b,c,d");
        let config = parse(&four).unwrap();
        assert_eq!(config.get(5).unwrap().deps(), ["a", "b", "c", "d"]);
        assert_eq!(order_of(&four), [0, 1, 2, 3, 4, 5]);
        let five = format!("{defs}service z /bin/x after=a,b,c,d,e");
        assert_eq!(fails(&five), (6, Reason::TooManyDeps));
        assert_eq!(MAX_DEPS, 4);
    }

    #[test]
    fn line_length_bound_is_exact() {
        // `service a /bin/x -- ` is 20 bytes; pad the argument to the bound.
        let at = format!("service a /bin/x -- {}", "y".repeat(MAX_LINE - 20));
        assert_eq!(at.len(), MAX_LINE);
        assert_eq!(only(&at).args.len(), MAX_LINE - 20);
        let over = format!("{at}y");
        assert_eq!(fails(&over), (1, Reason::LineTooLong));
        // The terminator is not part of the line.
        assert!(parse(&format!("{at}\r\n{at}\n").replacen("service a", "service b", 1)).is_ok());
        // Comments and blank lines obey the same bound.
        let comment = format!("#{}", " ".repeat(MAX_LINE - 1));
        assert!(parse(&comment).is_ok());
        assert_eq!(fails(&format!("\n{comment} ")), (2, Reason::LineTooLong));
    }

    #[test]
    fn name_length_bound_is_exact() {
        let fifteen = "abcdefghijklm-9";
        assert_eq!(fifteen.len(), MAX_NAME);
        assert_eq!(only(&format!("service {fifteen} /bin/x")).name, fifteen);
        assert_eq!(
            fails("service abcdefghijklm-90 /bin/x"),
            (1, Reason::BadName)
        );
        // Dependency names follow the same rule.
        let dep_ok = format!("service a /bin/x after={fifteen}\nservice {fifteen} /bin/y");
        assert!(parse(&dep_ok).is_ok());
        assert_eq!(
            fails("service a /bin/x after=abcdefghijklm-90"),
            (1, Reason::BadName)
        );
        for bad in ["a_b", "a.b", "\u{e9}", "a/b", "a=b", "a,b"] {
            assert_eq!(
                fails(&format!("service {bad} /bin/x")),
                (1, Reason::BadName),
                "{bad}"
            );
        }
        assert!(parse("service 0-9 /bin/x").is_ok());
    }

    #[test]
    fn path_length_bound_and_rules() {
        let at = format!("/bin/{}", "p".repeat(MAX_PATH - 5));
        assert_eq!(at.len(), MAX_PATH);
        assert_eq!(only(&format!("service a {at}")).path, at);
        assert_eq!(fails(&format!("service a {at}p")), (1, Reason::BadPath));
        let sbin = format!("/sbin/{}", "q".repeat(MAX_PATH - 6));
        assert_eq!(only(&format!("service a {sbin}")).path, sbin);
        assert_eq!(fails(&format!("service a {sbin}q")), (1, Reason::BadPath));
        for good in [
            "/bin/init",
            "/sbin/init",
            "/bin/A-z.0_9",
            "/bin/.hidden",
            "/bin/a.b",
        ] {
            assert_eq!(only(&format!("service a {good}")).path, good);
        }
        for bad in [
            "/bin/",
            "/sbin/",
            "/bin",
            "/bin/.",
            "/bin/..",
            "/bin/a..b",
            "/bin//x",
            "//bin/x",
            "/bin/x/y",
            "/boot/x",
            "/BIN/x",
            "bin/x",
            "/bin/x+y",
            "/bin/x\u{e9}",
            "/etc/init.conf",
        ] {
            assert_eq!(
                fails(&format!("service a {bad}")),
                (1, Reason::BadPath),
                "{bad}"
            );
        }
    }

    #[test]
    fn file_size_bound_is_exact() {
        // 1024 bytes: a service line plus comment padding.
        let head = "service a /bin/x\n";
        let mut text = String::from(head);
        while text.len() < MAX_FILE {
            let room = (MAX_FILE - text.len()).min(MAX_LINE + 1);
            if room >= 2 {
                text.push('#');
                text.push_str(&" ".repeat(room - 2));
                text.push('\n');
            } else {
                text.push('\n');
            }
        }
        assert_eq!(text.len(), MAX_FILE);
        assert_eq!(parse(&text).unwrap().len, 1);
        assert_eq!(parse_bytes(text.as_bytes()).unwrap().len, 1);
        text.push('\n');
        assert_eq!(fails(&text), (0, Reason::FileTooLarge));
        let e = parse_bytes(text.as_bytes()).unwrap_err();
        assert_eq!((e.line, e.reason), (0, Reason::FileTooLarge));
        // Size is checked before encoding: an oversized non-UTF-8 file is too large.
        let e = parse_bytes(&[0xFF; MAX_FILE + 1]).unwrap_err();
        assert_eq!(e.reason, Reason::FileTooLarge);
        let e = parse_bytes(&[0xFF; MAX_FILE]).unwrap_err();
        assert_eq!((e.line, e.reason), (0, Reason::NotUtf8));
        // Non-UTF-8 is refused even inside a comment.
        let e = parse_bytes(b"# \xC3\n").unwrap_err();
        assert_eq!((e.line, e.reason), (0, Reason::NotUtf8));
        assert!(parse_bytes("# caf\u{e9}\n".as_bytes()).is_ok());
    }

    #[test]
    fn argument_count_bound_is_exact() {
        let sixteen = format!("service a /bin/x --{}", " a".repeat(progargs::MAX_ARGS));
        let svc = only(&sixteen);
        assert_eq!(svc.arg_tokens().count(), progargs::MAX_ARGS);
        assert_eq!(block(&svc), b"a\0".repeat(progargs::MAX_ARGS));
        let seventeen = format!("service a /bin/x --{}", " a".repeat(progargs::MAX_ARGS + 1));
        assert_eq!(fails(&seventeen), (1, Reason::BadArgs));
        // Separators between arguments collapse; options after `--` are arguments.
        let svc = only("service a /bin/x caps=ipc --  one\t\ttwo  caps=gui -- ");
        assert_eq!(svc.args, "one\t\ttwo  caps=gui --");
        assert_eq!(
            svc.arg_tokens().collect::<Vec<_>>(),
            ["one", "two", "caps=gui", "--"]
        );
        assert_eq!(block(&svc), b"one\0two\0caps=gui\0--\0");
        assert_eq!(svc.caps, CAP_IPC);
        // encode_args refuses a hand-built service with too many arguments.
        let mut hand = svc;
        let many = " x".repeat(progargs::MAX_ARGS + 1);
        hand.args = &many;
        let mut out = [0u8; progargs::MAX_BLOCK];
        assert_eq!(hand.encode_args(&mut out), Err(progargs::ArgError::TooMany));
    }

    #[test]
    fn capability_lists() {
        assert_eq!(only("service a /bin/x caps=ipc").caps, CAP_IPC);
        assert_eq!(
            only("service a /bin/x caps=spawn,ipc,fs_read").caps,
            CAP_SPAWN | CAP_IPC | CAP_FS_READ
        );
        assert_eq!(only("service a /bin/x caps=ipc,ipc").caps, CAP_IPC);
        let all = "service a /bin/x caps=spawn,ipc,gui,dev,fs_read,audio,sys_admin,fs_write,network,proc_control,service";
        assert_eq!(only(all).caps, caps::CAP_DELEGABLE);
        for bad in [
            "caps=",
            "caps=,",
            "caps=ipc,",
            "caps=,ipc",
            "caps=-,ipc",
            "caps=IPC",
            "caps=--",
            "caps=ipc,\u{3000}gui",
            "caps=all",
        ] {
            assert_eq!(
                fails(&format!("service a /bin/x {bad}")),
                (1, Reason::UnknownCapability),
                "{bad}"
            );
        }
    }

    #[test]
    fn restart_policies() {
        assert_eq!(
            only("service a /bin/x restart=always").restart,
            Restart::Always
        );
        assert_eq!(
            only("service a /bin/x restart=on-failure").restart,
            Restart::OnFailure
        );
        assert_eq!(
            only("service a /bin/x restart=never").restart,
            Restart::Never
        );
        for bad in [
            "restart=",
            "restart=Always",
            "restart=on_failure",
            "restart=always,never",
        ] {
            assert_eq!(
                fails(&format!("service a /bin/x {bad}")),
                (1, Reason::BadRestart),
                "{bad}"
            );
        }
    }

    #[test]
    fn order_is_dependencies_then_lowest_index() {
        // c needs b, b needs a, d is free. At each step the lowest ready index
        // starts: a(1), then b(2), then c(0) — which became ready before d(3).
        let text = "service c /bin/x after=b\nservice a /bin/x\nservice b /bin/x after=a\nservice d /bin/x";
        assert_eq!(order_of(text), [1, 2, 0, 3]);
        // Deterministic.
        assert_eq!(order_of(text), order_of(text));
        // Forward references and diamonds.
        let diamond = "service top /bin/x after=l,r\nservice l /bin/x after=base\n\
                       service r /bin/x after=base\nservice base /bin/x";
        assert_eq!(order_of(diamond), [3, 1, 2, 0]);
        // Without dependencies the order is file order.
        assert_eq!(
            order_of("service z /bin/x\nservice y /bin/x\nservice x /bin/x"),
            [0, 1, 2]
        );
    }

    #[test]
    fn cycles_are_refused_at_line_0() {
        for text in [
            "service a /bin/x after=a",
            "service a /bin/x after=b\nservice b /bin/x after=a",
            "service a /bin/x after=c\nservice b /bin/x after=a\nservice c /bin/x after=b",
            "service ok /bin/x\nservice a /bin/x after=ok,b\nservice b /bin/x after=a",
        ] {
            assert_eq!(fails(text), (0, Reason::Cycle), "{text:?}");
        }
    }

    #[test]
    fn order_on_a_hand_built_config() {
        let mut config = parse("service a /bin/x\nservice b /bin/x after=a").unwrap();
        // A hole is skipped, and a dependency on it no longer resolves.
        config.services[0] = None;
        assert_eq!(
            config.order().unwrap_err(),
            ConfigError {
                line: 2,
                reason: Reason::UnknownDependency
            }
        );
        config.services[1].as_mut().unwrap().n_after = 0;
        let (order, n) = config.order().unwrap();
        assert_eq!(&order[..n], [1]);
        config.services[1].as_mut().unwrap().n_after = MAX_DEPS + 1;
        assert_eq!(config.order().unwrap_err().reason, Reason::TooManyDeps);
        config.len = MAX_SERVICES + 1;
        assert_eq!(
            config.order().unwrap_err(),
            ConfigError {
                line: 0,
                reason: Reason::TooManyServices
            }
        );
    }

    #[test]
    fn reason_names_are_stable_and_distinct() {
        let names: Vec<&str> = Reason::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(
            names,
            [
                "unknown_directive",
                "missing_name",
                "missing_path",
                "bad_name",
                "duplicate_name",
                "bad_path",
                "unknown_option",
                "duplicate_option",
                "unknown_capability",
                "console_only_capability",
                "bad_restart",
                "too_many_deps",
                "bad_args",
                "line_too_long",
                "too_many_services",
                "unknown_dependency",
                "cycle",
                "file_too_large",
                "not_utf8",
            ]
        );
        for n in &names {
            assert!(
                n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{n}"
            );
        }
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), Reason::ALL.len());
    }
}
