//! `/sbin/init` as the kernel sees it (V0.10, INIT10-001/003).
//!
//! Init is an ordinary Ring 3 process with a few kernel-recognised roles: it
//! adopts orphans (`proc::ADOPT_PID`), it reports the services it supervises
//! (`svc_report`, syscall 39), and it says when its start phase is over
//! (`ready`), which the boot sequence waits for before the console starts.

use core::sync::atomic::{AtomicBool, Ordering};

static READY: AtomicBool = AtomicBool::new(false);

/// Is `pid` the running init (the registered adopter of orphans)?
pub fn is_init(pid: u64) -> bool {
    pid != 0 && pid == crate::proc::ADOPT_PID.load(Ordering::SeqCst)
}

/// Has init finished starting its services?
pub fn ready() -> bool {
    READY.load(Ordering::SeqCst)
}

/// Init's `ready` report was accepted (from `svc_report`).
pub(crate) fn mark_ready() {
    READY.store(true, Ordering::SeqCst);
}
