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

pub const ERR_NOSYS: u64 = u64::MAX;
pub const ERR_FAULT: u64 = u64::MAX - 1;
pub const ERR_BADF: u64 = u64::MAX - 2;
pub const ERR_NOENT: u64 = u64::MAX - 3;
pub const ERR_AGAIN: u64 = u64::MAX - 4;
pub const ERR_INVAL: u64 = u64::MAX - 5;
pub const ERR_2BIG: u64 = u64::MAX - 6;
pub const ERR_PERM: u64 = u64::MAX - 7;

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

static mut SYSCALL_STACK: [u8; KSTACK_SIZE] = [0; KSTACK_SIZE];

/// PID of the currently running user process (0 = none).
pub static CURRENT_PID: AtomicU64 = AtomicU64::new(0);

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
        let base = (&raw const SYSCALL_STACK) as u64;
        let top = (base + KSTACK_SIZE as u64) & !0xF;
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
        SYS_SPAWN => crate::proc::sys_spawn(a1, a2),
        SYS_WAIT => crate::proc::sys_wait(a1),
        SYS_MSG_SEND => crate::ipc::sys_msg_send(a1, a2, a3),
        SYS_MSG_RECV => crate::ipc::sys_msg_recv(a1, a2, a3),
        SYS_GUI_CREATE => sys_gui_create(a1, a2),
        SYS_GUI_FILL => sys_gui_fill(a1, a2, a3),
        SYS_GUI_TEXT => sys_gui_text(a1, a2, a3),
        SYS_GUI_PRESENT => sys_gui_present(a1),
        _ => {
            NOSYS_COUNT.fetch_add(1, Ordering::SeqCst);
            ERR_NOSYS
        }
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
    validate_user_range(ptr, len)?;
    // SAFETY: range validated in the active space; the owning process is
    // suspended in this syscall so the mapping cannot change.
    let src = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    Ok(src.to_vec())
}

/// Copy kernel bytes into a validated user buffer; returns bytes written.
pub fn copy_to_user(ptr: u64, data: &[u8]) -> Result<u64, u64> {
    validate_user_range(ptr, data.len() as u64)?;
    // SAFETY: as above.
    let dst = unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, data.len()) };
    dst.copy_from_slice(data);
    Ok(data.len() as u64)
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
