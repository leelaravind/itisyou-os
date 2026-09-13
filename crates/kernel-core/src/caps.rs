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

/// Every currently defined capability bit.
pub const CAP_ALL_KNOWN: u64 = CAP_SPAWN
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
/// `rsh`, desktop apps and `pkg launch` never get the view or `propose`.
pub const CAP_LEGACY_FULL: u64 = CAP_DELEGABLE;

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
}

/// Parse a comma-separated capability list (e.g. `"gui,fs_read"`). Empty
/// input means no capabilities (default deny). Whitespace around names is
/// tolerated; unknown names are errors.
pub fn parse(list: &str) -> Result<u64, CapParseError> {
    let mut caps = 0u64;
    for raw in list.split(',') {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        match NAMES.iter().find(|(n, _)| *n == name) {
            Some((_, bit)) => caps |= bit,
            None => return Err(CapParseError::Unknown),
        }
    }
    Ok(caps)
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
        assert_eq!(parse(" spawn , ipc ").unwrap(), CAP_SPAWN | CAP_IPC);
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
        // No default set carries them.
        assert_eq!(CAP_LEGACY_FULL & CAP_CONSOLE_ONLY, 0);
        assert_eq!(CAP_LEGACY_FULL | CAP_CONSOLE_ONLY, CAP_ALL_KNOWN);
        // The console names them explicitly.
        assert_eq!(parse("sys_view,propose").unwrap(), CAP_CONSOLE_ONLY);
    }

    #[test]
    fn names_round_trip() {
        for (name, bit) in NAMES {
            assert_eq!(parse(name).unwrap(), *bit);
            assert_eq!(name_of(*bit), *name);
        }
        let listed: Vec<&str> = names(CAP_GUI | CAP_SPAWN).collect();
        assert_eq!(listed, vec!["spawn", "gui"]);
    }
}
