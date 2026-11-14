//! CPU-enforced separation between kernel and user memory (V0.8).
//!
//! The page tables already say which pages are user pages, and the kernel
//! already validates every pointer userspace hands it. SMEP and SMAP add the
//! thing neither of those can: enforcement that does not depend on the kernel
//! being correct.
//!
//! * **SMEP** — the CPU refuses to *execute* a user page in kernel mode. A
//!   corrupted function pointer that lands in a page userspace controls stops
//!   being an exploit primitive and becomes a fault.
//! * **SMAP** — the CPU refuses to *read or write* a user page in kernel mode
//!   unless `EFLAGS.AC` is set. That inverts the default: instead of the
//!   kernel being allowed to touch user memory everywhere and being careful
//!   not to, it is forbidden everywhere and must say explicitly where it
//!   means to. The three places that legitimately do — the two user-copy
//!   helpers and `write` — bracket their access with `stac`/`clac`; anywhere
//!   else, a stray dereference of a user pointer now faults instead of
//!   quietly working.
//! * **UMIP** — userspace cannot read the descriptor-table registers
//!   (`sgdt`/`sidt`/`sldt`/`str`/`smsw`), which otherwise leak kernel
//!   addresses to a Ring 3 program for free.
//!
//! Each is enabled only if the CPU advertises it, and what was actually
//! enabled is reported rather than assumed — a hardening feature you believe
//! is on and is not is worse than one you know is off.
//!
//! * **No FPU or SIMD** (V1.0, SEC1-002) — the kernel never saves x87/SSE
//!   state across a process switch, and every program is built soft-float,
//!   so that state was a storage channel between processes no capability
//!   gates (one program's registers were the next one's to read). With
//!   `CR0.EM` set and `CR4.OSFXSR` clear, an x87 instruction is `#NM` and an
//!   SSE one `#UD`, both contained: the program ends, nothing leaks.

use crate::serial_println;
use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};

/// True once SMAP is enabled, so the user-copy helpers know whether the
/// `stac`/`clac` bracket is needed. Executing `stac` on a CPU without SMAP is
/// an invalid opcode, so this is not merely an optimisation.
static SMAP_ENABLED: AtomicBool = AtomicBool::new(false);

/// RFLAGS.DF and RFLAGS.AC: a Ring 3 program may set both, and neither is
/// cleared by interrupt delivery. Kernel code must never run with them
/// (V0.10, HARD10-002): DF reverses string instructions, AC disables SMAP.
pub const USER_FLAGS: u64 = (1 << 10) | (1 << 18);

/// Kernel entries from Ring 3 that were checked for DF/AC, and how many found
/// either still set — the evidence the `harden` command reports.
pub static FLAG_CHECKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static FLAG_DIRTY_TIMER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static FLAG_DIRTY_LANDING: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// Timer entries from Ring 3 whose INTERRUPTED context had DF or AC set (read
/// from the saved frame): proof the probe really set them, so `dirty_timer=0`
/// is a measured result rather than a tautology.
pub static FLAG_USER_SET_SEEN: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Clear DF and AC on kernel entry (V0.10, HARD10-002). `pushfq`/`popfq`
/// rather than `clac`, which would #UD on a CPU without SMAP; interrupt
/// delivery has already cleared IF, so rewriting RFLAGS here changes nothing
/// else. The interrupted context's own flags live in its stack frame and are
/// restored untouched by `iretq`.
#[inline(always)]
pub fn clear_user_flags() {
    // SAFETY: only DF and AC change; the push/pop pair is balanced.
    unsafe {
        core::arch::asm!(
            "cld",
            "pushfq",
            "and qword ptr [rsp], -262145",
            "popfq",
            options(nomem, preserves_flags)
        );
    }
}

/// Current RFLAGS & (DF | AC).
#[inline(always)]
pub fn user_flags_now() -> u64 {
    x86_64::registers::rflags::read_raw() & USER_FLAGS
}
static SMEP_ENABLED: AtomicBool = AtomicBool::new(false);
static UMIP_ENABLED: AtomicBool = AtomicBool::new(false);

/// Which protections are active.
pub fn state() -> (bool, bool, bool) {
    (
        SMEP_ENABLED.load(Ordering::Relaxed),
        SMAP_ENABLED.load(Ordering::Relaxed),
        UMIP_ENABLED.load(Ordering::Relaxed),
    )
}

/// Enable every supported supervisor-mode protection.
///
/// Must run before any Ring 3 process exists: turning SMAP on while a syscall
/// is mid-copy would fault the kernel on its own legitimate access.
pub fn init() {
    // CPUID leaf 7 subleaf 0: EBX bit 7 = SMEP, bit 20 = SMAP; ECX bit 2 =
    // UMIP.
    let leaf7 = core::arch::x86_64::__cpuid_count(7, 0);
    let smep = leaf7.ebx & (1 << 7) != 0;
    let smap = leaf7.ebx & (1 << 20) != 0;
    let umip = leaf7.ecx & (1 << 2) != 0;

    // SAFETY: each bit is only set after CPUID advertised the feature, and
    // this runs during single-threaded boot before any user process exists.
    // The FPU bits: the kernel is soft-float (x86_64-unknown-none), so
    // nothing in it executes an x87 or SSE instruction that EM or a clear
    // OSFXSR would fault.
    unsafe {
        Cr4::update(|flags| {
            if smep {
                flags.insert(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
            }
            if smap {
                flags.insert(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION);
            }
            if umip {
                flags.insert(Cr4Flags::USER_MODE_INSTRUCTION_PREVENTION);
            }
            flags.remove(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
        });
        Cr0::update(|flags| {
            flags.insert(Cr0Flags::EMULATE_COPROCESSOR);
            flags.remove(Cr0Flags::MONITOR_COPROCESSOR | Cr0Flags::TASK_SWITCHED);
        });
    }
    SMEP_ENABLED.store(smep, Ordering::SeqCst);
    SMAP_ENABLED.store(smap, Ordering::SeqCst);
    UMIP_ENABLED.store(umip, Ordering::SeqCst);
    let fpu_off = Cr0::read().contains(Cr0Flags::EMULATE_COPROCESSOR)
        && !Cr4::read().contains(Cr4Flags::OSFXSR);
    serial_println!(
        "[ITISYOU:HARDEN] cpu_protection smep={smep} smap={smap} umip={umip} user_fpu={} cr4={:#x}",
        if fpu_off { "off" } else { "ON" },
        Cr4::read_raw(),
    );
}

/// Permit kernel access to user pages for the duration of a guard.
///
/// A guard rather than a bare pair of calls: an early return between `stac`
/// and `clac` would leave the kernel running with user access permitted, which
/// is precisely the state SMAP exists to prevent. `Drop` closes the window on
/// every return path. A panic does not unwind (the kernel is built
/// `panic = "abort"`), so `Drop` does not run then — but the panic handler
/// halts or exits QEMU, so no code ever runs with the window left open.
pub struct UserAccess {
    active: bool,
}

impl UserAccess {
    /// Open the window. Cheap and a no-op when SMAP is unavailable.
    pub fn begin() -> UserAccess {
        let active = SMAP_ENABLED.load(Ordering::Relaxed);
        if active {
            // SAFETY: `stac` is valid because CPUID advertised SMAP and CR4
            // has it enabled; it only sets EFLAGS.AC.
            unsafe { core::arch::asm!("stac", options(nomem, nostack)) };
        }
        UserAccess { active }
    }
}

impl Drop for UserAccess {
    fn drop(&mut self) {
        if self.active {
            // SAFETY: as above; `clac` clears EFLAGS.AC.
            unsafe { core::arch::asm!("clac", options(nomem, nostack)) };
        }
    }
}
