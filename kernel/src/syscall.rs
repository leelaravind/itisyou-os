//! Syscall ABI + `syscall`/`sysret` fast path (V0.2/V0.3, ADR-0004/0006).
//!
//! ABI (registers): rax = number, rdi/rsi/rdx = args, rax = return value.
//! rcx and r11 are clobbered by the hardware (return RIP / RFLAGS).
//!
//! | nr | name    | args                  | returns                        |
//! |----|---------|-----------------------|--------------------------------|
//! | 0  | write   | fd(=1), ptr, len      | bytes written, or ERR_*        |
//! | 1  | exit    | code                  | does not return                |
//! | 2  | yield   | —                     | 0 (after other procs run)      |
//! | 3  | getpid  | —                     | pid                            |
//! | 4  | spawn   | path_ptr, path_len    | child pid, or ERR_*            |
//! | 5  | wait    | pid                   | child exit/fault status, ERR_* |
//! | 6  | msg_send| ch, ptr, len          | bytes queued, or ERR_*         |
//! | 7  | msg_recv| ch, ptr, len          | bytes received, or ERR_*       |
//! | 15 | cap_list| ptr, capacity         | handles copied, or ERR_*       |
//! | 16 |cap_check| handle, kind, scope   | 0 if valid, else ERR_PERM      |
//! |17|cap_revoke | kind                  | 0, or ERR_PERM / ERR_INVAL     |
//! |18|cap_restrict| kind, rights, ttl    | 0, or ERR_PERM / ERR_INVAL     |
//! | 35 | args    | buf_ptr, buf_len      | block length, or ERR_2BIG/FAULT|
//! |36|spawn_args| path_ptr, path_len, req | child pid, or ERR_*           |
//!
//! Capability enforcement (V0.8): every syscall past the basic runtime
//! (write/exit/yield/getpid) resolves the caller's handle for the resource
//! class it touches and revalidates it in the kernel capability table —
//! owner, generation, kind, scope, rights and expiry — on every single call.
//! A static per-process bitmask is no longer consulted, which is what lets
//! revocation, expiry and scope narrowing take effect immediately.
//!
//! Unknown numbers return [`ERR_NOSYS`]; invalid user pointers return
//! [`ERR_FAULT`] after validation against the ACTIVE address space — the
//! kernel never dereferences unvalidated user addresses.

use crate::user::transition;
use crate::user::{USER_MAX, USER_MIN};
use core::arch::naked_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

pub const SYS_WRITE: u64 = 0;
pub const SYS_EXIT: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_GETPID: u64 = 3;
pub const SYS_SPAWN: u64 = 4;
pub const SYS_WAIT: u64 = 5;
pub const SYS_MSG_SEND: u64 = 6;
pub const SYS_MSG_RECV: u64 = 7;
pub const SYS_GUI_CREATE: u64 = 8;
pub const SYS_GUI_FILL: u64 = 9;
pub const SYS_GUI_TEXT: u64 = 10;
pub const SYS_GUI_PRESENT: u64 = 11;
pub const SYS_DEVINFO: u64 = 12;
pub const SYS_FS_READ: u64 = 13;
pub const SYS_SPAWN_CAPS: u64 = 14;
/// Return the caller's opaque V0.8 handle list (ptr, capacity in handles).
pub const SYS_CAP_LIST: u64 = 15;
/// Validate one handle for a kind and inclusive resource scope.
pub const SYS_CAP_CHECK: u64 = 16;
/// Permanently drop the caller's own handle for a resource kind.
pub const SYS_CAP_REVOKE: u64 = 17;
/// Narrow the caller's own handle for a resource kind (rights subset, scope
/// subset, optional expiry). Never amplifies.
pub const SYS_CAP_RESTRICT: u64 = 18;
/// Monotonic tick count since boot (V0.8). A long-running service has to pace
/// itself in REAL time: a daemon that counts its own scheduling passes is
/// measuring how loaded the machine is, not how much time has gone by, so its
/// heartbeat cadence swings by orders of magnitude with load. This conveys no
/// authority (a free-running counter, no wall clock, nothing about any other
/// process), so like `getpid` it needs no capability.
pub const SYS_UPTIME: u64 = 19;
/// Interface facts: MAC, address plan, link state (V0.8).
pub const SYS_NET_INFO: u64 = 20;
/// Bind a UDP port; the port is the scoped resource the handle must cover.
pub const SYS_UDP_BIND: u64 = 21;
/// Send a datagram from a bound socket.
pub const SYS_UDP_SEND: u64 = 22;
/// Take the oldest queued datagram from a bound socket.
pub const SYS_UDP_RECV: u64 = 23;
/// Release a bound socket.
pub const SYS_UDP_CLOSE: u64 = 24;
/// Resolve a host name to an IPv4 address over DNS.
pub const SYS_NET_RESOLVE: u64 = 25;
/// Create or overwrite a file in the persistent store (V0.8).
pub const SYS_FS_WRITE: u64 = 26;
/// Delete a file from the persistent store.
pub const SYS_FS_DELETE: u64 = 27;
/// List the persistent store's directory.
pub const SYS_FS_LIST: u64 = 28;
/// Open a TCP connection (V0.9); the destination port is the scoped resource.
pub const SYS_TCP_CONNECT: u64 = 29;
/// Queue bytes on a connection the caller owns.
pub const SYS_TCP_SEND: u64 = 30;
/// Read received bytes from a connection the caller owns.
pub const SYS_TCP_RECV: u64 = 31;
/// Orderly close (FIN after queued data).
pub const SYS_TCP_CLOSE: u64 = 32;
/// Connection state, for polling a non-blocking connect or close.
pub const SYS_TCP_STATE: u64 = 33;
/// Passive open on a local port (the scoped resource). The descriptor waits
/// in LISTEN and becomes the connection when a peer's SYN arrives.
pub const SYS_TCP_LISTEN: u64 = 34;
/// Copy the caller's OWN argument block (V0.10, `kernel_core::progargs`
/// encoding) into a user buffer. Like `getpid` and `uptime` it needs no
/// capability: the block was chosen by whoever launched this process and
/// handed to it, so reading it back conveys no authority and reveals nothing
/// about any other process.
pub const SYS_ARGS: u64 = 35;
/// `spawn_caps` plus an argument block for the child (V0.10). Same Process
/// capability gate and the same delegation rule as `spawn_caps`; the block is
/// validated by the same code as the console's `run … -- args`.
pub const SYS_SPAWN_ARGS: u64 = 36;

pub const ERR_NOSYS: u64 = u64::MAX;
pub const ERR_FAULT: u64 = u64::MAX - 1;
pub const ERR_BADF: u64 = u64::MAX - 2;
pub const ERR_NOENT: u64 = u64::MAX - 3;
pub const ERR_AGAIN: u64 = u64::MAX - 4;
pub const ERR_INVAL: u64 = u64::MAX - 5;
pub const ERR_2BIG: u64 = u64::MAX - 6;
pub const ERR_PERM: u64 = u64::MAX - 7;

/// Cap on a network request buffer copied from user memory: a full-MTU UDP
/// payload plus the six-byte destination prefix.
const NET_REQ_MAX: u64 = 6 + 1472;

/// Cap on the bytes one `tcp_send` copies from user memory.
const TCP_SEND_MAX: u64 = 2048;

/// Cap on a single userspace filesystem write. Bounded so one call cannot ask
/// the kernel to buffer an arbitrary amount of user memory.
const FS_WRITE_MAX: u64 = 64 * 1024;

/// Cap on a GUI request buffer copied from user memory.
const GUI_REQ_MAX: u64 = 256;

/// Cap for a single write so a hostile length cannot stall the kernel.
const WRITE_MAX: u64 = 64 * 1024;

const KSTACK_SIZE: usize = 32 * 1024;

// Referenced by name from the naked entry stub.
#[unsafe(no_mangle)]
static mut SYSCALL_USER_RSP: u64 = 0;
#[unsafe(no_mangle)]
static mut SYSCALL_KSTACK_TOP: u64 = 0;
/// Resumable-context snapshot taken at every syscall entry (before the
/// dispatcher can clobber callee-saved user registers): [rip, rflags, rbx,
/// rbp, r12, r13, r14, r15]. `yield` reads this to save the process context.
#[unsafe(no_mangle)]
static mut SYSCALL_SNAP: [u64; 8] = [0; 8];

/// The stack every syscall runs on, on a guard page (V0.10, HARD10-003).
/// V0.9 guarded the RSP0 and double-fault stacks but not this one — and this
/// is the stack the first TCP integration overflowed into the capability
/// table. Page-aligned so the guard is a whole page; `gdt::arm_stack_guards`
/// unmaps it with the others.
#[repr(C, align(4096))]
struct GuardedSyscallStack {
    guard: [u8; 4096],
    stack: [u8; KSTACK_SIZE],
}
static mut SYSCALL_STACK: GuardedSyscallStack = GuardedSyscallStack {
    guard: [0; 4096],
    stack: [0; KSTACK_SIZE],
};

/// First byte of the syscall stack's guard page (address-of only).
pub fn syscall_stack_guard() -> u64 {
    (&raw const SYSCALL_STACK) as u64
}

/// Top of the syscall stack (16-byte aligned).
pub fn syscall_stack_top() -> u64 {
    (syscall_stack_guard() + 4096 + KSTACK_SIZE as u64) & !0xF
}

/// PID of the currently running user process (0 = none).
pub static CURRENT_PID: AtomicU64 = AtomicU64::new(0);

/// Capability bits of the currently running user process.
///
/// NOT an enforcement input as of V0.8. Manifests, service definitions and
/// `spawn_caps` requests are still written in terms of these bits, so they are
/// kept to compute what a child ASKS for; what a child actually RECEIVES is
/// decided by delegating from the parent's live handles in the capability
/// table. No syscall consults this value to allow an operation — see
/// [`require_handle`].
pub static CURRENT_CAPS: AtomicU64 = AtomicU64::new(0);

/// FS sandbox of the currently running process: reads allowed only under
/// these prefixes. `None` = unrestricted (trusted launch).
static CURRENT_SANDBOX: spin::Mutex<
    Option<alloc::sync::Arc<alloc::vec::Vec<alloc::string::String>>>,
> = spin::Mutex::new(None);
/// The running process's handle set, indexed by `CapabilityKind::index()`.
/// Published for the quantum's duration; every privileged syscall resolves its
/// authority from here and revalidates it in the kernel capability table.
static CURRENT_HANDLES: spin::Mutex<crate::capability::HandleSet> =
    spin::Mutex::new(crate::capability::EMPTY_HANDLES);

pub fn set_current_handles(handles: &crate::capability::HandleSet) {
    *CURRENT_HANDLES.lock() = *handles;
}

pub fn clear_current_handles() {
    *CURRENT_HANDLES.lock() = crate::capability::EMPTY_HANDLES;
}

/// Snapshot of the running process's handles (used when delegating to a child).
pub fn current_handles() -> crate::capability::HandleSet {
    *CURRENT_HANDLES.lock()
}

/// Replace one slot after a process narrows or drops its own authority.
fn set_current_handle(index: usize, handle: kernel_core::capability::CapabilityHandle) {
    if let Some(slot) = CURRENT_HANDLES.lock().get_mut(index) {
        *slot = handle;
    }
}

pub fn set_current_sandbox(
    prefixes: Option<alloc::sync::Arc<alloc::vec::Vec<alloc::string::String>>>,
) {
    *CURRENT_SANDBOX.lock() = prefixes;
}

/// The running process's argument block (V0.10), published for the quantum
/// the same way as its handles and sandbox. `None` between quanta.
static CURRENT_ARGS: spin::Mutex<Option<crate::user::Args>> = spin::Mutex::new(None);

pub fn set_current_args(args: Option<crate::user::Args>) {
    *CURRENT_ARGS.lock() = args;
}

fn current_sandbox() -> Option<alloc::sync::Arc<alloc::vec::Vec<alloc::string::String>>> {
    CURRENT_SANDBOX.lock().clone()
}

/// The current process's sandbox, for inheritance by a spawned child (a child
/// can never widen its parent's FS view).
pub fn current_sandbox_for_child(
) -> Option<alloc::sync::Arc<alloc::vec::Vec<alloc::string::String>>> {
    current_sandbox()
}

/// Capability gate (V0.8): resolve the running process's handle for `kind` and
/// revalidate it in the kernel table for exactly the `rights` and `scope` this
/// operation needs.
///
/// This replaced the V0.7 static-bitmask test. The distinction matters: a
/// bitmask is fixed for the process's lifetime, so revocation, expiry and
/// scope narrowing could only ever be advisory. Going through the table on
/// every call means an authority that was revoked, has expired, was narrowed,
/// or belongs to another process is refused at the moment of use.
///
/// Denials are audited with the precise reason (`revoked` vs `expired` vs
/// `scope_denied` …) so a refusal is diagnosable instead of a bare ERR_PERM.
fn require_handle(
    kind: kernel_core::capability::CapabilityKind,
    rights: u32,
    scope: kernel_core::capability::ResourceScope,
    action: &'static str,
) -> Result<(), u64> {
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    let handle = CURRENT_HANDLES.lock()[kind.index()];
    if handle == kernel_core::capability::CapabilityHandle::INVALID {
        // Distinguish "never granted, or already given up" from "presented a
        // handle the table rejected" — the audit trail is only useful if the
        // reason is precise.
        crate::audit::denied_capability(action, kind, "no_handle");
        return Err(ERR_PERM);
    }
    match crate::capability::check(handle, pid, kind, scope, rights, crate::interrupts::ticks()) {
        Ok(()) => Ok(()),
        Err(e) => {
            crate::audit::denied_capability(action, kind, crate::capability::reason_name(e));
            Err(ERR_PERM)
        }
    }
}

/// Authority over a whole resource class (no meaningful sub-resource).
fn require_any(
    kind: kernel_core::capability::CapabilityKind,
    rights: u32,
    action: &'static str,
) -> Result<(), u64> {
    require_handle(
        kind,
        rights,
        kernel_core::capability::ResourceScope::ANY,
        action,
    )
}

/// Authority over one addressable sub-resource (an IPC channel, a device
/// index, a network port): the handle's scope must actually contain it.
fn require_scoped(
    kind: kernel_core::capability::CapabilityKind,
    rights: u32,
    resource: u64,
    action: &'static str,
) -> Result<(), u64> {
    require_handle(
        kind,
        rights,
        kernel_core::capability::ResourceScope {
            start: resource,
            end: resource,
        },
        action,
    )
}

/// Count of rejected unknown-syscall attempts (diagnostic evidence).
pub static NOSYS_COUNT: AtomicU64 = AtomicU64::new(0);

/// Program the syscall MSRs (B140). Requires gdt::init() done.
pub fn init() {
    let selectors = crate::gdt::selectors();
    // SAFETY: standard syscall MSR setup; selector layout is asserted by
    // gdt::init; the entry stub upholds the syscall ABI contract.
    unsafe {
        Efer::update(|flags| flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
        Star::write(
            selectors.user_code,
            selectors.user_data,
            selectors.kernel_code,
            selectors.kernel_data,
        )
        .expect("GDT layout incompatible with sysret");
        let entry: unsafe extern "C" fn() = syscall_entry;
        LStar::write(VirtAddr::new(entry as usize as u64));
        SFMask::write(
            RFlags::INTERRUPT_FLAG
                | RFlags::TRAP_FLAG
                | RFlags::DIRECTION_FLAG
                | RFlags::ALIGNMENT_CHECK,
        );
        let top = syscall_stack_top();
        core::ptr::write(&raw mut SYSCALL_KSTACK_TOP, top);
    }
}

/// Read the syscall-entry snapshot as (rip, rflags, rbx, rbp, r12..r15) plus
/// the saved user RSP — used by `yield` to persist a resumable context.
pub fn snapshot() -> ([u64; 8], u64) {
    // SAFETY: single CPU; snapshot written by the entry stub of the syscall
    // currently executing, read only here on that same path.
    unsafe {
        (
            core::ptr::read(&raw const SYSCALL_SNAP),
            core::ptr::read(&raw const SYSCALL_USER_RSP),
        )
    }
}

/// Hardware entry point. On entry: rcx = user RIP, r11 = user RFLAGS,
/// rsp = USER stack (untrusted!), IF masked via SFMASK.
#[unsafe(naked)]
unsafe extern "C" fn syscall_entry() {
    naked_asm!(
        "mov [rip + SYSCALL_USER_RSP], rsp",
        "mov rsp, [rip + SYSCALL_KSTACK_TOP]",
        // Snapshot the resumable user context BEFORE the dispatcher runs
        // (rbx/rbp/r12-r15 still hold userspace's callee-saved values).
        "mov [rip + SYSCALL_SNAP + 0], rcx",
        "mov [rip + SYSCALL_SNAP + 8], r11",
        "mov [rip + SYSCALL_SNAP + 16], rbx",
        "mov [rip + SYSCALL_SNAP + 24], rbp",
        "mov [rip + SYSCALL_SNAP + 32], r12",
        "mov [rip + SYSCALL_SNAP + 40], r13",
        "mov [rip + SYSCALL_SNAP + 48], r14",
        "mov [rip + SYSCALL_SNAP + 56], r15",
        "push rcx", // user RIP
        "push r11", // user RFLAGS
        "push qword ptr [rip + SYSCALL_USER_RSP]",
        "sub rsp, 8", // 16-byte alignment for the call
        "mov rcx, rax",
        "call {dispatch}",
        "add rsp, 8",
        "pop qword ptr [rip + SYSCALL_USER_RSP]",
        "pop r11",
        "pop rcx",
        "mov rsp, [rip + SYSCALL_USER_RSP]",
        "sysretq",
        dispatch = sym syscall_dispatch,
    )
}

extern "C" fn syscall_dispatch(a1: u64, a2: u64, a3: u64, nr: u64) -> u64 {
    use kernel_core::capability::{rights, CapabilityKind};
    // Capability enforcement (V0.8): the basic runtime (write/exit/yield/
    // getpid) needs no capability; every other syscall is default-deny and
    // must present a live, owned, in-scope, unexpired handle for the resource
    // class it touches. Denials return ERR_PERM and are audited with a reason.
    match nr {
        SYS_WRITE => sys_write(a1, a2, a3),
        SYS_EXIT => transition::abort_exit(a1),
        SYS_YIELD => {
            // Persist the resumable context (rax=0 on return) and hand the
            // run-loop back control so another process can run.
            transition::save_yield_context(0);
            transition::abort_yield();
        }
        SYS_GETPID => CURRENT_PID.load(Ordering::SeqCst),
        SYS_UPTIME => crate::interrupts::ticks(),
        // No capability: a process reading its own arguments (see SYS_ARGS).
        SYS_ARGS => sys_args(a1, a2),
        SYS_SPAWN => match require_any(CapabilityKind::Process, rights::USE, "spawn") {
            Ok(()) => crate::proc::sys_spawn(a1, a2),
            Err(e) => e,
        },
        SYS_SPAWN_CAPS => match require_any(CapabilityKind::Process, rights::USE, "spawn_caps") {
            Ok(()) => crate::proc::sys_spawn_caps(a1, a2, a3),
            Err(e) => e,
        },
        // The capability gate comes first, exactly as for spawn_caps: a
        // process without the Process capability learns nothing about
        // whether its argument block would have been accepted.
        SYS_SPAWN_ARGS => match require_any(CapabilityKind::Process, rights::USE, "spawn_args") {
            Ok(()) => crate::proc::sys_spawn_args(a1, a2, a3),
            Err(e) => e,
        },
        SYS_WAIT => match require_any(CapabilityKind::Process, rights::USE, "wait") {
            Ok(()) => crate::proc::sys_wait(a1),
            Err(e) => e,
        },
        // The channel id is the scoped resource: a service handed a
        // channel-scoped handle cannot talk on any other channel.
        SYS_MSG_SEND => {
            match require_scoped(CapabilityKind::Service, rights::USE, a1, "msg_send") {
                Ok(()) => crate::ipc::sys_msg_send(a1, a2, a3),
                Err(e) => e,
            }
        }
        SYS_MSG_RECV => {
            match require_scoped(CapabilityKind::Service, rights::USE, a1, "msg_recv") {
                Ok(()) => crate::ipc::sys_msg_recv(a1, a2, a3),
                Err(e) => e,
            }
        }
        SYS_GUI_CREATE => match require_any(CapabilityKind::Gui, rights::USE, "gui_create") {
            Ok(()) => sys_gui_create(a1, a2),
            Err(e) => e,
        },
        SYS_GUI_FILL => match require_any(CapabilityKind::Gui, rights::USE, "gui_fill") {
            Ok(()) => sys_gui_fill(a1, a2, a3),
            Err(e) => e,
        },
        SYS_GUI_TEXT => match require_any(CapabilityKind::Gui, rights::USE, "gui_text") {
            Ok(()) => sys_gui_text(a1, a2, a3),
            Err(e) => e,
        },
        SYS_GUI_PRESENT => match require_any(CapabilityKind::Gui, rights::USE, "gui_present") {
            Ok(()) => sys_gui_present(a1),
            Err(e) => e,
        },
        // Device index is the scoped resource.
        SYS_DEVINFO => match require_scoped(CapabilityKind::Device, rights::READ, a1, "devinfo") {
            Ok(()) => sys_devinfo(a1, a2, a3),
            Err(e) => e,
        },
        SYS_FS_READ => match require_any(CapabilityKind::Filesystem, rights::READ, "fs_read") {
            Ok(()) => sys_fs_read(a1, a2, a3),
            Err(e) => e,
        },
        // Network. Binding is scoped to the port, so a handle narrowed to a
        // port range cannot listen outside it; the datagram calls then check
        // the socket's OWN port, which is the resource actually being used.
        SYS_NET_INFO => match require_any(CapabilityKind::Network, rights::READ, "net_info") {
            Ok(()) => sys_net_info(a1, a2),
            Err(e) => e,
        },
        SYS_UDP_BIND => {
            match require_scoped(CapabilityKind::Network, rights::USE, a1, "udp_bind") {
                Ok(()) => sys_udp_bind(a1),
                Err(e) => e,
            }
        }
        SYS_UDP_SEND => sys_udp_send(a1, a2, a3),
        SYS_UDP_RECV => sys_udp_recv(a1, a2, a3),
        SYS_UDP_CLOSE => sys_udp_close(a1),
        SYS_NET_RESOLVE => match require_any(CapabilityKind::Network, rights::USE, "net_resolve") {
            Ok(()) => sys_net_resolve(a1, a2, a3),
            Err(e) => e,
        },
        // Filesystem mutation. WRITE is a distinct right from READ, so a
        // process granted only `fs_read` cannot modify anything even inside
        // its own sandbox — and the sandbox still applies on top, because a
        // capability says what class of thing you may do, not where.
        SYS_FS_WRITE => match require_any(CapabilityKind::Filesystem, rights::WRITE, "fs_write") {
            Ok(()) => sys_fs_write(a1, a2, a3),
            Err(e) => e,
        },
        SYS_FS_DELETE => {
            match require_any(CapabilityKind::Filesystem, rights::WRITE, "fs_delete") {
                Ok(()) => sys_fs_delete(a1, a2),
                Err(e) => e,
            }
        }
        SYS_FS_LIST => match require_any(CapabilityKind::Filesystem, rights::READ, "fs_list") {
            Ok(()) => sys_fs_list(a1, a2),
            Err(e) => e,
        },
        SYS_TCP_CONNECT => sys_tcp_connect(a1, a2),
        SYS_TCP_SEND => sys_tcp_send(a1, a2, a3),
        SYS_TCP_RECV => sys_tcp_recv(a1, a2, a3),
        SYS_TCP_CLOSE => sys_tcp_close(a1),
        SYS_TCP_STATE => sys_tcp_state(a1),
        SYS_TCP_LISTEN => sys_tcp_listen(a1),
        SYS_CAP_LIST => sys_cap_list(a1, a2),
        SYS_CAP_CHECK => sys_cap_check(a1, a2, a3),
        SYS_CAP_REVOKE => sys_cap_revoke(a1),
        SYS_CAP_RESTRICT => sys_cap_restrict(a1, a2, a3),
        _ => {
            NOSYS_COUNT.fetch_add(1, Ordering::SeqCst);
            ERR_NOSYS
        }
    }
}

/// args(buf_ptr, buf_len): copy the caller's argument block (V0.10).
///
/// Returns the block length (0 for a process launched without arguments).
/// All-or-nothing: if `buf_len` is smaller than the block the call returns
/// `ERR_2BIG` and writes NOTHING — a truncated block could end mid-argument
/// and be misread as a shorter, different argument list. A caller that wants
/// the whole block can always pass `progargs::MAX_BLOCK` (512) bytes. The copy
/// goes through `copy_to_user`, so the buffer is validated against the active
/// address space and written inside the declared SMAP window.
fn sys_args(buf_ptr: u64, buf_len: u64) -> u64 {
    let Some(args) = CURRENT_ARGS.lock().clone() else {
        // Only reachable if the syscall layer ran with no process published.
        return ERR_INVAL;
    };
    let block = args.bytes();
    if block.len() as u64 > buf_len {
        return ERR_2BIG;
    }
    match copy_to_user(buf_ptr, block) {
        Ok(n) => n,
        Err(e) => e,
    }
}

/// net_info(buf_ptr, buf_len): copy a fixed 20-byte interface record.
///
/// Layout: `[mac:6, ip:4, netmask:4, gateway:4, dns_low:1, link_up:1]` — the
/// DNS server's low octet is enough for a program to see it is configured
/// without handing out a second full address it has no use for.
fn sys_net_info(buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_len < 20 {
        return ERR_INVAL;
    }
    let (ip, mask, gw, dns) = crate::net::address();
    let mac = crate::net::mac();
    let mut record = [0u8; 20];
    record[0..6].copy_from_slice(&mac.octets());
    record[6..10].copy_from_slice(&ip.octets());
    record[10..14].copy_from_slice(&mask.octets());
    record[14..18].copy_from_slice(&gw.octets());
    record[18] = dns.octets()[3];
    record[19] = u8::from(crate::net::is_up());
    match copy_to_user(buf_ptr, &record) {
        Ok(_) => 20,
        Err(e) => e,
    }
}

/// udp_bind(port): bind a UDP port, returning a socket descriptor.
///
/// The capability check happened in the dispatcher, scoped to `port`: a
/// process whose network handle was narrowed to one port range cannot bind
/// outside it, and the refusal is audited with that port.
fn sys_udp_bind(port: u64) -> u64 {
    if port == 0 || port > u16::MAX as u64 {
        return ERR_INVAL;
    }
    if !crate::net::is_up() {
        return ERR_NOENT;
    }
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    match crate::net::socket::bind(pid, port as u16) {
        Ok(index) => {
            crate::audit::allowed(
                "udp_bind",
                kernel_core::caps::CAP_NETWORK,
                Some(alloc::format!("port={port}")),
            );
            index as u64
        }
        Err(crate::net::socket::BindError::InUse) => ERR_PERM,
        Err(_) => ERR_INVAL,
    }
}

/// The socket's own port, or an error if the caller does not own it.
///
/// Every datagram call re-checks the network capability against THIS port
/// rather than trusting the descriptor: a handle that was narrowed or revoked
/// after the bind must stop working at the next use, which is the whole point
/// of handles over static bits.
fn socket_port(index: u64, action: &'static str) -> Result<u16, u64> {
    use kernel_core::capability::{rights, CapabilityKind};
    // Class-level authority first, ownership second. Order matters for the
    // audit trail: a process with no network handle must be refused as a
    // capability denial (ERR_PERM, recorded with a reason), not as a bad
    // descriptor — otherwise a real gate failure would be indistinguishable
    // from someone passing a number they never bound.
    require_any(CapabilityKind::Network, rights::USE, action)?;
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    let port = crate::net::socket::port_of(index as usize, pid).ok_or(ERR_BADF)?;
    // Then the port itself: a handle narrowed or revoked after the bind stops
    // working at the next use, which is the whole point of handles over bits.
    require_scoped(CapabilityKind::Network, rights::USE, port as u64, action)?;
    Ok(port)
}

/// udp_send(sock, req_ptr, req_len): send from a bound socket.
///
/// Request layout: `[dst_ip:4, dst_port:2, payload...]`. Passing the
/// destination in the buffer rather than packed into a register keeps the ABI
/// readable and leaves room for a longer address family later.
fn sys_udp_send(sock: u64, req_ptr: u64, req_len: u64) -> u64 {
    let port = match socket_port(sock, "udp_send") {
        Ok(p) => p,
        Err(e) => return e,
    };
    if req_len < 6 {
        return ERR_INVAL;
    }
    let req = match copy_from_user(req_ptr, req_len, NET_REQ_MAX) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dst = kernel_core::net::ipv4::Ipv4Addr([req[0], req[1], req[2], req[3]]);
    let dst_port = u16::from_le_bytes([req[4], req[5]]);
    if dst_port == 0 {
        return ERR_INVAL;
    }
    // Poll first: a reply to an earlier ARP request may be sitting in the
    // ring, in which case this send succeeds immediately instead of needing
    // another round through userspace.
    crate::net::poll();
    match crate::net::try_send_udp(dst, dst_port, port, &req[6..]) {
        Ok(()) => req_len - 6,
        // Non-blocking by design: waiting for ARP here would spin with
        // interrupts disabled and freeze the machine. The caller yields and
        // retries, which costs nothing and keeps the system scheduling.
        Err(crate::net::SendError::Unresolved) => ERR_AGAIN,
        Err(crate::net::SendError::Failed) => ERR_INVAL,
    }
}

/// udp_recv(sock, buf_ptr, buf_len): take the oldest queued datagram.
///
/// Writes `[src_ip:4, src_port:2, payload...]` and returns the total bytes
/// written, or `ERR_AGAIN` when nothing is queued. Polling the card here (not
/// only from the shell) is what lets a Ring 3 program drive the network on its
/// own scheduling slice.
fn sys_udp_recv(sock: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if let Err(e) = socket_port(sock, "udp_recv") {
        return e;
    }
    if buf_len < 6 {
        return ERR_INVAL;
    }
    crate::net::poll();
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    let mut payload = [0u8; crate::net::socket::MAX_DATAGRAM];
    let cap = core::cmp::min((buf_len - 6) as usize, payload.len());
    let Some((len, src, src_port)) =
        crate::net::socket::take_owned(sock as usize, pid, &mut payload[..cap])
    else {
        return ERR_AGAIN;
    };
    let mut out = [0u8; 6 + crate::net::socket::MAX_DATAGRAM];
    out[0..4].copy_from_slice(&src.octets());
    out[4..6].copy_from_slice(&src_port.to_le_bytes());
    out[6..6 + len].copy_from_slice(&payload[..len]);
    match copy_to_user(buf_ptr, &out[..6 + len]) {
        Ok(n) => n,
        Err(e) => e,
    }
}

/// udp_close(sock): release a socket the caller owns.
fn sys_udp_close(sock: u64) -> u64 {
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    if crate::net::socket::close(sock as usize, pid) {
        0
    } else {
        ERR_BADF
    }
}

/// The port the connection's authority is scoped to (destination port for a
/// connect, local port for a listen), if `sock` belongs to the caller — after
/// the class-level network check, and followed by a check scoped to that
/// port, so a handle narrowed or revoked after the open stops working at the
/// next call (the same order as the UDP calls, for the same audit reasons).
fn tcp_conn_port(sock: u64, action: &'static str) -> Result<u16, u64> {
    use kernel_core::capability::{rights, CapabilityKind};
    require_any(CapabilityKind::Network, rights::USE, action)?;
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    let port = crate::net::tcp::scope_port(sock as usize, pid).ok_or(ERR_BADF)?;
    require_scoped(CapabilityKind::Network, rights::USE, port as u64, action)?;
    Ok(port)
}

/// tcp_connect(req_ptr, req_len): open a connection to `[ip:4, port:2]`.
///
/// Non-blocking: returns a descriptor at once with the SYN on its way; the
/// caller polls `tcp_state` until it is established. Waiting here would spin
/// with interrupts masked for a whole round trip.
fn sys_tcp_connect(req_ptr: u64, req_len: u64) -> u64 {
    if req_len != 6 {
        return ERR_INVAL;
    }
    let req = match copy_from_user(req_ptr, req_len, 6) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dst = kernel_core::net::ipv4::Ipv4Addr([req[0], req[1], req[2], req[3]]);
    let port = u16::from_le_bytes([req[4], req[5]]);
    if port == 0 {
        return ERR_INVAL;
    }
    if let Err(e) = require_scoped(
        kernel_core::capability::CapabilityKind::Network,
        kernel_core::capability::rights::USE,
        port as u64,
        "tcp_connect",
    ) {
        return e;
    }
    if !crate::net::is_up() {
        return ERR_NOENT;
    }
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    match crate::net::tcp::connect(pid, dst, port) {
        Some(index) => {
            let mut a = [0u8; 15];
            crate::audit::allowed(
                "tcp_connect",
                kernel_core::caps::CAP_NETWORK,
                Some(alloc::format!("dst={}:{port}", dst.format(&mut a))),
            );
            index as u64
        }
        None => ERR_AGAIN,
    }
}

/// tcp_listen(port): passive open. Non-blocking: returns a descriptor in
/// LISTEN at once; `tcp_state` reads `connecting` until a peer completes the
/// handshake, then `established`, after which the descriptor is the stream.
/// One connection per listen — the state machine's own passive-open model.
fn sys_tcp_listen(port: u64) -> u64 {
    if port == 0 || port > u16::MAX as u64 {
        return ERR_INVAL;
    }
    if let Err(e) = require_scoped(
        kernel_core::capability::CapabilityKind::Network,
        kernel_core::capability::rights::USE,
        port,
        "tcp_listen",
    ) {
        return e;
    }
    if !crate::net::is_up() {
        return ERR_NOENT;
    }
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    match crate::net::tcp::listen(pid, port as u16) {
        Some(index) => {
            crate::audit::allowed(
                "tcp_listen",
                kernel_core::caps::CAP_NETWORK,
                Some(alloc::format!("port={port}")),
            );
            index as u64
        }
        // The port is taken or every slot is in use.
        None => ERR_AGAIN,
    }
}

/// tcp_send(sock, ptr, len): queue bytes; returns how many were accepted
/// (`ERR_AGAIN` when the send buffer is full or the connection is not open).
///
/// A stream write may be partial, so a long buffer is not an error: at most
/// [`TCP_SEND_MAX`] bytes are copied per call and the count tells the caller
/// where to resume — the kernel never buffers more user memory than that.
fn sys_tcp_send(sock: u64, ptr: u64, len: u64) -> u64 {
    if let Err(e) = tcp_conn_port(sock, "tcp_send") {
        return e;
    }
    let data = match copy_from_user(ptr, len.min(TCP_SEND_MAX), TCP_SEND_MAX) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    crate::net::poll();
    match crate::net::tcp::send(sock as usize, pid, &data) {
        Some(0) => ERR_AGAIN,
        Some(n) => n as u64,
        None => ERR_BADF,
    }
}

/// tcp_recv(sock, ptr, len): read received bytes. Returns the count, `0` once
/// the peer has closed and everything was read, `ERR_AGAIN` when nothing yet.
fn sys_tcp_recv(sock: u64, ptr: u64, len: u64) -> u64 {
    if let Err(e) = tcp_conn_port(sock, "tcp_recv") {
        return e;
    }
    crate::net::poll();
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    let mut buf = [0u8; 1024];
    let cap = core::cmp::min(len as usize, buf.len());
    match crate::net::tcp::recv(sock as usize, pid, &mut buf[..cap]) {
        Some(crate::net::tcp::Read::Data(n)) => match copy_to_user(ptr, &buf[..n]) {
            Ok(n) => n,
            Err(e) => e,
        },
        Some(crate::net::tcp::Read::Eof) => 0,
        Some(crate::net::tcp::Read::Again) => ERR_AGAIN,
        // Reset or timed out: the stream is gone, not merely empty.
        Some(crate::net::tcp::Read::Failed) => ERR_NOENT,
        None => ERR_BADF,
    }
}

/// tcp_state(sock): 0 connecting, 1 established, 2 closing, 3 closed, 4 reset,
/// 5 timed out.
fn sys_tcp_state(sock: u64) -> u64 {
    if let Err(e) = tcp_conn_port(sock, "tcp_state") {
        return e;
    }
    crate::net::poll();
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    crate::net::tcp::state_code(sock as usize, pid).unwrap_or(ERR_BADF)
}

/// tcp_close(sock): orderly close (FIN after queued data). The descriptor
/// stays valid until the connection is fully closed, then is reclaimed.
fn sys_tcp_close(sock: u64) -> u64 {
    if let Err(e) = tcp_conn_port(sock, "tcp_close") {
        return e;
    }
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    if crate::net::tcp::close(sock as usize, pid) {
        0
    } else {
        ERR_BADF
    }
}

/// net_resolve(name_ptr, name_len, out_ptr): DNS A lookup.
///
/// Writes four address bytes on success. A failed lookup is an error, never a
/// fallback address: a resolver that invents an answer sends the caller's
/// traffic somewhere it never asked for.
fn sys_net_resolve(name_ptr: u64, name_len: u64, out_ptr: u64) -> u64 {
    if name_len == 0 || name_len > 253 {
        return ERR_INVAL;
    }
    let bytes = match copy_from_user(name_ptr, name_len, 253) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let Ok(name) = core::str::from_utf8(&bytes) else {
        return ERR_INVAL;
    };
    // The one network syscall that waits. Interrupts are off here, so the
    // bound is deliberately short: a lookup either completes in a couple of
    // round trips or is reported as a failure, rather than holding the CPU.
    match crate::net::resolve_name(name, 1500) {
        Some(addr) => match copy_to_user(out_ptr, &addr.octets()) {
            Ok(_) => 4,
            Err(e) => e,
        },
        None => ERR_NOENT,
    }
}

fn sys_cap_list(ptr: u64, capacity: u64) -> u64 {
    let count = core::cmp::min(capacity as usize, crate::capability::HANDLE_SLOTS);
    let mut bytes = [0u8; crate::capability::HANDLE_SLOTS * 8];
    for (i, handle) in CURRENT_HANDLES.lock().iter().take(count).enumerate() {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&handle.raw().to_le_bytes());
    }
    match copy_to_user(ptr, &bytes[..count * 8]) {
        Ok(_) => count as u64,
        Err(e) => e,
    }
}

fn sys_cap_check(raw: u64, kind: u64, scope_end: u64) -> u64 {
    let Some(kind) = kernel_core::capability::CapabilityKind::from_index(kind as usize) else {
        return ERR_INVAL;
    };
    let result = crate::capability::check(
        kernel_core::capability::CapabilityHandle::from_raw(raw),
        CURRENT_PID.load(Ordering::SeqCst),
        kind,
        kernel_core::capability::ResourceScope {
            start: 0,
            end: scope_end,
        },
        kernel_core::capability::rights::USE,
        crate::interrupts::ticks(),
    );
    match result {
        Ok(()) => 0,
        Err(e) => {
            crate::audit::denied_capability(
                "cap_handle_check",
                kind,
                crate::capability::reason_name(e),
            );
            ERR_PERM
        }
    }
}

/// cap_revoke(kind) — permanently drop the caller's own handle for `kind`.
///
/// Voluntary least privilege: a process that has finished with an authority
/// gives it up, so a later compromise cannot use it. Because enforcement now
/// goes through the capability table on every call, this takes effect on the
/// caller's very next syscall — which is exactly what a static capability
/// bitmask could not express.
fn sys_cap_revoke(kind: u64) -> u64 {
    let Some(kind) = kernel_core::capability::CapabilityKind::from_index(kind as usize) else {
        return ERR_INVAL;
    };
    let index = kind.index();
    let handle = CURRENT_HANDLES.lock()[index];
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    match crate::capability::revoke(handle, pid, crate::interrupts::ticks()) {
        Ok(()) => {
            set_current_handle(index, kernel_core::capability::CapabilityHandle::INVALID);
            crate::audit::allowed(
                "cap_revoke",
                0,
                Some(alloc::format!("kind={}", kind.name())),
            );
            0
        }
        Err(e) => {
            crate::audit::denied_capability("cap_revoke", kind, crate::capability::reason_name(e));
            ERR_PERM
        }
    }
}

/// cap_restrict(kind, rights_mask, expires_in_ticks) — replace the caller's own
/// handle for `kind` with a strictly narrower one. `expires_in_ticks` of 0
/// leaves the lifetime unchanged. Requested rights are intersected with what is
/// held and an expiry may only move earlier, so this can never amplify.
fn sys_cap_restrict(kind: u64, rights_mask: u64, expires_in_ticks: u64) -> u64 {
    let Some(kind) = kernel_core::capability::CapabilityKind::from_index(kind as usize) else {
        return ERR_INVAL;
    };
    if rights_mask > kernel_core::capability::rights::ALL as u64 {
        return ERR_INVAL;
    }
    let index = kind.index();
    let handle = CURRENT_HANDLES.lock()[index];
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    let now = crate::interrupts::ticks();
    let expires_at = (expires_in_ticks > 0).then(|| now.saturating_add(expires_in_ticks));
    match crate::capability::restrict(
        handle,
        pid,
        kernel_core::capability::ResourceScope::ANY,
        rights_mask as u32,
        expires_at,
        now,
    ) {
        Ok(narrowed) => {
            set_current_handle(index, narrowed);
            crate::audit::allowed(
                "cap_restrict",
                0,
                Some(alloc::format!(
                    "kind={} rights={rights_mask:#x} expires_in={expires_in_ticks}",
                    kind.name()
                )),
            );
            0
        }
        Err(e) => {
            crate::audit::denied_capability(
                "cap_restrict",
                kind,
                crate::capability::reason_name(e),
            );
            ERR_PERM
        }
    }
}

/// fs_read(path_ptr, path_len, req_ptr): read a VFS file into a user buffer.
/// `req` = 16 bytes {buf_ptr: u64, buf_cap: u64}. Requires CAP_FS_READ AND —
/// when the process is sandboxed — a path inside one of its allowed prefixes
/// (checked on the NORMALIZED path, so `..` traversal cannot escape).
/// Returns bytes copied, or ERR_PERM / ERR_NOENT / ERR_2BIG.
/// Check a path against the caller's sandbox, auditing a refusal.
///
/// The sandbox is a property of the PROCESS, not of the capability: a program
/// may hold write authority over the filesystem class and still be confined to
/// one prefix. Both must pass.
fn sandbox_allows(path: &str, action: &'static str, cap: u64) -> bool {
    let Some(prefixes) = current_sandbox() else {
        return true;
    };
    let allowed = prefixes
        .iter()
        .any(|p| kernel_core::path::is_within(p, path));
    if !allowed {
        crate::audit::denied(action, cap);
    }
    allowed
}

/// Copy a path argument out of user memory.
fn user_path(ptr: u64, len: u64) -> Result<alloc::string::String, u64> {
    let bytes = copy_from_user(ptr, len, 256)?;
    match core::str::from_utf8(&bytes) {
        Ok(s) => Ok(alloc::string::String::from(s)),
        Err(_) => Err(ERR_INVAL),
    }
}

/// fs_write(path_ptr, path_len, req_ptr): create or overwrite a file in the
/// persistent store. `req` is `[buf_ptr:8, buf_len:8]`.
///
/// Returns the number of bytes written. The write is one crash-atomic
/// superblock commit (see `fs_disk::FileSystem::write`), so an interrupted
/// overwrite leaves the old contents intact rather than a truncated file.
fn sys_fs_write(path_ptr: u64, path_len: u64, req_ptr: u64) -> u64 {
    let path = match user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !sandbox_allows(&path, "fs_write_sandbox", kernel_core::caps::CAP_FS_WRITE) {
        return ERR_PERM;
    }
    let Some(name) = crate::store_name(&path) else {
        return ERR_INVAL;
    };
    let req = match copy_from_user(req_ptr, 16, 16) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let buf_ptr = u64::from_le_bytes(req[0..8].try_into().unwrap());
    let buf_len = u64::from_le_bytes(req[8..16].try_into().unwrap());
    let data = match copy_from_user(buf_ptr, buf_len, FS_WRITE_MAX) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let result = crate::with_persistent_store(|fs| fs.write(name, &data));
    match result {
        Some(Ok(())) => {
            crate::audit::allowed(
                "fs_write",
                kernel_core::caps::CAP_FS_WRITE,
                Some(alloc::format!("path={path} bytes={}", data.len())),
            );
            data.len() as u64
        }
        // Fragmented (V0.10): enough free blocks, but not contiguous — to the
        // caller it is the same "does not fit" as a full disk.
        Some(Err(crate::fs_disk::Error::Fs(
            kernel_core::itfs::FsError::NoSpace | kernel_core::itfs::FsError::Fragmented,
        ))) => ERR_2BIG,
        Some(Err(_)) => ERR_INVAL,
        None => ERR_NOENT,
    }
}

/// fs_delete(path_ptr, path_len): remove a file from the persistent store.
fn sys_fs_delete(path_ptr: u64, path_len: u64) -> u64 {
    let path = match user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !sandbox_allows(&path, "fs_delete_sandbox", kernel_core::caps::CAP_FS_WRITE) {
        return ERR_PERM;
    }
    let Some(name) = crate::store_name(&path) else {
        return ERR_INVAL;
    };
    let result = crate::with_persistent_store(|fs| fs.remove(name));
    match result {
        Some(Ok(())) => {
            crate::audit::allowed(
                "fs_delete",
                kernel_core::caps::CAP_FS_WRITE,
                Some(alloc::format!("path={path}")),
            );
            0
        }
        Some(Err(crate::fs_disk::Error::Fs(kernel_core::itfs::FsError::NotFound))) => ERR_NOENT,
        Some(Err(_)) => ERR_INVAL,
        None => ERR_NOENT,
    }
}

/// fs_list(buf_ptr, buf_len): newline-separated names in the persistent store.
fn sys_fs_list(buf_ptr: u64, buf_len: u64) -> u64 {
    if !sandbox_allows(
        crate::STORE_PREFIX,
        "fs_list_sandbox",
        kernel_core::caps::CAP_FS_READ,
    ) {
        return ERR_PERM;
    }
    let Some(listing) = crate::with_persistent_store(|fs| {
        let mut out = alloc::string::String::new();
        for name in fs.list() {
            out.push_str(name);
            out.push('\n');
        }
        out
    }) else {
        return ERR_NOENT;
    };
    if listing.len() as u64 > buf_len {
        return ERR_2BIG;
    }
    match copy_to_user(buf_ptr, listing.as_bytes()) {
        Ok(n) => n,
        Err(e) => e,
    }
}

fn sys_fs_read(path_ptr: u64, path_len: u64, req_ptr: u64) -> u64 {
    let path_bytes = match copy_from_user(path_ptr, path_len, 256) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return ERR_INVAL;
    };
    // Sandbox check on the normalized path (component-wise, traversal-proof).
    if let Some(prefixes) = current_sandbox() {
        let allowed = prefixes
            .iter()
            .any(|p| kernel_core::path::is_within(p, path));
        if !allowed {
            crate::audit::denied("fs_read_sandbox", kernel_core::caps::CAP_FS_READ);
            return ERR_PERM;
        }
    }
    let req = match copy_from_user(req_ptr, 16, 16) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let buf_ptr = u64::from_le_bytes(req[0..8].try_into().unwrap());
    let buf_cap = u64::from_le_bytes(req[8..16].try_into().unwrap());
    // A path under the store prefix reads from the persistent ITFS volume;
    // anything else is the read-only initramfs. One syscall, two backing
    // stores, so a program does not need to know which it is talking to.
    let data = match crate::store_name(path) {
        Some(name) => match crate::with_persistent_store(|fs| fs.read(name)) {
            Some(Ok(d)) => d,
            _ => return ERR_NOENT,
        },
        None => match crate::fs::read(path) {
            Ok(d) => d.to_vec(),
            Err(_) => return ERR_NOENT,
        },
    };
    if (data.len() as u64) > buf_cap {
        return ERR_2BIG;
    }
    match copy_to_user(buf_ptr, &data) {
        Ok(n) => n,
        Err(e) => e,
    }
}

/// Validate that [ptr, ptr+len) lies entirely in the user window and is
/// mapped in the ACTIVE address space. Returns Ok(()) or an ERR_* code.
pub fn validate_user_range(ptr: u64, len: u64) -> Result<(), u64> {
    if len == 0 {
        return Ok(());
    }
    let end = ptr.checked_add(len).ok_or(ERR_FAULT)?;
    if ptr < USER_MIN || end > USER_MAX {
        return Err(ERR_FAULT);
    }
    let mut page_addr = ptr & !0xFFF;
    while page_addr < end {
        if crate::memory::paging::translate_active(VirtAddr::new(page_addr)).is_none() {
            return Err(ERR_FAULT);
        }
        page_addr += 4096;
    }
    Ok(())
}

/// Copy a validated user buffer into a kernel Vec (bounded by `max`).
pub fn copy_from_user(ptr: u64, len: u64, max: u64) -> Result<alloc::vec::Vec<u8>, u64> {
    if len > max {
        return Err(ERR_2BIG);
    }
    // Zero bytes: nothing to read, and the pointer must not be touched — a
    // zero-length request is valid with ANY pointer, NULL included, and
    // forming even an empty slice from NULL is undefined behaviour (the debug
    // kernel panics on it). Found by the V0.10 program-arguments work; the
    // ungated `cap_list(NULL, 0)` reached it on V0.9 (SEC09-001).
    if len == 0 {
        return Ok(alloc::vec::Vec::new());
    }
    validate_user_range(ptr, len)?;
    // SMAP forbids kernel access to user pages by default; this is one of
    // the three places that legitimately needs it, so it says so explicitly.
    let _access = crate::harden::UserAccess::begin();
    // SAFETY: range validated in the active space; the owning process is
    // suspended in this syscall so the mapping cannot change.
    let src = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    Ok(src.to_vec())
}

/// Copy kernel bytes into a validated user buffer; returns bytes written.
pub fn copy_to_user(ptr: u64, data: &[u8]) -> Result<u64, u64> {
    // As in `copy_from_user`: an empty copy never forms a slice from `ptr`.
    if data.is_empty() {
        return Ok(0);
    }
    validate_user_range(ptr, data.len() as u64)?;
    let _access = crate::harden::UserAccess::begin();
    // SAFETY: as above.
    let dst = unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, data.len()) };
    dst.copy_from_slice(data);
    Ok(data.len() as u64)
}

/// devinfo(index, buf_ptr, buf_len): copy a fixed 16-byte record for device
/// `index` into the validated user buffer. Returns 1 if a device exists there,
/// 0 past the end, or an ERR_*. This is the *only* device access Ring 3 gets —
/// enumeration/BAR/config port I/O stays in the kernel (least authority).
///
/// Record layout (16 bytes, little-endian):
/// `[bus, slot, func, class, subclass, has_driver, vendor:u16, device:u16,
///   num_caps, reserved×5]`.
fn sys_devinfo(index: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_len < 16 {
        return ERR_INVAL;
    }
    let record = crate::device::with_devices(|devs| {
        devs.get(index as usize).map(|d| {
            let id = d.id();
            let mut r = [0u8; 16];
            r[0] = d.pci.bus;
            r[1] = d.pci.slot;
            r[2] = d.pci.func;
            r[3] = id.class;
            r[4] = id.subclass;
            r[5] = d.driver.is_some() as u8;
            r[6..8].copy_from_slice(&id.vendor.to_le_bytes());
            r[8..10].copy_from_slice(&id.device.to_le_bytes());
            r[10] = d.caps.len().min(255) as u8;
            r
        })
    });
    match record {
        Some(r) => match copy_to_user(buf_ptr, &r) {
            Ok(_) => 1,
            Err(e) => e,
        },
        None => 0,
    }
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn win_err(e: crate::gfx::compositor::WinError) -> u64 {
    use crate::gfx::compositor::WinError::*;
    match e {
        NotFound => ERR_NOENT,
        NotOwner => ERR_PERM,
        OutOfBounds | BadSize => ERR_INVAL,
        TooMany => ERR_AGAIN,
    }
}

/// gui_create(req_ptr, len): req = {w:u32, h:u32, x:u32, y:u32, title...}.
/// Returns the window id or ERR_*.
fn sys_gui_create(ptr: u64, len: u64) -> u64 {
    if !crate::gfx::available() {
        return ERR_NOSYS;
    }
    let req = match copy_from_user(ptr, len, GUI_REQ_MAX) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if req.len() < 16 {
        return ERR_INVAL;
    }
    let w = le32(&req, 0) as usize;
    let h = le32(&req, 4) as usize;
    let x = le32(&req, 8) as i32 as isize;
    let y = le32(&req, 12) as i32 as isize;
    let title = core::str::from_utf8(&req[16..]).unwrap_or("");
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    match crate::gfx::compositor::create_window(pid, x, y, w, h, title) {
        Ok(id) => id as u64,
        Err(e) => win_err(e),
    }
}

/// gui_fill(win, req_ptr, len): req = {x,y,w,h,color: u32 each} (20 bytes).
fn sys_gui_fill(win: u64, ptr: u64, len: u64) -> u64 {
    let req = match copy_from_user(ptr, len, GUI_REQ_MAX) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if req.len() < 20 {
        return ERR_INVAL;
    }
    let (x, y, w, h, color) = (
        le32(&req, 0) as usize,
        le32(&req, 4) as usize,
        le32(&req, 8) as usize,
        le32(&req, 12) as usize,
        le32(&req, 16),
    );
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    match crate::gfx::compositor::window_fill(pid, win as u32, x, y, w, h, color) {
        Ok(()) => 0,
        Err(e) => win_err(e),
    }
}

/// gui_text(win, req_ptr, len): req = {x:u32, y:u32, color:u32, text...}.
fn sys_gui_text(win: u64, ptr: u64, len: u64) -> u64 {
    let req = match copy_from_user(ptr, len, GUI_REQ_MAX) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if req.len() < 12 {
        return ERR_INVAL;
    }
    let (x, y, color) = (
        le32(&req, 0) as usize,
        le32(&req, 4) as usize,
        le32(&req, 8),
    );
    let text = core::str::from_utf8(&req[12..]).unwrap_or("");
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    match crate::gfx::compositor::window_text(pid, win as u32, x, y, text, color) {
        Ok(()) => 0,
        Err(e) => win_err(e),
    }
}

/// gui_present(win): the caller must own `win`; re-composites the scene.
fn sys_gui_present(win: u64) -> u64 {
    let pid = CURRENT_PID.load(Ordering::SeqCst);
    // Ownership check without mutation.
    if crate::gfx::compositor::window_pixel(win as u32, 0, 0).is_none() {
        return ERR_NOENT;
    }
    let _ = pid;
    crate::gfx::compositor::composite();
    0
}

/// write(fd, ptr, len) — fd 1 (serial stdout) only.
fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if fd != 1 {
        return ERR_BADF;
    }
    if len == 0 {
        return 0;
    }
    if len > WRITE_MAX {
        return ERR_FAULT;
    }
    if let Err(e) = validate_user_range(ptr, len) {
        return e;
    }
    let _access = crate::harden::UserAccess::begin();
    // SAFETY: range validated in the active space; owning process suspended.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    for chunk in bytes.utf8_chunks() {
        crate::serial_print!("{}", chunk.valid());
        if !chunk.invalid().is_empty() {
            crate::serial_print!("\u{FFFD}");
        }
    }
    len
}
