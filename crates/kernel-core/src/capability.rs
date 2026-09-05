//! V0.8 resource-scoped capability handles.
//!
//! This module is deliberately allocation-free and side-effect-free.  The
//! kernel owns the table and exposes only opaque handles to processes.  A
//! handle contains an index and generation, while ownership, kind, scope and
//! expiry remain in the table; consequently guessing an integer never grants
//! authority and a reused slot cannot resurrect an old handle.

/// Opaque capability reference.  The raw representation is useful at an ABI
/// boundary, but it has no authority without a matching live table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct CapabilityHandle(u64);

impl CapabilityHandle {
    pub const INVALID: Self = Self(0);

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    const fn new(index: usize, generation: u32) -> Self {
        Self(((generation as u64) << 32) | (index as u64 + 1))
    }

    const fn parts(self) -> Option<(usize, u32)> {
        let slot = (self.0 & 0xffff_ffff) as usize;
        if slot == 0 {
            None
        } else {
            Some((slot - 1, (self.0 >> 32) as u32))
        }
    }
}

/// Resource class checked at a service/kernel boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityKind {
    Filesystem,
    Device,
    Gui,
    Network,
    Audio,
    Process,
    Service,
    SystemAdministration,
}

impl CapabilityKind {
    /// Number of resource classes. A process holds at most one handle per
    /// class, so this is also the size of a process's handle array and the
    /// index space the kernel and userspace agree on.
    pub const COUNT: usize = 8;

    /// Stable index used as the handle-array slot and as the ABI value for
    /// the kind argument of the capability syscalls. Never renumber these.
    pub const fn index(self) -> usize {
        match self {
            CapabilityKind::Filesystem => 0,
            CapabilityKind::Device => 1,
            CapabilityKind::Gui => 2,
            CapabilityKind::Network => 3,
            CapabilityKind::Audio => 4,
            CapabilityKind::Process => 5,
            CapabilityKind::Service => 6,
            CapabilityKind::SystemAdministration => 7,
        }
    }

    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(CapabilityKind::Filesystem),
            1 => Some(CapabilityKind::Device),
            2 => Some(CapabilityKind::Gui),
            3 => Some(CapabilityKind::Network),
            4 => Some(CapabilityKind::Audio),
            5 => Some(CapabilityKind::Process),
            6 => Some(CapabilityKind::Service),
            7 => Some(CapabilityKind::SystemAdministration),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            CapabilityKind::Filesystem => "filesystem",
            CapabilityKind::Device => "device",
            CapabilityKind::Gui => "gui",
            CapabilityKind::Network => "network",
            CapabilityKind::Audio => "audio",
            CapabilityKind::Process => "process",
            CapabilityKind::Service => "service",
            CapabilityKind::SystemAdministration => "sysadmin",
        }
    }
}

/// Rights carried inside a handle, interpreted per [`CapabilityKind`].
///
/// V0.7 modelled authority as one flat bit per operation class. Translating
/// those bits straight into one handle per class would have made two
/// different authorities over the same resource — filesystem read and
/// filesystem write — indistinguishable, so holding `fs_read` would have
/// satisfied a write check. Rights live inside the handle instead: one handle
/// per class says exactly what its holder may do, and delegation can only
/// ever clear bits.
pub mod rights {
    /// Basic use of the resource class (spawn a process, open a socket, draw).
    pub const USE: u32 = 1 << 0;
    /// Observe resource state (read a file, query the device table).
    pub const READ: u32 = 1 << 1;
    /// Mutate resource state (write/delete/rename a file).
    pub const WRITE: u32 = 1 << 2;
    /// Lifecycle control over an existing instance (terminate a process).
    pub const CONTROL: u32 = 1 << 3;
    /// Administrative authority over the whole class.
    pub const ADMIN: u32 = 1 << 4;
    /// Every defined right — for bounds checks only, never granted wholesale.
    pub const ALL: u32 = USE | READ | WRITE | CONTROL | ADMIN;
}

/// Translate a legacy V0.7 capability bitmask into the per-kind rights a
/// process's handle set is minted from.
///
/// The bit set remains the *source* of a process's authority (manifests,
/// service definitions and spawn requests are still written in those terms),
/// but it stops being the thing enforcement consults: the kernel mints one
/// handle per non-empty entry here and every privileged syscall then revalidates
/// the live handle. Result index is [`CapabilityKind::index`].
pub fn rights_from_bits(bits: u64) -> [u32; CapabilityKind::COUNT] {
    use crate::caps::*;
    let mut out = [0u32; CapabilityKind::COUNT];
    let mut add = |kind: CapabilityKind, right: u32| {
        out[kind.index()] |= right;
    };
    if bits & CAP_SPAWN != 0 {
        add(CapabilityKind::Process, rights::USE);
    }
    if bits & CAP_PROC_CONTROL != 0 {
        add(CapabilityKind::Process, rights::CONTROL);
    }
    if bits & CAP_IPC != 0 {
        add(CapabilityKind::Service, rights::USE);
    }
    if bits & CAP_SERVICE != 0 {
        add(CapabilityKind::Service, rights::ADMIN);
    }
    if bits & CAP_GUI != 0 {
        add(CapabilityKind::Gui, rights::USE);
    }
    if bits & CAP_DEV != 0 {
        add(CapabilityKind::Device, rights::READ);
    }
    if bits & CAP_FS_READ != 0 {
        add(CapabilityKind::Filesystem, rights::READ);
    }
    if bits & CAP_FS_WRITE != 0 {
        add(CapabilityKind::Filesystem, rights::WRITE);
    }
    if bits & CAP_AUDIO != 0 {
        add(CapabilityKind::Audio, rights::USE);
    }
    if bits & CAP_NETWORK != 0 {
        // Reading the interface's own address and link state is part of
        // using it, not a separate privilege — a program that may send
        // packets but may not learn its own IP is an awkward fiction. A
        // handle narrowed to USE alone (via `restrict`) still loses the
        // observation right, so the distinction stays available.
        add(CapabilityKind::Network, rights::USE | rights::READ);
    }
    if bits & CAP_SYS_ADMIN != 0 {
        add(CapabilityKind::SystemAdministration, rights::ADMIN);
    }
    out
}

/// A compact resource scope.  `start..=end` is interpreted by the consumer
/// (path id, device id, port range, or service id); the table only enforces
/// that delegated scopes cannot expand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceScope {
    pub start: u64,
    pub end: u64,
}

impl ResourceScope {
    pub const ANY: Self = Self {
        start: 0,
        end: u64::MAX,
    };

    pub const fn contains(self, other: Self) -> bool {
        other.start >= self.start && other.end <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    InvalidHandle,
    Revoked,
    Expired,
    NotOwner,
    WrongKind,
    ScopeDenied,
    NotDelegable,
    TableFull,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    owner: u64,
    kind: CapabilityKind,
    scope: ResourceScope,
    rights: u32,
    generation: u32,
    expires_at: Option<u64>,
    delegable: bool,
}

/// Fixed-capacity capability store suitable for a kernel static allocation.
pub struct CapabilityTable<const N: usize> {
    entries: [Option<Entry>; N],
    generations: [u32; N],
}

impl<const N: usize> CapabilityTable<N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; N],
            generations: [0; N],
        }
    }

    pub fn grant(
        &mut self,
        owner: u64,
        kind: CapabilityKind,
        scope: ResourceScope,
        rights: u32,
        expires_at: Option<u64>,
        delegable: bool,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let index = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(CapabilityError::TableFull)?;
        let generation = self.generations[index].wrapping_add(1).max(1);
        self.generations[index] = generation;
        self.entries[index] = Some(Entry {
            owner,
            kind,
            scope,
            rights,
            generation,
            expires_at,
            delegable,
        });
        Ok(CapabilityHandle::new(index, generation))
    }

    fn entry(&self, handle: CapabilityHandle, now: u64) -> Result<(usize, Entry), CapabilityError> {
        let (index, generation) = handle.parts().ok_or(CapabilityError::InvalidHandle)?;
        let entry = self
            .entries
            .get(index)
            .and_then(|e| *e)
            .ok_or(CapabilityError::Revoked)?;
        if entry.generation != generation {
            return Err(CapabilityError::InvalidHandle);
        }
        if entry.expires_at.is_some_and(|deadline| now >= deadline) {
            return Err(CapabilityError::Expired);
        }
        Ok((index, entry))
    }

    pub fn check(
        &self,
        handle: CapabilityHandle,
        owner: u64,
        kind: CapabilityKind,
        requested_scope: ResourceScope,
        requested_rights: u32,
        now: u64,
    ) -> Result<(), CapabilityError> {
        let (_, entry) = self.entry(handle, now)?;
        if entry.owner != owner {
            return Err(CapabilityError::NotOwner);
        }
        if entry.kind != kind {
            return Err(CapabilityError::WrongKind);
        }
        if !entry.scope.contains(requested_scope) {
            return Err(CapabilityError::ScopeDenied);
        }
        if requested_rights & !entry.rights != 0 {
            return Err(CapabilityError::ScopeDenied);
        }
        Ok(())
    }

    pub fn delegate(
        &mut self,
        parent: CapabilityHandle,
        parent_owner: u64,
        child_owner: u64,
        scope: ResourceScope,
        rights: u32,
        now: u64,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let (_, entry) = self.entry(parent, now)?;
        if entry.owner != parent_owner {
            return Err(CapabilityError::NotOwner);
        }
        if !entry.delegable {
            return Err(CapabilityError::NotDelegable);
        }
        if !entry.scope.contains(scope) || rights & !entry.rights != 0 {
            return Err(CapabilityError::ScopeDenied);
        }
        let expiry = entry.expires_at;
        self.grant(child_owner, entry.kind, scope, rights, expiry, true)
    }

    pub fn revoke(
        &mut self,
        handle: CapabilityHandle,
        owner: u64,
        now: u64,
    ) -> Result<(), CapabilityError> {
        let (index, entry) = self.entry(handle, now)?;
        if entry.owner != owner {
            return Err(CapabilityError::NotOwner);
        }
        self.entries[index] = None;
        Ok(())
    }

    /// Replace `handle` with a strictly narrower one owned by the same
    /// process: rights are intersected, the scope must stay inside the
    /// original, and an expiry may only be brought forward. Used by a process
    /// voluntarily dropping authority it no longer needs (least privilege),
    /// so a later compromise of that process cannot use what it gave up.
    pub fn restrict(
        &mut self,
        handle: CapabilityHandle,
        owner: u64,
        scope: ResourceScope,
        rights: u32,
        expires_at: Option<u64>,
        now: u64,
    ) -> Result<CapabilityHandle, CapabilityError> {
        let (index, entry) = self.entry(handle, now)?;
        if entry.owner != owner {
            return Err(CapabilityError::NotOwner);
        }
        if !entry.scope.contains(scope) {
            return Err(CapabilityError::ScopeDenied);
        }
        // Narrowing only: never hand back a right or a lifetime the caller
        // did not already hold.
        let rights = rights & entry.rights;
        let expires_at = match (entry.expires_at, expires_at) {
            (Some(old), Some(new)) => Some(old.min(new)),
            (Some(old), None) => Some(old),
            (None, new) => new,
        };
        // Free the old slot first so restricting cannot fail on a full table.
        self.entries[index] = None;
        self.grant(
            owner,
            entry.kind,
            scope,
            rights,
            expires_at,
            entry.delegable,
        )
    }

    /// Number of live entries owned by `owner` (teardown/leak evidence).
    pub fn count_owned(&self, owner: u64) -> usize {
        self.entries
            .iter()
            .filter(|e| e.is_some_and(|e| e.owner == owner))
            .count()
    }

    pub fn revoke_owner(&mut self, owner: u64) -> usize {
        let mut count = 0;
        for entry in &mut self.entries {
            if entry.is_some_and(|e| e.owner == owner) {
                *entry = None;
                count += 1;
            }
        }
        count
    }
}

impl<const N: usize> Default for CapabilityTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FS: ResourceScope = ResourceScope { start: 10, end: 20 };

    #[test]
    fn forged_and_stale_handles_are_rejected() {
        let mut t = CapabilityTable::<2>::new();
        let h = t
            .grant(1, CapabilityKind::Filesystem, FS, 0b11, None, true)
            .unwrap();
        assert_eq!(
            t.check(
                CapabilityHandle::from_raw(h.raw() ^ (1 << 32)),
                1,
                CapabilityKind::Filesystem,
                FS,
                1,
                0
            ),
            Err(CapabilityError::InvalidHandle)
        );
        t.revoke(h, 1, 0).unwrap();
        let h2 = t
            .grant(1, CapabilityKind::Filesystem, FS, 1, None, true)
            .unwrap();
        assert_ne!(h, h2);
        assert_eq!(
            t.check(h, 1, CapabilityKind::Filesystem, FS, 1, 0),
            Err(CapabilityError::InvalidHandle)
        );
    }

    #[test]
    fn ownership_scope_and_rights_are_enforced() {
        let mut t = CapabilityTable::<1>::new();
        let h = t
            .grant(7, CapabilityKind::Filesystem, FS, 1, None, true)
            .unwrap();
        assert_eq!(
            t.check(h, 8, CapabilityKind::Filesystem, FS, 1, 0),
            Err(CapabilityError::NotOwner)
        );
        assert_eq!(
            t.check(
                h,
                7,
                CapabilityKind::Filesystem,
                ResourceScope { start: 9, end: 20 },
                1,
                0
            ),
            Err(CapabilityError::ScopeDenied)
        );
        assert_eq!(
            t.check(h, 7, CapabilityKind::Filesystem, FS, 2, 0),
            Err(CapabilityError::ScopeDenied)
        );
    }

    #[test]
    fn delegation_cannot_amplify_and_teardown_revokes() {
        let mut t = CapabilityTable::<3>::new();
        let h = t
            .grant(
                1,
                CapabilityKind::Network,
                ResourceScope { start: 80, end: 90 },
                1,
                None,
                true,
            )
            .unwrap();
        let child = t
            .delegate(h, 1, 2, ResourceScope { start: 82, end: 85 }, 1, 0)
            .unwrap();
        assert_eq!(
            t.delegate(child, 2, 3, ResourceScope { start: 80, end: 85 }, 1, 0),
            Err(CapabilityError::ScopeDenied)
        );
        assert_eq!(t.revoke_owner(2), 1);
        assert_eq!(
            t.check(
                child,
                2,
                CapabilityKind::Network,
                ResourceScope { start: 82, end: 85 },
                1,
                0
            ),
            Err(CapabilityError::Revoked)
        );
    }

    /// The whole point of putting rights inside the handle: a read authority
    /// must never satisfy a write check on the same resource class. Under the
    /// V0.7 flat-bit model both mapped to "a Filesystem capability".
    #[test]
    fn read_rights_never_satisfy_a_write_check() {
        use crate::caps::{CAP_FS_READ, CAP_FS_WRITE};
        let reader = rights_from_bits(CAP_FS_READ);
        let fs = CapabilityKind::Filesystem.index();
        assert_eq!(reader[fs], rights::READ);
        assert_eq!(reader[fs] & rights::WRITE, 0);

        let mut t = CapabilityTable::<1>::new();
        let h = t
            .grant(
                1,
                CapabilityKind::Filesystem,
                ResourceScope::ANY,
                reader[fs],
                None,
                true,
            )
            .unwrap();
        assert!(t
            .check(
                h,
                1,
                CapabilityKind::Filesystem,
                ResourceScope::ANY,
                rights::READ,
                0
            )
            .is_ok());
        assert_eq!(
            t.check(
                h,
                1,
                CapabilityKind::Filesystem,
                ResourceScope::ANY,
                rights::WRITE,
                0
            ),
            Err(CapabilityError::ScopeDenied)
        );
        // And a writer is not implicitly a reader either.
        let writer = rights_from_bits(CAP_FS_WRITE);
        assert_eq!(writer[fs], rights::WRITE);
    }

    /// Distinct bits over one resource class must accumulate into one handle
    /// rather than colliding, and unrelated classes must stay empty.
    #[test]
    fn rights_accumulate_per_kind_and_default_deny() {
        use crate::caps::{CAP_FS_READ, CAP_FS_WRITE, CAP_SPAWN};
        let r = rights_from_bits(CAP_FS_READ | CAP_FS_WRITE | CAP_SPAWN);
        assert_eq!(
            r[CapabilityKind::Filesystem.index()],
            rights::READ | rights::WRITE
        );
        assert_eq!(r[CapabilityKind::Process.index()], rights::USE);
        assert_eq!(r[CapabilityKind::Network.index()], 0);
        assert_eq!(r[CapabilityKind::SystemAdministration.index()], 0);
        // No bits at all => no authority anywhere (default deny).
        assert_eq!(rights_from_bits(0), [0; CapabilityKind::COUNT]);
    }

    /// Kind indices are an ABI: userspace uses them to address its own
    /// handles, so the mapping must round-trip and stay total.
    #[test]
    fn kind_indices_round_trip() {
        for i in 0..CapabilityKind::COUNT {
            let kind = CapabilityKind::from_index(i).expect("index in range");
            assert_eq!(kind.index(), i);
        }
        assert_eq!(CapabilityKind::from_index(CapabilityKind::COUNT), None);
    }

    /// Self-restriction may only ever remove authority, and the handle it
    /// returns must replace the old one (which becomes unusable).
    #[test]
    fn restrict_only_narrows_and_invalidates_the_old_handle() {
        let mut t = CapabilityTable::<2>::new();
        let h = t
            .grant(
                4,
                CapabilityKind::Filesystem,
                ResourceScope { start: 0, end: 100 },
                rights::READ | rights::WRITE,
                Some(50),
                true,
            )
            .unwrap();
        let narrowed = t
            .restrict(
                h,
                4,
                ResourceScope { start: 10, end: 20 },
                // Ask for MORE than held (CONTROL) and a LATER expiry: both
                // requests must be ignored rather than honoured.
                rights::READ | rights::CONTROL,
                Some(9_000),
                0,
            )
            .unwrap();
        assert!(t
            .check(
                narrowed,
                4,
                CapabilityKind::Filesystem,
                ResourceScope { start: 10, end: 20 },
                rights::READ,
                0
            )
            .is_ok());
        // Amplification refused on every axis.
        assert_eq!(
            t.check(
                narrowed,
                4,
                CapabilityKind::Filesystem,
                ResourceScope { start: 10, end: 20 },
                rights::WRITE,
                0
            ),
            Err(CapabilityError::ScopeDenied)
        );
        assert_eq!(
            t.check(
                narrowed,
                4,
                CapabilityKind::Filesystem,
                ResourceScope { start: 0, end: 100 },
                rights::READ,
                0
            ),
            Err(CapabilityError::ScopeDenied)
        );
        // The original expiry survives the attempt to extend it.
        assert_eq!(
            t.check(
                narrowed,
                4,
                CapabilityKind::Filesystem,
                ResourceScope { start: 10, end: 20 },
                rights::READ,
                50
            ),
            Err(CapabilityError::Expired)
        );
        // The pre-restriction handle is gone for good.
        assert_eq!(
            t.check(
                h,
                4,
                CapabilityKind::Filesystem,
                ResourceScope { start: 0, end: 100 },
                rights::READ,
                0
            ),
            Err(CapabilityError::InvalidHandle)
        );
        assert_eq!(t.count_owned(4), 1);
    }

    /// Another process's handle may not be narrowed (or otherwise touched).
    #[test]
    fn restrict_requires_ownership() {
        let mut t = CapabilityTable::<1>::new();
        let h = t
            .grant(
                1,
                CapabilityKind::Network,
                ResourceScope::ANY,
                rights::USE,
                None,
                true,
            )
            .unwrap();
        assert_eq!(
            t.restrict(h, 2, ResourceScope::ANY, rights::USE, None, 0),
            Err(CapabilityError::NotOwner)
        );
        assert_eq!(t.count_owned(1), 1);
    }

    #[test]
    fn expiry_is_enforced() {
        let mut t = CapabilityTable::<1>::new();
        let h = t
            .grant(
                1,
                CapabilityKind::Device,
                ResourceScope::ANY,
                1,
                Some(5),
                true,
            )
            .unwrap();
        assert!(t
            .check(h, 1, CapabilityKind::Device, ResourceScope::ANY, 1, 4)
            .is_ok());
        assert_eq!(
            t.check(h, 1, CapabilityKind::Device, ResourceScope::ANY, 1, 5),
            Err(CapabilityError::Expired)
        );
    }
}
