//! GDT/TSS setup (B080) — V0.2 adds Ring 3 segments and the privilege-0
//! stack used when an interrupt/exception arrives from userspace.
//!
//! Segment order is dictated by the `syscall`/`sysret` STAR MSR contract:
//! kernel_data = kernel_code + 8, and user_code = user_data + 8.

use spin::Lazy;
use x86_64::instructions::segmentation::{Segment, CS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::registers::segmentation::SegmentSelector;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::PrivilegeLevel;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const IST_STACK_SIZE: usize = 4096 * 5;
const PRIV_STACK_SIZE: usize = 4096 * 8;

/// Fixed selector values implied by the append order below. `init` asserts
/// they match reality; the syscall entry/user-transition asm relies on them.
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub const USER_DATA_SELECTOR: u16 = 0x18 | 3;
pub const USER_CODE_SELECTOR: u16 = 0x20 | 3;

/// A static kernel stack with one page BELOW it that is unmapped at boot
/// ([`arm_stack_guards`]). Stacks grow down, so an overflow runs into the
/// guard and faults instead of silently overwriting whatever the linker
/// placed next — which is exactly what V0.9's first TCP integration did to
/// the capability table. Page-aligned so the guard is a whole page and the
/// stack top is 16-byte aligned.
#[repr(C, align(4096))]
struct GuardedStack<const N: usize> {
    guard: [u8; 4096],
    stack: [u8; N],
}

static mut IST_STACK: GuardedStack<IST_STACK_SIZE> = GuardedStack {
    guard: [0; 4096],
    stack: [0; IST_STACK_SIZE],
};
static mut PRIV_STACK: GuardedStack<PRIV_STACK_SIZE> = GuardedStack {
    guard: [0; 4096],
    stack: [0; PRIV_STACK_SIZE],
};

const GUARD_LEN: u64 = 4096;

/// Address of a guarded stack: its guard page comes first (`repr(C)`), the
/// stack proper `GUARD_LEN` bytes later. Taking the address of the whole
/// static needs no `unsafe`; projecting a field through a `static mut` would.
fn stack_base<const N: usize>(stack: *const GuardedStack<N>) -> u64 {
    stack as u64
}

/// Top of the RSP0 stack (the first byte past it).
pub fn priv_stack_top() -> VirtAddr {
    // Address-of only; nothing reads or writes through this pointer here.
    VirtAddr::new(stack_base(&raw const PRIV_STACK) + GUARD_LEN + PRIV_STACK_SIZE as u64)
}

/// The guard page of each static kernel stack: `(name, first byte)`.
pub fn stack_guards() -> [(&'static str, u64); 2] {
    [
        ("priv", stack_base(&raw const PRIV_STACK)),
        ("ist_double_fault", stack_base(&raw const IST_STACK)),
    ]
}

/// The stack whose guard page contains `addr`, if any.
pub fn guard_hit(addr: u64) -> Option<&'static str> {
    stack_guards()
        .into_iter()
        .find(|&(_, g)| (g..g + GUARD_LEN).contains(&addr))
        .map(|(name, _)| name)
}

/// Unmap each stack's guard page. Called once, after paging and the
/// descriptors are up. The frame that backed a guard page is deliberately
/// never reused (two pages for the life of the boot), so nothing can map it
/// back by accident.
pub fn arm_stack_guards() {
    use x86_64::structures::paging::{Page, Size4KiB};
    for (name, guard) in stack_guards() {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(guard));
        match crate::memory::paging::unmap_page(page) {
            Ok(_) => crate::serial_println!(
                "[ITISYOU:HARDEN] stack_guard stack={name} guard={guard:#x} armed=true unmapped={}",
                crate::memory::paging::is_unmapped(VirtAddr::new(guard))
            ),
            Err(e) => crate::serial_println!(
                "[ITISYOU:HARDEN] stack_guard stack={name} guard={guard:#x} armed=false reason={e:?}"
            ),
        }
    }
}

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();
    // Dedicated static stack for the double-fault handler.
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
        VirtAddr::new(stack_base(&raw const IST_STACK) + GUARD_LEN + IST_STACK_SIZE as u64);
    // RSP0: stack the CPU switches to when an interrupt/exception arrives
    // while running in Ring 3, and the stack every syscall runs on. Single
    // CPU, non-nested handlers.
    tss.privilege_stack_table[0] = priv_stack_top();
    tss
});

pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
    tss: SegmentSelector,
}

static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let kernel_code = gdt.append(Descriptor::kernel_code_segment());
    let kernel_data = gdt.append(Descriptor::kernel_data_segment());
    // Order matters (sysret): user_data first, then user_code.
    let user_data = gdt.append(Descriptor::user_data_segment());
    let user_code = gdt.append(Descriptor::user_code_segment());
    let tss = gdt.append(Descriptor::tss_segment(&TSS));
    (
        gdt,
        Selectors {
            kernel_code,
            kernel_data,
            user_data,
            user_code,
            tss,
        },
    )
});

/// Load the GDT, reload segment registers, load the TSS.
pub fn init() {
    GDT.0.load();
    // The transition/syscall asm hardcodes the selector constants above —
    // fail loudly if the table layout ever drifts.
    assert_eq!(GDT.1.kernel_code.0, KERNEL_CODE_SELECTOR);
    assert_eq!(GDT.1.kernel_data.0, KERNEL_DATA_SELECTOR);
    assert_eq!(
        SegmentSelector::new(GDT.1.user_data.index(), PrivilegeLevel::Ring3).0,
        USER_DATA_SELECTOR
    );
    assert_eq!(
        SegmentSelector::new(GDT.1.user_code.index(), PrivilegeLevel::Ring3).0,
        USER_CODE_SELECTOR
    );
    // SAFETY: the selectors reference entries in the GDT loaded above.
    unsafe {
        CS::set_reg(GDT.1.kernel_code);
        SS::set_reg(GDT.1.kernel_data);
        load_tss(GDT.1.tss);
    }
}

pub fn selectors() -> &'static Selectors {
    &GDT.1
}
