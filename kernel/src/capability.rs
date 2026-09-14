//! Kernel-owned capability-handle registry — the V0.8 enforcement path.
//!
//! V0.7 enforced authority by testing a static bitmask (`CURRENT_CAPS`) that
//! was fixed for a process's whole lifetime. V0.8 moves enforcement into this
//! table: the bitmask now only *seeds* a process's handles at load time, and
//! every privileged syscall revalidates the live handle instead. That is what
//! makes ownership binding, resource scoping, bounded delegation, expiry and
//! revocation real rather than advisory — revoking a handle takes effect on
//! the process's very next syscall, which a static bitmask could never do.
//!
//! Layout: one handle per [`CapabilityKind`], stored at `kind.index()`. The
//! authority a handle carries lives in its rights bits, so a filesystem READ
//! handle cannot satisfy a filesystem WRITE check. Handles are opaque to
//! userspace: guessing an integer grants nothing, because the table checks the
//! generation, the owning pid, the kind, the scope and the expiry on use.

use kernel_core::capability::{
    rights_from_bits, scope_from_bits, CapabilityError, CapabilityHandle, CapabilityKind,
    CapabilityTable, ResourceScope,
};

use crate::sync::Mutex;

/// One slot per resource kind; slot index IS the kind index (also the ABI
/// value userspace uses to address its own handles).
pub const HANDLE_SLOTS: usize = CapabilityKind::COUNT;

/// Handles for every live process. Sized for many concurrent processes each
/// holding at most `HANDLE_SLOTS` handles, plus room for self-restriction
/// churn (restrict frees before it grants, so it never needs headroom).
static TABLE: Mutex<CapabilityTable<256>> = Mutex::new(CapabilityTable::new());

/// A process's handle set: index by [`CapabilityKind::index`]; an empty slot
/// is [`CapabilityHandle::INVALID`] and confers no authority.
pub type HandleSet = [CapabilityHandle; HANDLE_SLOTS];

pub const EMPTY_HANDLES: HandleSet = [CapabilityHandle::INVALID; HANDLE_SLOTS];

/// Mint the handle set for a newly loaded process from its capability bits.
///
/// Every handle is owned by `pid`, so it dies with the process (see
/// [`revoke_owner`]) and is unusable by anyone else even if its raw value
/// leaks. Handles minted here are delegable: a process may pass a *narrowed*
/// copy to a child it spawns.
pub fn handles_for(pid: u64, bits: u64) -> HandleSet {
    let rights = rights_from_bits(bits);
    let mut set = EMPTY_HANDLES;
    let mut table = TABLE.lock();
    for (index, &right_bits) in rights.iter().enumerate() {
        if right_bits == 0 {
            continue;
        }
        let Some(kind) = CapabilityKind::from_index(index) else {
            continue;
        };
        // V1.0 (ADR-0025): the IPC handle is scoped to the channels the bits
        // name; bits naming no usable range mint no IPC handle at all.
        let Some(scope) = scope_from_bits(kind, bits) else {
            continue;
        };
        if let Ok(handle) = table.grant(pid, kind, scope, right_bits, None, true) {
            set[index] = handle;
        }
    }
    set
}

/// Derive a child's handle set by delegating from the parent's live handles.
///
/// Delegation goes through the table, so amplification is refused by the same
/// code that enforces every other check: a child can only receive rights its
/// parent actually holds *right now*, over a scope inside the parent's. A
/// parent whose own authority was revoked or expired can no longer pass it on.
/// Returns the child's handle set and whether every handle it should have
/// received was granted. `false` means the global table was full (V1.0,
/// CAP1-001): a caller that gets `false` must refuse the launch rather than
/// admit a process with a silently reduced authority — otherwise a program
/// holding `spawn` could fill the table with long-lived children and every
/// later process (the console's own launches included) would run with no
/// authority, unreported, while the spawn audit still claimed the full set.
/// A handle the parent legitimately cannot delegate (it does not hold it, or
/// the child's scope is not inside the parent's) is NOT a shortfall — that is
/// ordinary least-privilege narrowing.
pub fn delegate_to_child(
    parent_pid: u64,
    child_pid: u64,
    requested_bits: u64,
    now: u64,
) -> (HandleSet, bool) {
    let requested = rights_from_bits(requested_bits);
    let parent_handles = crate::syscall::current_handles();
    let mut set = EMPTY_HANDLES;
    let mut all_granted = true;
    let mut table = TABLE.lock();
    for (index, &want) in requested.iter().enumerate() {
        if want == 0 {
            continue;
        }
        let parent = parent_handles[index];
        if parent == CapabilityHandle::INVALID {
            continue;
        }
        // The child's scope comes from its own (already intersected) bits;
        // the table refuses it unless it lies inside the parent's (V1.0).
        let Some(scope) = CapabilityKind::from_index(index)
            .and_then(|kind| scope_from_bits(kind, requested_bits))
        else {
            continue;
        };
        match table.delegate(parent, parent_pid, child_pid, scope, want, now) {
            Ok(handle) => set[index] = handle,
            // The one failure that is not the parent's own least-privilege:
            // the table has no free slot for a handle the parent could pass.
            Err(CapabilityError::TableFull) => all_granted = false,
            Err(_) => {}
        }
    }
    (set, all_granted)
}

/// Validate one handle for an operation. This is the single enforcement point
/// every privileged syscall funnels through.
pub fn check(
    handle: CapabilityHandle,
    owner: u64,
    kind: CapabilityKind,
    scope: ResourceScope,
    rights: u32,
    now: u64,
) -> Result<(), CapabilityError> {
    TABLE.lock().check(handle, owner, kind, scope, rights, now)
}

/// Voluntarily drop one of a process's own handles. Takes effect immediately:
/// the next syscall needing that authority is refused.
pub fn revoke(handle: CapabilityHandle, owner: u64, now: u64) -> Result<(), CapabilityError> {
    TABLE.lock().revoke(handle, owner, now)
}

/// Replace one of a process's own handles with a strictly narrower one.
pub fn restrict(
    handle: CapabilityHandle,
    owner: u64,
    scope: ResourceScope,
    rights: u32,
    expires_at: Option<u64>,
    now: u64,
) -> Result<CapabilityHandle, CapabilityError> {
    TABLE
        .lock()
        .restrict(handle, owner, scope, rights, expires_at, now)
}

/// Drop every handle owned by `pid`. Called on process teardown so a dead
/// process's authority cannot outlive it or be inherited by a recycled pid.
pub fn revoke_owner(pid: u64) -> usize {
    TABLE.lock().revoke_owner(pid)
}

/// Live handles in the table (V1.0 leak accounting).
pub fn count_live() -> usize {
    TABLE.lock().live()
}

/// Live handles owned by `pid` — teardown evidence for tests and the shell.
pub fn count_owned(pid: u64) -> usize {
    TABLE.lock().count_owned(pid)
}

/// Human-readable denial reason for audit records.
pub fn reason_name(error: CapabilityError) -> &'static str {
    match error {
        CapabilityError::InvalidHandle => "invalid_handle",
        CapabilityError::Revoked => "revoked",
        CapabilityError::Expired => "expired",
        CapabilityError::NotOwner => "not_owner",
        CapabilityError::WrongKind => "wrong_kind",
        CapabilityError::ScopeDenied => "scope_denied",
        CapabilityError::NotDelegable => "not_delegable",
        CapabilityError::TableFull => "table_full",
    }
}
