//! Kernel-owned V0.8 capability-handle registry.

use kernel_core::capability::{CapabilityHandle, CapabilityKind, CapabilityTable, ResourceScope};
use kernel_core::caps::*;
use spin::Mutex;

pub const HANDLE_SLOTS: usize = 16;
static TABLE: Mutex<CapabilityTable<256>> = Mutex::new(CapabilityTable::new());

pub fn handles_for(pid: u64, bits: u64) -> [CapabilityHandle; HANDLE_SLOTS] {
    let mut result = [CapabilityHandle::INVALID; HANDLE_SLOTS];
    let definitions = [
        (CAP_SPAWN, CapabilityKind::Process),
        (CAP_IPC, CapabilityKind::Service),
        (CAP_GUI, CapabilityKind::Gui),
        (CAP_DEV, CapabilityKind::Device),
        (CAP_FS_READ, CapabilityKind::Filesystem),
        (CAP_AUDIO, CapabilityKind::Audio),
        (CAP_SYS_ADMIN, CapabilityKind::SystemAdministration),
        (CAP_FS_WRITE, CapabilityKind::Filesystem),
        (CAP_NETWORK, CapabilityKind::Network),
        (CAP_PROC_CONTROL, CapabilityKind::Process),
        (CAP_SERVICE, CapabilityKind::Service),
    ];
    let mut out = 0;
    let mut table = TABLE.lock();
    for (bit, kind) in definitions {
        if bits & bit != 0 && out < result.len() {
            if let Ok(handle) = table.grant(pid, kind, ResourceScope::ANY, 1, None, true) {
                result[out] = handle;
                out += 1;
            }
        }
    }
    result
}

pub fn revoke_owner(pid: u64) {
    let _ = TABLE.lock().revoke_owner(pid);
}

pub fn check(
    handle: CapabilityHandle,
    owner: u64,
    kind: CapabilityKind,
    scope: ResourceScope,
    rights: u32,
    now: u64,
) -> Result<(), kernel_core::capability::CapabilityError> {
    TABLE.lock().check(handle, owner, kind, scope, rights, now)
}
