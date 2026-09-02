//! IDT, exception handlers, PIC remap and timer interrupt (B080/B090).
//!
//! Exception policy (plan §10.2): report vector + error code + fault address
//! on the serial panic path; never continue with known corruption. The
//! breakpoint handler is the only resumable exception (used by selftests).

use core::sync::atomic::{AtomicU64, Ordering};
use pic8259::ChainedPics;
use spin::{Lazy, Mutex};
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub const TIMER_VECTOR: u8 = PIC_1_OFFSET; // IRQ0
pub const KEYBOARD_VECTOR: u8 = PIC_1_OFFSET + 1; // IRQ1 (future input work)

/// Timer frequency: PIT programmed to ~100 Hz (divisor 11932 of 1.193182 MHz).
pub const TICK_HZ: u64 = 100;
const PIT_DIVISOR: u16 = 11932;

static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Monotonic tick counter incremented by the timer interrupt.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Breakpoint hit counter (selftest evidence that the IDT works).
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    idt.general_protection_fault
        .set_handler_fn(general_protection_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    // SAFETY: DOUBLE_FAULT_IST_INDEX refers to a valid IST entry installed
    // by gdt::init before the IDT is loaded.
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(crate::gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt[TIMER_VECTOR].set_handler_fn(timer_handler);
    idt
});

/// Install GDT + IDT (B080). Interrupts stay disabled until [`enable_timer`].
pub fn init_descriptors() {
    crate::gdt::init();
    IDT.load();
}

/// Remap the PICs, program the PIT, unmask only the timer line, enable
/// interrupts (B090).
pub fn enable_timer() {
    // SAFETY: standard PIC remap + PIT mode-3 programming on the QEMU pc
    // machine; ports 0x20/0xA0/0x21/0xA1 (PIC) and 0x43/0x40 (PIT) are the
    // architectural locations.
    unsafe {
        PICS.lock().initialize();
        // Mask everything except IRQ0 (timer) on the primary PIC; mask all
        // of the secondary.
        PICS.lock().write_masks(!0b0000_0001, 0xFF);

        let mut cmd: Port<u8> = Port::new(0x43);
        let mut data: Port<u8> = Port::new(0x40);
        cmd.write(0b0011_0110u8); // channel 0, lobyte/hibyte, mode 3
        data.write((PIT_DIVISOR & 0xFF) as u8);
        data.write((PIT_DIVISOR >> 8) as u8);
    }
    x86_64::instructions::interrupts::enable();
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_seconds() -> u64 {
    ticks() / TICK_HZ
}

pub fn breakpoint_count() -> u64 {
    BREAKPOINTS.load(Ordering::Relaxed)
}

extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: acknowledging the interrupt we are currently servicing.
    unsafe {
        PICS.lock().notify_end_of_interrupt(TIMER_VECTOR);
    }
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[ITISYOU:INFO] exception=breakpoint rip={:#x} (resumed)",
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    panic!(
        "invalid opcode at rip={:#x}",
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn general_protection_handler(frame: InterruptStackFrame, error_code: u64) {
    panic!(
        "general protection fault error_code={:#x} rip={:#x}",
        error_code,
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn page_fault_handler(
    frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let addr = x86_64::registers::control::Cr2::read();
    panic!(
        "page fault addr={:?} error={:?} rip={:#x}",
        addr,
        error_code,
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error_code: u64) -> ! {
    panic!(
        "double fault error_code={:#x} rip={:#x}",
        error_code,
        frame.instruction_pointer.as_u64()
    );
}
