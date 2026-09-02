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

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        // Dedicated static stack for the double-fault handler.
        static mut STACK: [u8; IST_STACK_SIZE] = [0; IST_STACK_SIZE];
        // SAFETY: address-of a static; exclusively reserved for this IST.
        let start = VirtAddr::from_ptr(&raw const STACK);
        start + IST_STACK_SIZE as u64
    };
    // RSP0: stack the CPU switches to when an interrupt/exception arrives
    // while running in Ring 3. Single CPU, non-nested handlers.
    tss.privilege_stack_table[0] = {
        static mut STACK: [u8; PRIV_STACK_SIZE] = [0; PRIV_STACK_SIZE];
        // SAFETY: as above.
        let start = VirtAddr::from_ptr(&raw const STACK);
        start + PRIV_STACK_SIZE as u64
    };
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
