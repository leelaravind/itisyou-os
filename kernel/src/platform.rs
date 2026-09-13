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
use kernel_core::trust::TrustStore;
use kernel_core::update::{self, StoreState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformError {
    /// Package failed parsing/digest/manifest verification.
    BadPackage,
    /// The package is intact but not vouched for by a trusted key: unsigned,
    /// signed by a stranger, or carrying a signature that does not verify.
    /// Distinct from `BadPackage` because the operator response is different —
    /// one is a damaged file, the other is a provenance decision.
    UntrustedPackage,
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

/// The package-signing trust ROOT (V0.9, KEY09-001): the public half of an
/// offline key whose private half never enters the source tree.
///
/// Compiled in rather than stored on disk: a trust root that lives in the
/// mutable store could be replaced by whatever it is supposed to be
/// protecting, which is not a root at all. The root signs nothing at run time;
/// it certifies signing keys (`/etc/trust/certs`) and revocation lists
/// (`/etc/trust/revocations.bin`), and the kernel accepts only what it signed.
/// The key comes from `keys/root.pub.hex` via `build.rs`.
const TRUST_ROOT: [u8; kernel_core::ed25519::PUBLIC_KEY_LEN] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/trust_root.bin"));

/// This kernel's release epoch. Certificates state the epochs they are valid
/// for, because the kernel has no trusted clock to check a date against.
pub const TRUST_EPOCH: u32 = 9;

/// The trust store, built from `/etc/trust` on first use.
static TRUST: spin::Mutex<Option<TrustStore>> = spin::Mutex::new(None);
/// Certificates refused while loading (not signed by the root, malformed, ...).
static CERTS_REJECTED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn load_trust() -> TrustStore {
    let mut store = TrustStore::new(TRUST_ROOT, TRUST_EPOCH);
    crate::serial_println!(
        "[ITISYOU:TRUST] root={} epoch={TRUST_EPOCH}",
        short_key(&TRUST_ROOT)
    );
    let mut names: Vec<String> = crate::fs::list("/etc/trust/certs")
        .map(|v| {
            v.into_iter()
                .filter(|e| !e.is_dir)
                .map(|e| e.name)
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for name in names {
        let path = format!("/etc/trust/certs/{name}");
        let result = crate::fs::read(&path)
            .map_err(|_| kernel_core::trust::CertError::TooShort)
            .and_then(|bytes| store.add_certificate(bytes));
        match result {
            Ok(k) => crate::serial_println!(
                "[ITISYOU:TRUST] cert key_id={} label={} scope={:?} epochs={}..={} signer={} result=ok",
                k.key_id,
                k.label(),
                k.scope(),
                k.first_epoch,
                k.last_epoch,
                short_key(&k.public_key)
            ),
            Err(e) => {
                CERTS_REJECTED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                crate::serial_println!(
                    "[ITISYOU:TRUST] cert file={name} result=rejected reason={}",
                    e.name()
                );
                crate::audit::denied("trust_load_certificate", 0);
            }
        }
    }
    match crate::fs::read("/etc/trust/revocations.bin") {
        Ok(bytes) => match store.apply_revocations(bytes) {
            Ok(seq) => crate::serial_println!(
                "[ITISYOU:TRUST] revocations seq={seq} count={} result=ok",
                store.revocations().revoked().len()
            ),
            Err(e) => crate::serial_println!(
                "[ITISYOU:TRUST] revocations result=rejected reason={}",
                e.name()
            ),
        },
        Err(_) => crate::serial_println!("[ITISYOU:TRUST] revocations result=absent"),
    }
    crate::serial_println!(
        "[ITISYOU:TRUST] certs_loaded={} rejected={}",
        store.keys().count(),
        CERTS_REJECTED.load(core::sync::atomic::Ordering::Relaxed)
    );
    store
}

/// Run `f` against the trust store, loading it on first use.
pub fn with_trust<R>(f: impl FnOnce(&TrustStore) -> R) -> R {
    let mut guard = TRUST.lock();
    let store = guard.get_or_insert_with(load_trust);
    f(store)
}

/// Print the trust store (the console's `pkg trust`).
pub fn report_trust() {
    with_trust(|s| {
        crate::serial_println!(
            "[ITISYOU:TRUST] root={} epoch={} certs={} rejected={} revocations_seq={} revoked={:?}",
            short_key(s.root()),
            s.epoch(),
            s.keys().count(),
            CERTS_REJECTED.load(core::sync::atomic::Ordering::Relaxed),
            s.revocations().sequence,
            s.revocations().revoked()
        );
        for k in s.keys() {
            let state = if s.revocations().contains(k.key_id) {
                "revoked"
            } else if s.epoch() > k.last_epoch {
                "expired"
            } else if s.epoch() < k.first_epoch {
                "not_yet_valid"
            } else {
                "valid"
            };
            crate::serial_println!(
                "[ITISYOU:TRUST] key key_id={} label={} scope={:?} epochs={}..={} state={state}",
                k.key_id,
                k.label(),
                k.scope(),
                k.first_epoch,
                k.last_epoch
            );
        }
    });
}

/// Check a parsed package against the trust store, reporting the precise
/// reason on refusal.
///
/// Integrity (`pkg::parse`) has already passed by the time this runs, so a
/// failure here is always about WHO vouched for the bytes, never about
/// whether they are intact. Keeping the two apart is what lets the audit trail
/// say `unsigned` versus `untrusted_signer` versus `bad_signature` — three
/// very different operational situations.
fn require_trusted(parsed: &pkg::Package<'_>, action: &'static str) -> Result<(), PlatformError> {
    match with_trust(|store| store.verify_package(parsed)) {
        Ok(key) => {
            let signer = short_key(&key.public_key);
            crate::serial_println!(
                "[ITISYOU:PKG] signature result=ok signer={signer} key_id={} label={}",
                key.key_id,
                key.label()
            );
            // A SUCCESSFUL verification is recorded too, not just a refusal.
            // Provenance is the point of the audit trail: "this app ran, and
            // its signature was checked against this key" is the record an
            // operator needs afterwards, and it cannot be reconstructed from
            // denials alone.
            crate::audit::allowed(action, 0, Some(format!("signer={signer}")));
            Ok(())
        }
        Err(e) => {
            crate::serial_println!(
                "[ITISYOU:PKG] signature result=refused reason={} action={action}",
                e.name()
            );
            crate::audit::denied(action, 0);
            Err(PlatformError::UntrustedPackage)
        }
    }
}

/// First four bytes of a key, hex — enough to tell signers apart in a log
/// without printing a full key on every line.
fn short_key(key: &[u8; kernel_core::ed25519::PUBLIC_KEY_LEN]) -> String {
    let mut s = String::new();
    for b in &key[..4] {
        s.push_str(&format!("{b:02x}"));
    }
    s
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
    // Authenticity, after integrity: an unsigned, foreign or forged package
    // never reaches the store, so no later step has to re-decide the question.
    require_trusted(&parsed, "pkg_verify_signature")?;
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
    // Re-checked at launch as well as install: the store is mutable, so a
    // package that was trustworthy when installed is not automatically
    // trustworthy when run.
    require_trusted(&parsed, "pkg_launch_signature")?;
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
