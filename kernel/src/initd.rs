//! `/sbin/init` as the kernel sees it (V0.10, INIT10-001/003; ADR-0023).
//!
//! Init is an ordinary Ring 3 process with a few kernel-recognised roles: it
//! is started at boot as pid 1, it adopts orphans (`proc::ADOPT_PID`), it
//! reports the services it supervises (`svc_report`, syscall 39), and it says
//! when its start phase is over (`ready`), which the boot sequence waits for
//! before the console starts. If it dies, the kernel ends every process
//! below it and starts a new init — at most `service::RESTART_LIMIT` times.
//!
//! Its authority is fixed here, not chosen by init: spawn, IPC, reading
//! files under `/etc`, and reporting services (0x413). Every service gets at
//! most that, and exactly what `/etc/init.conf` names.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use kernel_core::caps::{CAP_FS_READ, CAP_IPC, CAP_SERVICE, CAP_SPAWN};

pub const INIT_PATH: &str = "/sbin/init";
pub const INIT_CAPS: u64 = CAP_SPAWN | CAP_IPC | CAP_FS_READ | CAP_SERVICE;
pub const INIT_SANDBOX: &str = "/etc";

static READY: AtomicBool = AtomicBool::new(false);
static RESTARTS: AtomicU32 = AtomicU32::new(0);
/// A death not yet handled: the pid (0 = none), its status word, and how many
/// descendants were ended with it.
static DEAD_PID: AtomicU64 = AtomicU64::new(0);
static DEAD_STATUS: AtomicU64 = AtomicU64::new(0);
static DEAD_REAPED: AtomicU64 = AtomicU64::new(0);

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

/// Start `/sbin/init` (a child of the console) and make it the adopter.
/// Returns its pid. On failure the console still comes up, without init.
pub fn boot() -> Option<u64> {
    let sandbox = alloc::sync::Arc::new(alloc::vec![alloc::string::String::from(INIT_SANDBOX)]);
    match crate::user::load_with(INIT_PATH, INIT_CAPS, Some(sandbox)) {
        Ok(p) => {
            let pid = crate::proc::admit(p);
            crate::proc::ADOPT_PID.store(pid, Ordering::SeqCst);
            crate::serial_println!(
                "[ITISYOU:INIT] start pid={pid} path={INIT_PATH} caps={INIT_CAPS:#x} sandbox={INIT_SANDBOX}"
            );
            Some(pid)
        }
        Err(e) => {
            crate::serial_println!("[ITISYOU:INIT] start result=load_failed reason={e:?}");
            None
        }
    }
}

/// Init died (called from `proc::finish`, under the process table lock):
/// record it only; [`supervise`] acts on it outside any lock.
pub(crate) fn on_death(pid: u64, status: u64, descendants: u64) {
    DEAD_STATUS.store(status, Ordering::SeqCst);
    DEAD_REAPED.store(descendants, Ordering::SeqCst);
    DEAD_PID.store(pid, Ordering::SeqCst);
}

/// Restart a dead init, up to the ceiling (called from every background
/// slice, with no lock held).
pub fn supervise() {
    let pid = DEAD_PID.swap(0, Ordering::SeqCst);
    if pid == 0 {
        return;
    }
    let status = DEAD_STATUS.load(Ordering::SeqCst);
    let reaped = DEAD_REAPED.load(Ordering::SeqCst);
    // The dead init's slot is nobody's to collect.
    crate::proc::reap(pid);
    READY.store(false, Ordering::SeqCst);
    let restarts = RESTARTS.load(Ordering::SeqCst);
    let mut name = [0u8; 16];
    let status_name = status_name(status, &mut name);
    if restarts >= kernel_core::service::RESTART_LIMIT {
        crate::proc::ADOPT_PID.store(0, Ordering::SeqCst);
        crate::serial_println!(
            "[ITISYOU:INIT] died pid={pid} status={status_name} descendants_reaped={reaped} action=give_up restarts={restarts}"
        );
        return;
    }
    let restarts = restarts + 1;
    RESTARTS.store(restarts, Ordering::SeqCst);
    crate::serial_println!(
        "[ITISYOU:INIT] died pid={pid} status={status_name} descendants_reaped={reaped} action=restart restarts={restarts}"
    );
    if let Some(new) = boot() {
        crate::serial_println!(
            "[ITISYOU:INIT] restart pid={new} path={INIT_PATH} caps={INIT_CAPS:#x} sandbox={INIT_SANDBOX} restarts={restarts}"
        );
    } else {
        crate::proc::ADOPT_PID.store(0, Ordering::SeqCst);
    }
}

/// `exit:<n>`, `fault:<v>` or `killed`.
fn status_name(word: u64, buf: &mut [u8; 16]) -> &str {
    use core::fmt::Write;
    struct W<'a>(&'a mut [u8; 16], usize);
    impl Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = s.as_bytes();
            let end = (self.1 + b.len()).min(self.0.len());
            self.0[self.1..end].copy_from_slice(&b[..end - self.1]);
            self.1 = end;
            Ok(())
        }
    }
    let mut w = W(buf, 0);
    let _ = match kernel_core::procstatus::decode(word) {
        Some(kernel_core::procstatus::Status::Exited(c)) => write!(w, "exit:{c}"),
        Some(kernel_core::procstatus::Status::Faulted(v)) => write!(w, "fault:{v}"),
        Some(kernel_core::procstatus::Status::Killed) => write!(w, "killed"),
        None => write!(w, "unknown"),
    };
    let n = w.1;
    core::str::from_utf8(&buf[..n]).unwrap_or("?")
}
