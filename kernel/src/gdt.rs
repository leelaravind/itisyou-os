//! GDT/TSS setup (part of B080): kernel code/data segments plus a dedicated
//! interrupt stack for double faults so a corrupted kernel stack cannot
//! escalate to a triple fault silently.

use spin::Lazy;
use x86_64::instructions::segmentation::{Segment, CS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::registers::segmentation::SegmentSelector;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const IST_STACK_SIZE: usize = 4096 * 5;

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        // Dedicated static stack for the double-fault handler. No guard page
        // yet (documented limitation) — the stack is used only by that
        // handler, which never returns.
        static mut STACK: [u8; IST_STACK_SIZE] = [0; IST_STACK_SIZE];
        // SAFETY: address-of a static; the stack is exclusively reserved for
        // the double-fault IST entry.
        let start = VirtAddr::from_ptr(&raw const STACK);
        start + IST_STACK_SIZE as u64
    };
    tss
});

struct Selectors {
    code: SegmentSelector,
    data: SegmentSelector,
    tss: SegmentSelector,
}

static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let code = gdt.append(Descriptor::kernel_code_segment());
    let data = gdt.append(Descriptor::kernel_data_segment());
    let tss = gdt.append(Descriptor::tss_segment(&TSS));
    (gdt, Selectors { code, data, tss })
});

/// Load the GDT, reload segment registers, load the TSS.
pub fn init() {
    GDT.0.load();
    // SAFETY: the selectors reference entries in the GDT loaded above.
    unsafe {
        CS::set_reg(GDT.1.code);
        SS::set_reg(GDT.1.data);
        load_tss(GDT.1.tss);
    }
}
