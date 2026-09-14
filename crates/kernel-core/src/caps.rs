//! Capability model (V0.7): explicit, least-privilege authorities a process
//! must hold before the kernel performs a privileged operation on its behalf.
//!
//! Pure logic (bit definitions, parsing, formatting) shared by the kernel's
//! enforcement points, the app-manifest parser, and host tests. The basic
//! runtime (write/exit/yield/getpid) needs no capability; everything else is
//! default-deny: a process holds only what it was explicitly granted, and a
//! child can never hold more than its parent (delegation intersects).

/// Spawn + wait on child processes.
pub const CAP_SPAWN: u64 = 1 << 0;
/// IPC message channels (send/recv).
pub const CAP_IPC: u64 = 1 << 1;
/// GUI syscalls (window create/fill/text/present).
pub const CAP_GUI: u64 = 1 << 2;
/// Device-table queries (devinfo).
pub const CAP_DEV: u64 = 1 << 3;
/// Filesystem reads (subject to the process's sandbox path prefixes).
pub const CAP_FS_READ: u64 = 1 << 4;
/// Audio output (reserved — no userspace audio syscall exists yet).
pub const CAP_AUDIO: u64 = 1 << 5;
/// System administration (reserved — service/package control from userspace).
pub const CAP_SYS_ADMIN: u64 = 1 << 6;
/// Capability-scoped userspace filesystem mutation.
pub const CAP_FS_WRITE: u64 = 1 << 7;
/// Capability-scoped network socket and packet operations.
pub const CAP_NETWORK: u64 = 1 << 8;
/// Process teardown/control beyond spawning and waiting.
pub const CAP_PROC_CONTROL: u64 = 1 << 9;
/// Service lifecycle and service-RPC administration.
pub const CAP_SERVICE: u64 = 1 << 10;
/// Read the approved system view (V0.11, `sys_view`; SystemAdministration
/// READ). Console-granted only — see [`CAP_CONSOLE_ONLY`].
pub const CAP_SYS_VIEW: u64 = 1 << 11;
/// File a proposal for the console to approve (V0.11, `propose`;
/// SystemAdministration USE). Console-granted only.
pub const CAP_PROPOSE: u64 = 1 << 12;

/// Number of IPC channels (`kernel::ipc`).
pub const IPC_CHANNELS: u64 = 8;
/// Where the channel bits start in a capability word.
const IPC_CHANNEL_SHIFT: u64 = 16;

/// The capability bit for IPC channel `channel` (V1.0, ADR-0025). `CAP_IPC`
/// reaches only the channels whose bits are set with it, and they form one
/// contiguous range — the scope of the process's IPC handle. Being bits of
/// the same word, they intersect on delegation like every other capability,
/// so a child reaches at most its parent's channels.
pub const fn cap_ipc_channel(channel: u64) -> u64 {
    1u64 << (IPC_CHANNEL_SHIFT + channel)
}

/// Every channel bit.
pub const CAP_IPC_CHANNELS_ALL: u64 = ((1u64 << IPC_CHANNELS) - 1) << IPC_CHANNEL_SHIFT;

/// The inference service's channels (`crate::infer`): reached only by a
/// grant that names them (`ipc:6-7`), never by plain `ipc` or a default set,
/// so no ordinary program can read what a diagnosis sends inferd or answer
/// in inferd's place.
pub const CAP_IPC_RESERVED: u64 =
    cap_ipc_channel(crate::infer::REQUEST_CHANNEL) | cap_ipc_channel(crate::infer::REPLY_CHANNEL);

/// The channels plain `ipc` grants: all but the reserved ones.
pub const CAP_IPC_DEFAULT: u64 = CAP_IPC_CHANNELS_ALL & !CAP_IPC_RESERVED;

/// Every currently defined capability bit.
pub const CAP_ALL_KNOWN: u64 = CAP_IPC_CHANNELS_ALL
    | CAP_SPAWN
    | CAP_IPC
    | CAP_GUI
    | CAP_DEV
    | CAP_FS_READ
    | CAP_AUDIO
    | CAP_SYS_ADMIN
    | CAP_FS_WRITE
    | CAP_NETWORK
    | CAP_PROC_CONTROL
    | CAP_SERVICE
    | CAP_SYS_VIEW
    | CAP_PROPOSE;

/// Authority only the kernel console grants, by naming it (V0.11,
/// ADR-0024): the approved view and the right to propose. It is in no default
/// set, a parent cannot pass it to a child ([`delegate`] drops it), and a
/// service definition or a package manifest cannot obtain it — so the agent
/// that holds it is always one the operator started on purpose.
pub const CAP_CONSOLE_ONLY: u64 = CAP_SYS_VIEW | CAP_PROPOSE;

/// Every bit a parent may pass on.
pub const CAP_DELEGABLE: u64 = CAP_ALL_KNOWN & !CAP_CONSOLE_ONLY;

/// The full "legacy" set granted to programs started directly by the trusted
/// kernel shell/selftest (pre-platform paths), so V0.2–V0.6 behavior is
/// unchanged. Platform-launched apps NEVER get this implicitly — they receive
/// only what their manifest requests intersected with the launcher's caps.
/// It excludes the console-only bits (V0.11): `run` without a caps list,
/// `rsh`, desktop apps and `pkg launch` never get the view or `propose`; nor
/// (V1.0, ADR-0025) the reserved IPC channels — only a grant naming them
/// reaches the inference service — nor the supervisor's `service` right,
/// which is checked for the whole class and so carries every channel with
/// it: a program reports services, or reaches the reserved channels, only
/// when the console, init's config or a manifest names it.
pub const CAP_LEGACY_FULL: u64 = CAP_DELEGABLE & !CAP_IPC_RESERVED & !CAP_SERVICE;

const NAMES: &[(&str, u64)] = &[
    ("spawn", CAP_SPAWN),
    ("ipc", CAP_IPC),
    ("gui", CAP_GUI),
    ("dev", CAP_DEV),
    ("fs_read", CAP_FS_READ),
    ("audio", CAP_AUDIO),
    ("sys_admin", CAP_SYS_ADMIN),
    ("fs_write", CAP_FS_WRITE),
    ("network", CAP_NETWORK),
    ("proc_control", CAP_PROC_CONTROL),
    ("service", CAP_SERVICE),
    ("sys_view", CAP_SYS_VIEW),
    ("propose", CAP_PROPOSE),
];

/// Parse error for a capability list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapParseError {
    /// A name in the list is not a defined capability. Unknown names are
    /// REJECTED, never silently ignored — otherwise a typo could turn into a
    /// grant the author never reviewed (or mask one they intended).
    Unknown,
    /// An `ipc:<a>[-<b>]` channel range that is malformed, reversed, beyond
    /// the last channel, or that — with the list's other `ipc` entries —
    /// does not form one contiguous range (V1.0).
    BadChannels,
}

/// Parse a comma-separated capability list (e.g. `"gui,fs_read"`). Empty
/// input means no capabilities (default deny). Whitespace around names is
/// tolerated; unknown names are errors.
///
/// IPC (V1.0, ADR-0025): `ipc` grants the default channels
/// ([`CAP_IPC_DEFAULT`]); `ipc:<a>` or `ipc:<a>-<b>` grants exactly channels
/// `a..=b`, which is the only way to reach the reserved ones.
pub fn parse(list: &str) -> Result<u64, CapParseError> {
    let mut caps = 0u64;
    for raw in list.split(',') {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        if name == "ipc" {
            caps |= CAP_IPC | CAP_IPC_DEFAULT;
            continue;
        }
        if let Some(range) = name.strip_prefix("ipc:") {
            caps |= CAP_IPC | parse_channels(range)?;
            continue;
        }
        match NAMES.iter().find(|(n, _)| *n == name) {
            Some((_, bit)) => caps |= bit,
            None => return Err(CapParseError::Unknown),
        }
    }
    if caps & CAP_IPC != 0 && ipc_channels(caps).is_none() {
        return Err(CapParseError::BadChannels);
    }
    Ok(caps)
}

/// `<a>` or `<a>-<b>`: plain decimal channel numbers, `a <= b < IPC_CHANNELS`.
fn parse_channels(range: &str) -> Result<u64, CapParseError> {
    let num = |s: &str| -> Result<u64, CapParseError> {
        // Plain decimal with no leading zero: one spelling per channel.
        if s.is_empty()
            || s.len() > 2
            || !s.bytes().all(|b| b.is_ascii_digit())
            || (s.len() > 1 && s.starts_with('0'))
        {
            return Err(CapParseError::BadChannels);
        }
        let n = s.bytes().fold(0u64, |n, b| n * 10 + u64::from(b - b'0'));
        if n >= IPC_CHANNELS {
            return Err(CapParseError::BadChannels);
        }
        Ok(n)
    };
    let (lo, hi) = match range.split_once('-') {
        Some((a, b)) => (num(a)?, num(b)?),
        None => {
            let n = num(range)?;
            (n, n)
        }
    };
    if lo > hi {
        return Err(CapParseError::BadChannels);
    }
    Ok((lo..=hi).fold(0, |bits, c| bits | cap_ipc_channel(c)))
}

/// The IPC channel range `bits` grants: `Some((lo, hi))` when `CAP_IPC` is set
/// and its channel bits form one contiguous run, `None` otherwise — no
/// channels, or a set that is not a range, which grants no IPC at all (the
/// handle's scope is a single range, and rounding a set up to one would widen
/// it).
pub fn ipc_channels(bits: u64) -> Option<(u64, u64)> {
    if bits & CAP_IPC == 0 {
        return None;
    }
    let set = (bits & CAP_IPC_CHANNELS_ALL) >> IPC_CHANNEL_SHIFT;
    if set == 0 {
        return None;
    }
    let lo = u64::from(set.trailing_zeros());
    let hi = 63 - u64::from(set.leading_zeros());
    let run = ((1u64 << (hi - lo + 1)) - 1) << lo;
    (set == run).then_some((lo, hi))
}

/// Human-readable name of a single capability bit (diagnostics/audit).
pub fn name_of(bit: u64) -> &'static str {
    NAMES
        .iter()
        .find(|(_, b)| *b == bit)
        .map(|(n, _)| *n)
        .unwrap_or("?")
}

/// Iterate the names of every capability set in `caps` (diagnostics).
pub fn names(caps: u64) -> impl Iterator<Item = &'static str> {
    NAMES
        .iter()
        .filter(move |(_, b)| caps & b != 0)
        .map(|(n, _)| *n)
}

/// A capability set written as a list [`parse`] reads back to the same bits
/// (V1.0): the names, with `ipc` spelled `ipc:<a>-<b>` unless its channels
/// are exactly the defaults. Channel bits that are not one range (no parsed
/// list produces them) are written as plain `ipc`.
pub struct Describe(pub u64);

impl core::fmt::Display for Describe {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (name, bit) in NAMES {
            if self.0 & bit == 0 {
                continue;
            }
            if !first {
                f.write_str(",")?;
            }
            first = false;
            match (*bit == CAP_IPC, ipc_channels(self.0)) {
                (true, Some((lo, hi))) if self.0 & CAP_IPC_CHANNELS_ALL != CAP_IPC_DEFAULT => {
                    write!(f, "ipc:{lo}-{hi}")?
                }
                _ => f.write_str(name)?,
            }
        }
        Ok(())
    }
}

/// Delegation rule: a child receives what it requested intersected with what
/// the parent actually holds. Never amplification, and never the
/// console-only bits (V0.11).
pub fn delegate(parent: u64, requested: u64) -> u64 {
    parent & requested & CAP_DELEGABLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lists() {
        assert_eq!(parse("").unwrap(), 0);
        assert_eq!(parse("gui").unwrap(), CAP_GUI);
        assert_eq!(parse("gui,fs_read").unwrap(), CAP_GUI | CAP_FS_READ);
        assert_eq!(
            parse(" spawn , ipc ").unwrap(),
            CAP_SPAWN | CAP_IPC | CAP_IPC_DEFAULT
        );
    }

    #[test]
    fn ipc_names_its_channels_and_the_reserved_ones_only_explicitly() {
        // Plain `ipc`: channels 0-5; the inference channels 6 and 7 are not
        // in it, nor in any default set.
        assert_eq!(ipc_channels(parse("ipc").unwrap()), Some((0, 5)));
        assert_eq!(CAP_LEGACY_FULL & CAP_IPC_RESERVED, 0);
        assert_eq!(ipc_channels(CAP_LEGACY_FULL), Some((0, 5)));
        assert_eq!(
            CAP_IPC_RESERVED,
            cap_ipc_channel(crate::infer::REQUEST_CHANNEL)
                | cap_ipc_channel(crate::infer::REPLY_CHANNEL)
        );
        // Named ranges.
        assert_eq!(ipc_channels(parse("ipc:6-7").unwrap()), Some((6, 7)));
        assert_eq!(ipc_channels(parse("ipc:2").unwrap()), Some((2, 2)));
        assert_eq!(ipc_channels(parse("ipc:0-7").unwrap()), Some((0, 7)));
        // Overlapping or adjacent entries join into one range.
        assert_eq!(ipc_channels(parse("ipc:2-3,ipc:4").unwrap()), Some((2, 4)));
        // Malformed, reversed, out of range, or not one range: refused.
        for bad in [
            "ipc:",
            "ipc:8",
            "ipc:3-2",
            "ipc:1-",
            "ipc:-1",
            "ipc:a",
            "ipc:+1",
            "ipc:007",
            "ipc:07",
            "ipc:2-03",
            "ipc:2,ipc:6",
            "ipc: 2",
            "ipc:0-8",
        ] {
            assert_eq!(parse(bad), Err(CapParseError::BadChannels), "{bad}");
        }
        // Channel bits without CAP_IPC, no channel bits, or a gap: no IPC.
        assert_eq!(ipc_channels(cap_ipc_channel(3)), None);
        assert_eq!(ipc_channels(CAP_IPC), None);
        assert_eq!(
            ipc_channels(CAP_IPC | cap_ipc_channel(2) | cap_ipc_channel(6)),
            None
        );
        // Delegation intersects channels like any other bit: a child of a
        // process holding 6-7 that asks for the defaults gets no channel.
        let agent = parse("ipc:6-7").unwrap();
        assert_eq!(ipc_channels(delegate(agent, CAP_LEGACY_FULL)), None);
        assert_eq!(ipc_channels(delegate(agent, agent)), Some((6, 7)));
        assert_eq!(
            ipc_channels(delegate(CAP_LEGACY_FULL, parse("ipc:6-7").unwrap())),
            None
        );
        // Every single-range grant round-trips, also through `Describe`;
        // every other channel set is refused by the parser or yields no IPC.
        for lo in 0..IPC_CHANNELS {
            for hi in lo..IPC_CHANNELS {
                let caps = parse(&format!("ipc:{lo}-{hi},fs_read")).unwrap();
                assert_eq!(ipc_channels(caps), Some((lo, hi)));
                assert_eq!(parse(&format!("{}", Describe(caps))), Ok(caps));
            }
        }
        assert_eq!(
            format!("{}", Describe(parse("spawn,ipc").unwrap())),
            "spawn,ipc"
        );
        assert_eq!(
            format!("{}", Describe(parse("ipc:6-7").unwrap())),
            "ipc:6-7"
        );
    }

    #[test]
    fn rejects_unknown_names() {
        assert_eq!(parse("gui,root"), Err(CapParseError::Unknown));
        assert_eq!(parse("all"), Err(CapParseError::Unknown));
        assert_eq!(parse("GUI"), Err(CapParseError::Unknown)); // case-sensitive
    }

    #[test]
    fn delegation_never_amplifies() {
        let parent = CAP_SPAWN | CAP_IPC;
        // Child asks for everything; gets only the intersection.
        assert_eq!(delegate(parent, CAP_ALL_KNOWN), parent);
        // Child asks for what the parent lacks; gets nothing.
        assert_eq!(delegate(parent, CAP_GUI | CAP_DEV), 0);
        // Subset requests pass through.
        assert_eq!(delegate(parent, CAP_IPC), CAP_IPC);
    }

    #[test]
    fn console_only_authority_is_never_passed_on() {
        // A parent holding the view and propose cannot give either to a
        // child, even when it asks for exactly that.
        let agent = CAP_SYS_VIEW | CAP_PROPOSE | CAP_IPC | CAP_SPAWN;
        assert_eq!(delegate(agent, CAP_SYS_VIEW | CAP_PROPOSE), 0);
        assert_eq!(delegate(agent, CAP_ALL_KNOWN), CAP_IPC | CAP_SPAWN);
        // No default set carries them (nor, since V1.0, the reserved IPC
        // channels).
        assert_eq!(CAP_LEGACY_FULL & CAP_CONSOLE_ONLY, 0);
        assert_eq!(
            CAP_LEGACY_FULL | CAP_CONSOLE_ONLY | CAP_IPC_RESERVED | CAP_SERVICE,
            CAP_ALL_KNOWN
        );
        // The console names them explicitly.
        assert_eq!(parse("sys_view,propose").unwrap(), CAP_CONSOLE_ONLY);
    }

    #[test]
    fn names_round_trip() {
        for (name, bit) in NAMES {
            // `ipc` carries its default channels with it (V1.0).
            let extra = if *bit == CAP_IPC { CAP_IPC_DEFAULT } else { 0 };
            assert_eq!(parse(name).unwrap(), *bit | extra);
            assert_eq!(name_of(*bit), *name);
        }
        let listed: Vec<&str> = names(CAP_GUI | CAP_SPAWN).collect();
        assert_eq!(listed, vec!["spawn", "gui"]);
    }
}
