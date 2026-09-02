//! Serial marker grammar — the single source of truth shared by the kernel
//! (emission) and the QEMU test harness (assertion).
//!
//! Grammar (one marker per line, anywhere in serial output):
//!
//! ```text
//! [ITISYOU:B010] human readable text        stage marker
//! [ITISYOU:PANIC] panic message             kernel panic
//! [ITISYOU:SELFTEST] pass=12 fail=0        selftest suite summary
//! [ITISYOU:TEST] name=heap_alloc result=pass   individual test event
//! [ITISYOU:INFO] key=value free text        structured information
//! [ITISYOU:MODE] interactive|selftest       kernel run mode
//! ```

use crate::stage::Stage;

pub const PREFIX: &str = "[ITISYOU:";

/// A parsed marker line (borrowing from the input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker<'a> {
    Stage(Stage, &'a str),
    Panic(&'a str),
    Selftest { pass: u32, fail: u32 },
    Test { name: &'a str, pass: bool },
    Info(&'a str),
    Mode(&'a str),
}

/// Parse a serial line into a marker, if it contains one.
pub fn parse_line(line: &str) -> Option<Marker<'_>> {
    let start = line.find(PREFIX)?;
    let rest = &line[start + PREFIX.len()..];
    let close = rest.find(']')?;
    let tag = &rest[..close];
    let payload = rest[close + 1..].trim();

    match tag {
        "PANIC" => Some(Marker::Panic(payload)),
        "INFO" => Some(Marker::Info(payload)),
        "MODE" => Some(Marker::Mode(payload)),
        "SELFTEST" => {
            let pass = field(payload, "pass")?.parse().ok()?;
            let fail = field(payload, "fail")?.parse().ok()?;
            Some(Marker::Selftest { pass, fail })
        }
        "TEST" => {
            let name = field(payload, "name")?;
            let result = field(payload, "result")?;
            Some(Marker::Test {
                name,
                pass: result == "pass",
            })
        }
        code => Stage::from_code(code).map(|s| Marker::Stage(s, payload)),
    }
}

/// Extract `key=value` fields from a payload (values must not contain spaces).
fn field<'a>(payload: &'a str, key: &str) -> Option<&'a str> {
    payload.split_whitespace().find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k == key).then_some(v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stage_marker() {
        let m = parse_line("[ITISYOU:B010] kernel entry reached in 64-bit mode");
        assert_eq!(
            m,
            Some(Marker::Stage(
                Stage::B010KernelEntry,
                "kernel entry reached in 64-bit mode"
            ))
        );
    }

    #[test]
    fn parses_marker_with_leading_noise() {
        // QEMU sometimes interleaves output; markers must still be found.
        let m = parse_line("garbage\x1b[0m[ITISYOU:PANIC] oh no");
        assert_eq!(m, Some(Marker::Panic("oh no")));
    }

    #[test]
    fn parses_selftest_summary() {
        let m = parse_line("[ITISYOU:SELFTEST] pass=12 fail=0");
        assert_eq!(m, Some(Marker::Selftest { pass: 12, fail: 0 }));
    }

    #[test]
    fn parses_test_event() {
        let m = parse_line("[ITISYOU:TEST] name=heap_alloc result=pass");
        assert_eq!(
            m,
            Some(Marker::Test {
                name: "heap_alloc",
                pass: true
            })
        );
        let m = parse_line("[ITISYOU:TEST] name=heap_alloc result=fail");
        assert_eq!(
            m,
            Some(Marker::Test {
                name: "heap_alloc",
                pass: false
            })
        );
    }

    #[test]
    fn rejects_malformed_lines() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("[ITISYOU:B010 no close bracket"), None);
        assert_eq!(parse_line("[ITISYOU:B999] unknown stage"), None);
        assert_eq!(parse_line("[ITISYOU:SELFTEST] pass=x fail=0"), None);
        assert_eq!(parse_line("plain log line"), None);
    }

    #[test]
    fn mode_marker() {
        assert_eq!(
            parse_line("[ITISYOU:MODE] selftest"),
            Some(Marker::Mode("selftest"))
        );
    }
}
