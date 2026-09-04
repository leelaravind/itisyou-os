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
