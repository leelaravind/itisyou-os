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
    // V0.11: a command meant for the dead init must not reach the next one;
    // whoever waits for its acknowledgement times out and fails safe.
    clear_mailbox();
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

// --- The kernel -> init mailbox (V0.11, ACT11-002, ADR-0024) -----------------
//
// One command at a time, posted only by the kernel (the console's `approve`
// path), read and acknowledged only by the live init through syscall 44.
// IPC is not used: it carries no sender identity, and any holder of the IPC
// capability could read or forge a command there.

use kernel_core::initctl::{Ack, Command, Op, ACK_LEN, COMMAND_LEN};

struct Mailbox {
    pending: Option<Command>,
    /// Init has fetched `pending` and owes an acknowledgement.
    delivered: bool,
    ack: Option<Ack>,
    next_seq: u32,
}

static MAILBOX: crate::sync::Mutex<Mailbox> = crate::sync::Mutex::new(Mailbox {
    pending: None,
    delivered: false,
    ack: None,
    next_seq: 1,
});

/// Post a command for init: its sequence number, or `None` when a command is
/// already outstanding or the name is malformed.
pub fn post(op: Op, name: &str) -> Option<u32> {
    let mut mb = MAILBOX.lock();
    if mb.pending.is_some() {
        return None;
    }
    let seq = mb.next_seq;
    let cmd = Command::new(seq, op, name)?;
    mb.next_seq = mb.next_seq.wrapping_add(1).max(1);
    mb.pending = Some(cmd);
    mb.delivered = false;
    // An acknowledgement of an earlier command stays until its waiter takes
    // it (acks are matched by sequence number): a stop posted by the
    // verification watch must not erase the retry's ack.
    drop(mb);
    crate::serial_println!(
        "[ITISYOU:INIT] mailbox post seq={seq} op={} name={name}",
        op.name()
    );
    Some(seq)
}

/// Init's acknowledgement of `seq`, once it has sent one.
pub fn take_ack(seq: u32) -> Option<Ack> {
    let mut mb = MAILBOX.lock();
    match mb.ack {
        Some(a) if a.seq == seq => mb.ack.take(),
        _ => None,
    }
}

/// Give up on `seq` (no acknowledgement in time): nothing may act on it now.
pub fn withdraw(seq: u32) {
    let mut mb = MAILBOX.lock();
    if mb.pending.is_some_and(|c| c.seq == seq) {
        mb.pending = None;
        mb.delivered = false;
    }
    if mb.ack.is_some_and(|a| a.seq == seq) {
        mb.ack = None;
    }
}

fn clear_mailbox() {
    let mut mb = MAILBOX.lock();
    let had = mb.pending.is_some() || mb.ack.is_some();
    mb.pending = None;
    mb.delivered = false;
    mb.ack = None;
    drop(mb);
    if had {
        crate::serial_println!("[ITISYOU:INIT] mailbox cleared reason=init_died");
    }
}

/// init_ctl(op, buf, len) (V0.11, syscall 44; Service ADMIN checked by the
/// dispatcher): op 0 fetches the pending command into `buf` (24 bytes, or
/// `ERR_AGAIN` when there is none); op 1 acknowledges it (16 bytes). Only the
/// live init may call it (`not_init`, audited), and an acknowledgement must
/// name the command init fetched.
pub fn sys_init_ctl(op: u64, buf: u64, len: u64) -> u64 {
    use crate::syscall::{ERR_2BIG, ERR_AGAIN, ERR_INVAL, ERR_PERM};
    let caller = crate::syscall::CURRENT_PID.load(Ordering::SeqCst);
    if !is_init(caller) {
        crate::serial_println!("[ITISYOU:INIT] init_ctl refused pid={caller} reason=not_init");
        crate::audit::denied_reason("init_ctl", CAP_SERVICE, "not_init");
        return ERR_PERM;
    }
    match op {
        0 => {
            if len < COMMAND_LEN as u64 {
                return ERR_2BIG;
            }
            let cmd = {
                let mb = MAILBOX.lock();
                match mb.pending {
                    Some(c) if !mb.delivered => c,
                    _ => return ERR_AGAIN,
                }
            };
            match crate::syscall::copy_to_user(buf, &cmd.encode()) {
                Ok(n) => {
                    let mut mb = MAILBOX.lock();
                    if mb.pending.is_some_and(|c| c.seq == cmd.seq) {
                        mb.delivered = true;
                    }
                    n
                }
                Err(e) => e,
            }
        }
        1 => {
            if len != ACK_LEN as u64 {
                return ERR_INVAL;
            }
            let bytes = match crate::syscall::copy_from_user(buf, len, len) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let Ok(ack) = Ack::decode(&bytes) else {
                return ERR_INVAL;
            };
            let mut mb = MAILBOX.lock();
            match mb.pending {
                Some(c) if mb.delivered && c.seq == ack.seq => {
                    mb.pending = None;
                    mb.delivered = false;
                    mb.ack = Some(ack);
                    drop(mb);
                    crate::serial_println!(
                        "[ITISYOU:INIT] mailbox ack seq={} ok={} pid={}",
                        ack.seq,
                        ack.ok,
                        ack.pid
                    );
                    0
                }
                _ => ERR_INVAL,
            }
        }
        _ => ERR_INVAL,
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
