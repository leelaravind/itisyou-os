//! Update/rollback state resolution for the package store (V0.7).
//!
//! Installed applications live on the persistent filesystem as version-
//! numbered file pairs:
//!
//! - `<app>.<v>.pkg` — the verified package bytes for store version `v`
//! - `<app>.<v>.ok`  — the commit marker for `v`
//!
//! The ACTIVE version is the highest `v` with BOTH files. Because creating or
//! removing one small file is a single crash-consistent ITFS superblock
//! commit, every state transition is atomic:
//!
//! - **install/update**: write `.pkg` (staged), verify, then create `.ok`
//!   (the atomic commit — before it, the previous version stays active)
//! - **rollback**: remove the highest `.ok` (previous committed version
//!   becomes active again; the demoted `.pkg` remains as evidence)
//! - **interrupted update**: a `.pkg` without `.ok` — never activated; boot
//!   recovery detects and reports it, then removes the orphan
//!
//! This module is the pure resolution logic (host-tested, allocation-free);
//! the kernel's platform module does the filesystem operations.

/// What a store filename means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pkg,
    Ok,
}

/// Parse `<app>.<v>.pkg` / `<app>.<v>.ok` → (app, version, kind).
pub fn parse_store_name(name: &str) -> Option<(&str, u32, Kind)> {
    let (rest, kind) = match name.strip_suffix(".pkg") {
        Some(r) => (r, Kind::Pkg),
        None => (name.strip_suffix(".ok")?, Kind::Ok),
    };
    let (app, ver) = rest.rsplit_once('.')?;
    if app.is_empty() {
        return None;
    }
    let v: u32 = ver.parse().ok()?;
    Some((app, v, kind))
}

/// The audit trail's name in the store (V0.8).
pub const AUDIT_TRAIL: &str = "audit.log";

/// Is `name` a store file the kernel owns (V0.11, SEC11-001)?
///
/// The audit trail, and every name [`parse_store_name`] reads as part of an
/// application's state. Programs reach the store through the `fs_*`
/// syscalls, and these names are not part of that namespace. Before V0.11 a
/// program holding `fs_write` could delete an application's highest `.ok` — a
/// rollback nobody approved — or plant one, or replace the audit trail with an
/// empty one that verifies.
pub fn kernel_owned(name: &str) -> bool {
    name == AUDIT_TRAIL || parse_store_name(name).is_some()
}

const MAX_VERSIONS: usize = 16;

/// Resolved store state for one application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoreState {
    /// Highest version with both `.pkg` and `.ok` — what launches.
    pub active: Option<u32>,
    /// Highest committed version below `active` — the rollback target.
    pub previous: Option<u32>,
    /// A `.pkg` without `.ok` (interrupted/staged update), if any.
    pub orphan_staged: Option<u32>,
    /// An `.ok` without `.pkg` (inconsistent commit marker), if any.
    pub dangling_ok: Option<u32>,
}

/// Resolve the store state for `app` from a listing of store filenames.
pub fn resolve<'a>(app: &str, names: impl Iterator<Item = &'a str>) -> StoreState {
    let mut pkgs = [0u32; MAX_VERSIONS];
    let mut oks = [0u32; MAX_VERSIONS];
    let (mut np, mut no) = (0usize, 0usize);
    for name in names {
        if let Some((a, v, kind)) = parse_store_name(name) {
            if a != app {
                continue;
            }
            match kind {
                Kind::Pkg if np < MAX_VERSIONS => {
                    pkgs[np] = v;
                    np += 1;
                }
                Kind::Ok if no < MAX_VERSIONS => {
                    oks[no] = v;
                    no += 1;
                }
                _ => {}
            }
        }
    }
    let has = |arr: &[u32], v: u32| arr.contains(&v);
    let mut state = StoreState::default();
    // Committed = pkg + ok. Active = max committed; previous = next-highest.
    for &v in &pkgs[..np] {
        if has(&oks[..no], v) {
            match state.active {
                Some(a) if v <= a => {
                    if state.previous.is_none_or(|p| v > p) && v < a {
                        state.previous = Some(v);
                    }
                }
                Some(a) => {
                    state.previous = Some(a);
                    state.active = Some(v);
                }
                None => state.active = Some(v),
            }
        } else if state.orphan_staged.is_none_or(|o| v > o) {
            state.orphan_staged = Some(v);
        }
    }
    for &v in &oks[..no] {
        if !has(&pkgs[..np], v) && state.dangling_ok.is_none_or(|o| v > o) {
            state.dangling_ok = Some(v);
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_store_names() {
        assert_eq!(
            parse_store_name("hello.1.pkg"),
            Some(("hello", 1, Kind::Pkg))
        );
        assert_eq!(
            parse_store_name("hello-app.12.ok"),
            Some(("hello-app", 12, Kind::Ok))
        );
        assert_eq!(parse_store_name("hello.pkg"), None); // no version
        assert_eq!(parse_store_name("hello.x.pkg"), None); // non-numeric
        assert_eq!(parse_store_name(".1.pkg"), None); // empty app
    }

    #[test]
    fn kernel_owned_names_are_the_trail_and_everything_the_resolver_reads() {
        for name in [
            "audit.log",
            "hello-app.1.pkg",
            "hello-app.1.ok",
            "hello-app.12.ok",
            // `u32::from_str` takes a sign, so the resolver reads this as
            // version 2 — which is exactly why it must be reserved too.
            "hello-app.+2.ok",
        ] {
            assert!(kernel_owned(name), "{name}");
        }
        for name in [
            "notes.txt",
            "audit.log.bak",
            "hello.pkg",
            "x.y.pkg",
            "user-note",
            "a.ok.txt",
        ] {
            assert!(!kernel_owned(name), "{name}");
        }
        // Every name `resolve` would count is reserved.
        for name in ["other.1.pkg", "other.1.ok", "a.0.ok", "a.4294967295.pkg"] {
            assert!(
                parse_store_name(name).is_some() && kernel_owned(name),
                "{name}"
            );
        }
        assert_eq!(parse_store_name("audit.log"), None); // unrelated file
    }

    fn st(names: &[&str]) -> StoreState {
        resolve("hello", names.iter().copied())
    }

    #[test]
    fn resolves_normal_lifecycle() {
        // Fresh install committed.
        assert_eq!(
            st(&["hello.1.pkg", "hello.1.ok"]),
            StoreState {
                active: Some(1),
                ..Default::default()
            }
        );
        // Updated to v2: v2 active, v1 is the rollback target.
        assert_eq!(
            st(&["hello.1.pkg", "hello.1.ok", "hello.2.pkg", "hello.2.ok"]),
            StoreState {
                active: Some(2),
                previous: Some(1),
                ..Default::default()
            }
        );
    }

    #[test]
    fn interrupted_update_never_activates() {
        // v2 staged but not committed → v1 stays active; v2 is the orphan.
        assert_eq!(
            st(&["hello.1.pkg", "hello.1.ok", "hello.2.pkg"]),
            StoreState {
                active: Some(1),
                orphan_staged: Some(2),
                ..Default::default()
            }
        );
        // Nothing committed at all.
        assert_eq!(
            st(&["hello.1.pkg"]),
            StoreState {
                orphan_staged: Some(1),
                ..Default::default()
            }
        );
    }

    #[test]
    fn rollback_state_after_ok_removal() {
        // v2's .ok removed (rollback): v1 active again, v2.pkg is evidence.
        assert_eq!(
            st(&["hello.1.pkg", "hello.1.ok", "hello.2.pkg"]),
            StoreState {
                active: Some(1),
                orphan_staged: Some(2),
                ..Default::default()
            }
        );
    }

    #[test]
    fn flags_dangling_commit_marker() {
        assert_eq!(
            st(&["hello.3.ok"]),
            StoreState {
                dangling_ok: Some(3),
                ..Default::default()
            }
        );
    }

    #[test]
    fn ignores_other_apps() {
        let s = resolve("hello", ["other.1.pkg", "other.1.ok"].into_iter());
        assert_eq!(s, StoreState::default());
    }
}
