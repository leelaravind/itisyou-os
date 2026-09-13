//! The diagnostic model's training data (V0.11, MODEL11-001, ADR-0024):
//! hand-written scenario definitions in `ai/scenarios.txt`, and the
//! deterministic generator that turns them into labelled examples.
//!
//! Every example is synthetic. A scenario names the conditions that hold in
//! it (none at all is a healthy system) and, for each of the 16 features of
//! [`crate::sysview::features`], the inclusive range its examples draw from;
//! the generator cycles through the scenarios and draws every feature
//! uniformly from its range with splitmix64 from a fixed seed. The model is
//! therefore exactly as good as these ranges are realistic — they encode the
//! author's assumptions, and the docs say so.
//!
//! ```text
//! # comment
//! scenario <conditions> <lo>..<hi> ×16
//! ```
//!
//! `<conditions>` is `healthy` or a comma-separated list of distinct
//! condition names ([`Condition::name`]) in any order. Ranges are plain
//! decimals with `0 <= lo <= hi <= 1000`. Lines end in LF or CRLF (a checkout
//! on Windows converts them) and may be at most [`MAX_LINE`] bytes; tokens are
//! separated by spaces or tabs.

use crate::sysview::{FEATURES, FEATURE_MAX};

/// The conditions the detectors recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    ServiceFailed = 0,
    SchedulerPaused = 1,
    DenialBurst = 2,
}

/// Number of conditions (one detector each).
pub const CONDITIONS: usize = 3;

impl Condition {
    pub const ALL: [Condition; CONDITIONS] = [
        Condition::ServiceFailed,
        Condition::SchedulerPaused,
        Condition::DenialBurst,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Condition::ServiceFailed => "service_failed",
            Condition::SchedulerPaused => "scheduler_paused",
            Condition::DenialBurst => "denial_burst",
        }
    }

    pub fn from_name(s: &str) -> Option<Condition> {
        Condition::ALL.iter().copied().find(|c| c.name() == s)
    }

    pub const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of conditions, one bit per [`Condition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Conditions(pub u8);

impl Conditions {
    pub const NONE: Conditions = Conditions(0);

    pub const fn has(self, c: Condition) -> bool {
        self.0 & c.bit() != 0
    }

    pub const fn with(self, c: Condition) -> Conditions {
        Conditions(self.0 | c.bit())
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Write the set the way the scenario file and the agent's markers spell
    /// it: names in [`Condition::ALL`] order, comma-separated, or `none`.
    pub fn write(self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
        if self.is_empty() {
            return out.write_str("none");
        }
        let mut first = true;
        for c in Condition::ALL {
            if self.has(c) {
                if !first {
                    out.write_char(',')?;
                }
                out.write_str(c.name())?;
                first = false;
            }
        }
        Ok(())
    }
}

/// One scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    pub conditions: Conditions,
    pub ranges: [(i32, i32); FEATURES],
}

impl Scenario {
    pub const EMPTY: Scenario = Scenario {
        conditions: Conditions::NONE,
        ranges: [(0, 0); FEATURES],
    };
}

/// Most scenarios a file may define.
pub const MAX_SCENARIOS: usize = 32;
/// Longest line, terminator excluded.
pub const MAX_LINE: usize = 256;

/// Why a scenario file was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    UnknownDirective,
    UnknownCondition,
    RepeatedCondition,
    /// `healthy` combined with a condition.
    HealthyWithCondition,
    MissingRange,
    BadRange,
    ExtraField,
    LineTooLong,
    TooManyScenarios,
    /// A condition no scenario includes, or no healthy scenario: a detector
    /// needs both sides.
    Uncovered,
}

impl Reason {
    pub const fn name(self) -> &'static str {
        match self {
            Reason::UnknownDirective => "unknown_directive",
            Reason::UnknownCondition => "unknown_condition",
            Reason::RepeatedCondition => "repeated_condition",
            Reason::HealthyWithCondition => "healthy_with_condition",
            Reason::MissingRange => "missing_range",
            Reason::BadRange => "bad_range",
            Reason::ExtraField => "extra_field",
            Reason::LineTooLong => "line_too_long",
            Reason::TooManyScenarios => "too_many_scenarios",
            Reason::Uncovered => "uncovered",
        }
    }
}

/// A refusal and its 1-based line (0: the file as a whole).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioError {
    pub line: usize,
    pub reason: Reason,
}

/// A plain decimal in `0..=FEATURE_MAX`: digits only, no sign.
fn value(t: &str) -> Option<i32> {
    if t.is_empty() || t.len() > 4 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v: i32 = t.parse().ok()?;
    (v <= FEATURE_MAX).then_some(v)
}

fn conditions(t: &str) -> Result<Conditions, Reason> {
    if t == "healthy" {
        return Ok(Conditions::NONE);
    }
    let mut set = Conditions::NONE;
    for name in t.split(',') {
        if name == "healthy" {
            return Err(Reason::HealthyWithCondition);
        }
        let c = Condition::from_name(name).ok_or(Reason::UnknownCondition)?;
        if set.has(c) {
            return Err(Reason::RepeatedCondition);
        }
        set = set.with(c);
    }
    Ok(set)
}

/// Parse a scenario file into `out`; returns how many scenarios it defines.
pub fn parse(text: &str, out: &mut [Scenario; MAX_SCENARIOS]) -> Result<usize, ScenarioError> {
    let mut n = 0;
    for (i, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let err = |reason| ScenarioError {
            line: i + 1,
            reason,
        };
        if line.len() > MAX_LINE {
            return Err(err(Reason::LineTooLong));
        }
        let mut tok = line.split([' ', '\t']).filter(|t| !t.is_empty());
        let Some(first) = tok.next() else {
            continue;
        };
        if first.starts_with('#') {
            continue;
        }
        if first != "scenario" {
            return Err(err(Reason::UnknownDirective));
        }
        let set = conditions(tok.next().ok_or(err(Reason::UnknownCondition))?).map_err(err)?;
        let mut ranges = [(0, 0); FEATURES];
        for r in ranges.iter_mut() {
            let t = tok.next().ok_or(err(Reason::MissingRange))?;
            let (lo, hi) = t.split_once("..").ok_or(err(Reason::BadRange))?;
            let (Some(lo), Some(hi)) = (value(lo), value(hi)) else {
                return Err(err(Reason::BadRange));
            };
            if lo > hi {
                return Err(err(Reason::BadRange));
            }
            *r = (lo, hi);
        }
        if tok.next().is_some() {
            return Err(err(Reason::ExtraField));
        }
        if n == MAX_SCENARIOS {
            return Err(err(Reason::TooManyScenarios));
        }
        out[n] = Scenario {
            conditions: set,
            ranges,
        };
        n += 1;
    }
    let file = ScenarioError {
        line: 0,
        reason: Reason::Uncovered,
    };
    let s = &out[..n];
    if !s.iter().any(|x| x.conditions.is_empty()) {
        return Err(file);
    }
    for c in Condition::ALL {
        if !s.iter().any(|x| x.conditions.has(c)) {
            return Err(file);
        }
    }
    Ok(n)
}

/// splitmix64: a small, well-mixed, fully specified PRNG (Vigna).
#[derive(Debug, Clone)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `lo..=hi` (`lo <= hi`, both within `0..=1000`).
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        let span = (hi - lo + 1) as u64;
        lo + (self.next_u64() % span) as i32
    }
}

/// One labelled example.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Example {
    pub x: [i32; FEATURES],
    pub y: Conditions,
}

impl Example {
    pub const ZERO: Example = Example {
        x: [0; FEATURES],
        y: Conditions::NONE,
    };
}

/// The `k`-th example of a data set: the scenarios are cycled in order and
/// every feature is drawn uniformly from its range.
pub fn example(scenarios: &[Scenario], rng: &mut SplitMix64, k: usize) -> Example {
    let s = &scenarios[k % scenarios.len()];
    let mut x = [0i32; FEATURES];
    for (f, v) in x.iter_mut().enumerate() {
        *v = rng.range(s.ranges[f].0, s.ranges[f].1);
    }
    Example { x, y: s.conditions }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(conds: &str) -> String {
        format!("scenario {conds} {}", "0..1 ".repeat(FEATURES))
    }

    fn file(lines: &[&str]) -> String {
        lines.iter().map(|c| line(c)).collect::<Vec<_>>().join("\n")
    }

    const COVER: [&str; 4] = [
        "healthy",
        "service_failed",
        "scheduler_paused",
        "denial_burst",
    ];

    fn parse_str(text: &str) -> Result<(usize, [Scenario; MAX_SCENARIOS]), ScenarioError> {
        let mut out = [Scenario::EMPTY; MAX_SCENARIOS];
        parse(text, &mut out).map(|n| (n, out))
    }

    #[test]
    fn the_shipped_file_parses_and_covers_every_condition() {
        let (n, s) = parse_str(include_str!("../../../ai/scenarios.txt")).unwrap();
        assert!(n >= 4);
        // CRLF, as a Windows checkout would have it, parses to the same.
        let crlf = include_str!("../../../ai/scenarios.txt").replace('\n', "\r\n");
        let (n2, s2) = parse_str(&crlf).unwrap();
        assert_eq!((n, &s[..n]), (n2, &s2[..n2]));
    }

    #[test]
    fn condition_sets_parse_in_any_order_and_print_canonically() {
        let (_, s) = parse_str(&file(&[
            "healthy",
            "denial_burst,service_failed",
            "scheduler_paused",
        ]))
        .unwrap();
        let set = s[1].conditions;
        assert!(set.has(Condition::ServiceFailed) && set.has(Condition::DenialBurst));
        assert!(!set.has(Condition::SchedulerPaused));
        let mut out = String::new();
        set.write(&mut out).unwrap();
        assert_eq!(out, "service_failed,denial_burst");
        let mut out = String::new();
        Conditions::NONE.write(&mut out).unwrap();
        assert_eq!(out, "none");
    }

    #[test]
    fn every_refusal_names_its_line_and_reason() {
        let cases: [(String, (usize, Reason)); 11] = [
            (
                format!("{}\nthing", file(&COVER)),
                (5, Reason::UnknownDirective),
            ),
            (
                format!("# c\n{}", line("nope")),
                (2, Reason::UnknownCondition),
            ),
            (
                line("service_failed,service_failed"),
                (1, Reason::RepeatedCondition),
            ),
            (
                line("healthy,service_failed"),
                (1, Reason::HealthyWithCondition),
            ),
            ("scenario healthy 0..1".into(), (1, Reason::MissingRange)),
            (
                format!("scenario healthy 5..1 {}", "0..1 ".repeat(15)),
                (1, Reason::BadRange),
            ),
            (
                format!("scenario healthy 0..1001 {}", "0..1 ".repeat(15)),
                (1, Reason::BadRange),
            ),
            (
                format!("scenario healthy +1..2 {}", "0..1 ".repeat(15)),
                (1, Reason::BadRange),
            ),
            (format!("{} 0..1", line("healthy")), (1, Reason::ExtraField)),
            (
                format!("# {}", "x".repeat(MAX_LINE)),
                (1, Reason::LineTooLong),
            ),
            (
                file(&["healthy", "service_failed", "scheduler_paused"]),
                (0, Reason::Uncovered),
            ),
        ];
        for (text, (at, why)) in cases {
            let e = parse_str(&text).unwrap_err();
            assert_eq!((e.line, e.reason), (at, why), "{text:?}");
        }
        // No healthy scenario is also uncovered: a detector needs negatives.
        let e = parse_str(&file(&[
            "service_failed",
            "scheduler_paused",
            "denial_burst",
        ]))
        .unwrap_err();
        assert_eq!(e.reason, Reason::Uncovered);
    }

    #[test]
    fn the_limits_are_exact() {
        // A line of exactly MAX_LINE bytes is fine; one more is not.
        let long_ok = format!("#{}", "x".repeat(MAX_LINE - 1));
        assert!(parse_str(&format!("{long_ok}\n{}", file(&COVER))).is_ok());
        // 32 scenarios parse; the 33rd is refused on its own line.
        let mut lines: Vec<&str> = COVER.to_vec();
        lines.extend(core::iter::repeat_n("healthy", MAX_SCENARIOS - COVER.len()));
        assert_eq!(parse_str(&file(&lines)).unwrap().0, MAX_SCENARIOS);
        lines.push("healthy");
        let e = parse_str(&file(&lines)).unwrap_err();
        assert_eq!(
            (e.line, e.reason),
            (MAX_SCENARIOS + 1, Reason::TooManyScenarios)
        );
    }

    #[test]
    fn the_generator_is_deterministic_and_stays_in_range() {
        let mut s = [Scenario::EMPTY; MAX_SCENARIOS];
        let n = parse(include_str!("../../../ai/scenarios.txt"), &mut s).unwrap();
        let mut a = SplitMix64(7);
        let mut b = SplitMix64(7);
        for k in 0..500 {
            let (ea, eb) = (example(&s[..n], &mut a, k), example(&s[..n], &mut b, k));
            assert_eq!(ea, eb);
            let sc = &s[k % n];
            assert_eq!(ea.y, sc.conditions);
            for f in 0..FEATURES {
                assert!((sc.ranges[f].0..=sc.ranges[f].1).contains(&ea.x[f]));
            }
        }
        // splitmix64's published first outputs for seed 0.
        let mut r = SplitMix64(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }
}
