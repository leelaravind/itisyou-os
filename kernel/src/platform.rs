//! Application platform: install / update / rollback / recovery / launch
//! (V0.7, ADR-0012).
//!
//! Applications ship as ITPKG packages (manifest + ELF + SHA-256, verified
//! by host-tested `kernel_core::pkg`) and live in the persistent ITFS store
//! as version-numbered `<app>.<v>.pkg` / `<app>.<v>.ok` pairs. Every state
//! transition is one crash-atomic superblock commit (`kernel_core::update`
//! documents the state machine):
//!
//! - install/update: write `.pkg` (staged) → verify → create `.ok` (commit)
//! - rollback: remove the newest `.ok` (previous version is active again)
//! - recovery: a `.pkg` without `.ok` (interrupted update) is detected on
//!   demand, reported, audited, and removed — it can NEVER activate
//!
//! Launch re-verifies the package digest, then runs the payload with ONLY
//! the manifest-requested capabilities (∩ launcher) under the standard app
//! sandbox — request → capability check → deterministic service → action →
//! audit, the same path any future AI agent must use.

use crate::fs_disk::{Error as FsDiskError, FileSystem};
use crate::user;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use kernel_core::caps;
use kernel_core::pkg;
use kernel_core::update::{self, StoreState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformError {
    /// Package failed parsing/digest/manifest verification.
    BadPackage,
    Storage,
    NotInstalled,
    /// Rollback with no previous committed version.
    NoPrevious,
    /// The app's payload failed to load/run.
    LaunchFailed,
}

fn fs_err(_e: FsDiskError) -> PlatformError {
    PlatformError::Storage
}

/// Resolve the store state for `app` from the live filesystem listing.
pub fn state(fs: &FileSystem, app: &str) -> StoreState {
    update::resolve(app, fs.list().into_iter())
}

/// Every distinct app name present in the store.
pub fn apps(fs: &FileSystem) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for f in fs.list() {
        if let Some((app, _, _)) = update::parse_store_name(f) {
            if !names.iter().any(|n| n == app) {
                names.push(app.to_string());
            }
        }
    }
    names
}

/// Verify a package and STAGE it (write `.pkg` without the commit marker).
/// Returns (app name, assigned store version). Used by `install` and by the
/// interrupted-update simulation.
pub fn stage(fs: &mut FileSystem, pkg_bytes: &[u8]) -> Result<(String, u32), PlatformError> {
    let parsed = pkg::parse(pkg_bytes).map_err(|e| {
        crate::serial_println!("[ITISYOU:PKG] verify result=refused reason={e:?}");
        crate::audit::denied("pkg_verify", 0);
        PlatformError::BadPackage
    })?;
    let app = parsed.manifest.name.to_string();
    let st = state(fs, &app);
    let next = st
        .active
        .max(st.orphan_staged)
        .max(st.dangling_ok)
        .map_or(1, |v| v + 1);
    if let Err(e) = fs.create(&format!("{app}.{next}.pkg"), pkg_bytes) {
        crate::serial_println!(
            "[ITISYOU:PKG] stage name={app} v={next} result=storage_error {e:?}"
        );
        return Err(fs_err(e));
    }
    crate::serial_println!(
        "[ITISYOU:PKG] stage name={app} v={next} manifest_version={} result=ok",
        parsed.manifest.version
    );
    Ok((app, next))
}

/// Install (or update to) a verified package: stage, then atomically commit.
pub fn install(fs: &mut FileSystem, pkg_bytes: &[u8]) -> Result<(String, u32), PlatformError> {
    let (app, v) = stage(fs, pkg_bytes)?;
    // The commit: one crash-atomic superblock transition. Before this line
    // the previous version (if any) is still the active one.
    fs.create(&format!("{app}.{v}.ok"), b"ok").map_err(fs_err)?;
    crate::audit::allowed("pkg_install", 0, Some(format!("{app} v{v}")));
    crate::serial_println!("[ITISYOU:PKG] install name={app} v={v} result=ok");
    Ok((app, v))
}

/// Roll back to the previous committed version by removing the newest commit
/// marker (one atomic superblock transition). The demoted package file is
/// kept as forensic evidence.
pub fn rollback(fs: &mut FileSystem, app: &str) -> Result<(u32, u32), PlatformError> {
    let st = state(fs, app);
    let active = st.active.ok_or(PlatformError::NotInstalled)?;
    let previous = st.previous.ok_or(PlatformError::NoPrevious)?;
    fs.remove(&format!("{app}.{active}.ok")).map_err(fs_err)?;
    crate::audit::allowed(
        "pkg_rollback",
        0,
        Some(format!("{app} v{active}->v{previous}")),
    );
    crate::serial_println!(
        "[ITISYOU:PKG] rollback name={app} from=v{active} to=v{previous} result=ok"
    );
    Ok((active, previous))
}

/// Recovery scan: detect and clean interrupted updates (`.pkg` without
/// `.ok`) and dangling commit markers. Each finding is reported + audited
/// BEFORE its cleanup, preserving the forensic trail. Returns findings.
pub fn recover(fs: &mut FileSystem) -> Result<u32, PlatformError> {
    let mut findings = 0u32;
    for app in apps(fs) {
        // Loop: multiple orphans possible (resolve reports the newest first).
        loop {
            let st = state(fs, &app);
            if let Some(v) = st.orphan_staged {
                crate::serial_println!(
                    "[ITISYOU:RECOVERY] app={app} orphan_staged=v{v} action=remove"
                );
                crate::audit::allowed("recovery_orphan", 0, Some(format!("{app} v{v}")));
                fs.remove(&format!("{app}.{v}.pkg")).map_err(fs_err)?;
                findings += 1;
                continue;
            }
            if let Some(v) = st.dangling_ok {
                crate::serial_println!(
                    "[ITISYOU:RECOVERY] app={app} dangling_ok=v{v} action=remove"
                );
                crate::audit::allowed("recovery_dangling", 0, Some(format!("{app} v{v}")));
                fs.remove(&format!("{app}.{v}.ok")).map_err(fs_err)?;
                findings += 1;
                continue;
            }
            break;
        }
    }
    crate::serial_println!("[ITISYOU:RECOVERY] scan_complete findings={findings}");
    Ok(findings)
}

/// Launch the ACTIVE version of an installed app: re-verify the package
/// digest, then run the payload with `manifest caps ∩ launcher caps` under
/// the standard app sandbox (its own app dir + /etc, read-only view).
/// Returns the app's exit code.
pub fn launch(fs: &FileSystem, app: &str, launcher_caps: u64) -> Result<u64, PlatformError> {
    let st = state(fs, app);
    let v = st.active.ok_or(PlatformError::NotInstalled)?;
    let bytes = fs.read(&format!("{app}.{v}.pkg")).map_err(fs_err)?;
    // Re-verify at launch: storage corruption or tampering is caught here.
    let parsed = pkg::parse(&bytes).map_err(|_| {
        crate::audit::denied("pkg_launch_verify", 0);
        PlatformError::BadPackage
    })?;
    let granted = caps::delegate(launcher_caps, parsed.manifest.caps);
    let sandbox = Arc::new(alloc::vec![format!("/apps/{app}"), String::from("/etc"),]);
    let mut process =
        user::load_from_bytes(parsed.payload).map_err(|_| PlatformError::LaunchFailed)?;
    process.set_authority(granted, Some(sandbox));
    crate::audit::allowed("pkg_launch", granted, Some(format!("{app} v{v}")));
    crate::serial_println!(
        "[ITISYOU:PKG] launch name={app} v={v} caps={granted:#x} manifest_version={}",
        parsed.manifest.version
    );
    match user::run(process) {
        user::UserExit::Exit(code) => Ok(code),
        _ => Err(PlatformError::LaunchFailed),
    }
}
