//! IDT, exception handlers, PIC remap and timer interrupt (B080/B090).
//!
//! Exception policy (plan §10.2): report vector + error code + fault address
//! on the serial panic path; never continue with known corruption. The
//! breakpoint handler is the only resumable exception (used by selftests).

use core::arch::naked_asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use pic8259::ChainedPics;
use spin::{Lazy, Mutex};
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::PrivilegeLevel;

/// True when the faulting context was Ring 3 — such faults are contained
/// (the process is terminated) instead of panicking the kernel (V0.2).
fn from_user(frame: &InterruptStackFrame) -> bool {
    frame.code_segment.rpl() == PrivilegeLevel::Ring3
}

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub const TIMER_VECTOR: u8 = PIC_1_OFFSET; // IRQ0
pub const KEYBOARD_VECTOR: u8 = PIC_1_OFFSET + 1; // IRQ1
pub const MOUSE_VECTOR: u8 = PIC_2_OFFSET + 4; // IRQ12

// APIC-delivered vectors (V0.8). They sit well above the PIC's 32..47 window
// so the two controllers can coexist: nothing here displaces a legacy line,
// and a vector collision would be a silent misdelivery rather than an error.
/// Local APIC timer, used to prove APIC delivery.
pub const VECTOR_APIC_TIMER: u8 = 0x41;
/// Message-signalled interrupts from the NIC.
pub const VECTOR_MSI: u8 = 0x42;
/// I/O APIC redirection target used for the V0.8 programming round-trip.
/// Unused since the V0.9 cutover routes the real lines; kept reserved so the
/// number is never reused for something else.
pub const VECTOR_IOAPIC_PROBE: u8 = 0x43;
/// Spurious-interrupt vector. The APIC raises these as normal behaviour; what
/// would be a fault is having no handler installed for one.
pub const VECTOR_SPURIOUS: u8 = 0xFF;

/// Timer frequency: PIT programmed to ~100 Hz (divisor 11932 of 1.193182 MHz).
pub const TICK_HZ: u64 = 100;
const PIT_DIVISOR: u16 = 11932;

static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Set once the I/O APIC delivers the line-based IRQs (timer, PS/2) and the
/// 8259s are retired (V0.9). Decides which controller an EOI goes to: sending
/// it to the wrong one does not fail loudly, the right one just never
/// delivers again.
static LEGACY_VIA_IOAPIC: AtomicBool = AtomicBool::new(false);

/// True when line-based IRQs come through the I/O APIC.
pub fn legacy_via_ioapic() -> bool {
    LEGACY_VIA_IOAPIC.load(Ordering::SeqCst)
}

/// Acknowledge a line-based interrupt on whichever controller delivered it.
/// Lock-free on the I/O APIC path (the local APIC's EOI register is written
/// directly), so an ISR can never spin on a lock the interrupted code holds.
fn eoi_line(vector: u8) {
    if LEGACY_VIA_IOAPIC.load(Ordering::Relaxed) {
        crate::apic::eoi();
    } else {
        // SAFETY: acknowledging the PIC interrupt currently being serviced.
        unsafe { PICS.lock().notify_end_of_interrupt(vector) };
    }
}

/// The 8259 interrupt mask registers (primary, secondary), read from the chips.
pub fn pic_masks() -> (u8, u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: reading the PIC mask registers has no side effects.
        let m = unsafe { PICS.lock().read_masks() };
        (m[0], m[1])
    })
}

/// Mask every line on both 8259s and mark the I/O APIC as the line-IRQ
/// controller. Called by `apic::cutover_legacy_irqs` with interrupts disabled,
/// after the replacement routes are programmed and before interrupts return,
/// so no edge is ever acknowledged on the wrong controller.
pub(crate) fn retire_pic() {
    // SAFETY: masking all PIC lines; nothing is delivered through them after
    // this, and the I/O APIC entries for the same sources are already armed.
    unsafe { PICS.lock().write_masks(0xFF, 0xFF) };
    LEGACY_VIA_IOAPIC.store(true, Ordering::SeqCst);
}

/// Monotonic tick counter incremented by the timer interrupt.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Breakpoint hit counter (selftest evidence that the IDT works).
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

/// Count of timer preemptions of Ring 3 processes (diagnostic evidence).
pub static PREEMPTIONS: AtomicU64 = AtomicU64::new(0);

/// Ticks remaining in the current user process's scheduling quantum. Armed
/// by the scheduler before entering Ring 3, decremented by the timer while
/// CPL=3; reaching zero triggers preemption. 0 = not armed (no preemption).
static QUANTUM_REMAINING: AtomicU32 = AtomicU32::new(0);

/// Quantum length in timer ticks (10 ms each at 100 Hz). Small so a
/// non-yielding process is preempted promptly and fairness is easy to prove.
pub const QUANTUM_TICKS: u32 = 2;

/// Arm the preemption quantum for the process about to run (scheduler call).
pub fn arm_quantum() {
    QUANTUM_REMAINING.store(QUANTUM_TICKS, Ordering::SeqCst);
}

/// Disarm preemption (scheduler call, after a process returns to the kernel).
pub fn disarm_quantum() {
    QUANTUM_REMAINING.store(0, Ordering::SeqCst);
}

pub fn preemption_count() -> u64 {
    PREEMPTIONS.load(Ordering::Relaxed)
}

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
    // SAFETY: timer_isr is a naked handler with the correct interrupt-frame
    // ABI; set_handler_addr wires it directly. It saves/restores all GPRs
    // and may preempt Ring 3 (see below).
    let isr: unsafe extern "C" fn() = timer_isr;
    unsafe {
        idt[TIMER_VECTOR].set_handler_addr(x86_64::VirtAddr::new(isr as usize as u64));
    }
    idt[KEYBOARD_VECTOR].set_handler_fn(keyboard_handler);
    idt[MOUSE_VECTOR].set_handler_fn(mouse_handler);
    idt[VECTOR_APIC_TIMER].set_handler_fn(apic_timer_handler);
    idt[VECTOR_MSI].set_handler_fn(msi_handler);
    idt[VECTOR_SPURIOUS].set_handler_fn(spurious_handler);
    idt
});

/// Local APIC timer. Counts and acknowledges; the test reads the counter.
extern "x86-interrupt" fn apic_timer_handler(_frame: InterruptStackFrame) {
    crate::apic::TIMER_COUNT.fetch_add(1, Ordering::SeqCst);
    crate::apic::eoi();
}

/// A message-signalled interrupt from a PCI device.
///
/// It deliberately does no device work: the network receive path stays polled
/// (ADR-0015), so this handler exists to prove the message was delivered, not
/// to become a second, interrupt-driven data path with its own locking rules.
/// The device's own interrupt cause register is cleared by the polling side.
extern "x86-interrupt" fn msi_handler(_frame: InterruptStackFrame) {
    crate::apic::MSI_COUNT.fetch_add(1, Ordering::SeqCst);
    crate::apic::eoi();
}

/// Spurious interrupt. No EOI: the architecture specifies that a spurious
/// vector does not set the in-service bit, so acknowledging it would retire
/// some *other* interrupt.
extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {
    crate::apic::SPURIOUS_COUNT.fetch_add(1, Ordering::SeqCst);
}

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
        // Channel 0, lobyte/hibyte, mode 2 (rate generator): one short low
        // pulse per period, i.e. exactly one rising edge. Mode 3 (square
        // wave) was used until V0.9 and was fine behind the PIC, but through
        // QEMU's edge-triggered I/O APIC it delivered TWICE per period —
        // measured at the cutover as 10 -> 20 ticks per 100 ms (ADR-0019).
        cmd.write(0b0011_0100u8);
        data.write((PIT_DIVISOR & 0xFF) as u8);
        data.write((PIT_DIVISOR >> 8) as u8);
    }
    x86_64::instructions::interrupts::enable();
}

/// Unmask IRQ1 (keyboard), IRQ2 (cascade) and IRQ12 (mouse) alongside the
/// timer (V0.5 input). Primary PIC keeps bits 0/1/2 enabled; secondary
/// enables bit 4 (IRQ12).
pub fn set_input_irqs_enabled() {
    if legacy_via_ioapic() {
        // After the cutover the lines live in the I/O APIC.
        crate::apic::set_isa_line_masked(1, false);
        crate::apic::set_isa_line_masked(12, false);
        return;
    }
    // Mask interrupts while holding the PICS lock: the timer/keyboard/mouse
    // ISRs also lock PICS (for EOI), so an interrupt here would deadlock.
    x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: reprogramming PIC masks; the corresponding IDT vectors are
        // installed (timer/keyboard/mouse).
        unsafe {
            PICS.lock().write_masks(!0b0000_0111, !0b0001_0000);
        }
    });
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

/// Saved register frame for the naked timer ISR. Field order matches the
/// push order in `timer_isr` exactly (r15 at offset 0 = lowest address =
/// where rsp points after the pushes; the CPU-pushed interrupt frame follows
/// the GPRs). `#[repr(C)]` is REQUIRED.
#[repr(C)]
struct TrapFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    // CPU-pushed interrupt frame:
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

/// Naked timer ISR: saves all GPRs, calls the Rust handler with a pointer to
/// the `TrapFrame`, and — if the handler returns (no preemption) — restores
/// and `iretq`s. When the handler preempts a Ring 3 process it long-jumps to
/// the scheduler and never returns here.
#[unsafe(naked)]
unsafe extern "C" fn timer_isr() {
    naked_asm!(
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp", // &TrapFrame
        "call {handler}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "iretq",
        handler = sym timer_handler_inner,
    )
}

/// Timer handler body. Runs with interrupts disabled (interrupt gate).
/// Increments ticks, EOIs the PIC, and — when a Ring 3 process's quantum has
/// expired — saves its full context and preempts it (never returns in that
/// case).
extern "C" fn timer_handler_inner(frame: &mut TrapFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);

    // Was the interrupt taken from Ring 3 (a user process)?
    let from_user = frame.cs & 0b11 == 0b11;

    // EOI before any possible long-jump, so the controller delivers the next
    // tick.
    eoi_line(TIMER_VECTOR);

    if !from_user {
        return; // kernel-context tick: just account it
    }

    // Decrement the quantum; preempt when it reaches zero.
    let remaining = QUANTUM_REMAINING.load(Ordering::SeqCst);
    if remaining == 0 {
        return; // not armed (shouldn't happen from user, but be safe)
    }
    if remaining > 1 {
        QUANTUM_REMAINING.store(remaining - 1, Ordering::SeqCst);
        return;
    }
    // Quantum expired: save the full user context and preempt.
    let ctx_ptr = crate::user::transition::current_ctx();
    if ctx_ptr.is_null() {
        return;
    }
    QUANTUM_REMAINING.store(0, Ordering::SeqCst);
    PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: ctx_ptr is the running process's context, valid until
    // enter_or_resume returns; single CPU serializes access.
    let ctx = unsafe { &mut *ctx_ptr };
    ctx.rax = frame.rax;
    ctx.rbx = frame.rbx;
    ctx.rcx = frame.rcx;
    ctx.rdx = frame.rdx;
    ctx.rsi = frame.rsi;
    ctx.rdi = frame.rdi;
    ctx.rbp = frame.rbp;
    ctx.r8 = frame.r8;
    ctx.r9 = frame.r9;
    ctx.r10 = frame.r10;
    ctx.r11 = frame.r11;
    ctx.r12 = frame.r12;
    ctx.r13 = frame.r13;
    ctx.r14 = frame.r14;
    ctx.r15 = frame.r15;
    ctx.rip = frame.rip;
    ctx.rsp = frame.rsp;
    ctx.rflags = frame.rflags;
    // Long-jump back to the scheduler run-loop; does not return.
    crate::user::transition::abort_preempt();
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    crate::input::on_keyboard_irq();
    eoi_line(KEYBOARD_VECTOR);
}

extern "x86-interrupt" fn mouse_handler(_frame: InterruptStackFrame) {
    crate::input::on_mouse_irq();
    // On the PIC path this EOIs both chips (IRQ12 is on the secondary).
    eoi_line(MOUSE_VECTOR);
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[ITISYOU:INFO] exception=breakpoint rip={:#x} (resumed)",
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    if from_user(&frame) {
        crate::serial_println!(
            "[ITISYOU:INFO] exception=invalid_opcode cs_rpl=3 rip={:#x} action=terminate_process",
            frame.instruction_pointer.as_u64()
        );
        crate::user::transition::abort_fault(6);
    }
    panic!(
        "invalid opcode at rip={:#x}",
        frame.instruction_pointer.as_u64()
    );
}

extern "x86-interrupt" fn general_protection_handler(frame: InterruptStackFrame, error_code: u64) {
    if from_user(&frame) {
        // A privileged instruction executed at CPL=3 lands here — this is
        // both containment and direct evidence the code ran in Ring 3.
        crate::serial_println!(
            "[ITISYOU:INFO] exception=general_protection cs_rpl=3 rip={:#x} error={:#x} action=terminate_process",
            frame.instruction_pointer.as_u64(),
            error_code
        );
        crate::user::transition::abort_fault(13);
    }
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
    if from_user(&frame) {
        use crate::user::transition;
        if let Ok(a) = addr {
            transition::FAULT_ADDR.store(a.as_u64(), Ordering::SeqCst);
            transition::FAULT_HAS_ADDR.store(true, Ordering::SeqCst);
        }
        crate::serial_println!(
            "[ITISYOU:INFO] exception=page_fault cs_rpl=3 rip={:#x} addr={:?} error={:?} action=terminate_process",
            frame.instruction_pointer.as_u64(),
            addr,
            error_code
        );
        transition::abort_fault(14);
    }
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

/// A wall-clock deadline built on the 100 Hz PIT tick counter.
///
/// Counting spin-loop iterations is NOT a timeout: it measures host CPU speed
/// and emulator throughput, not elapsed time, so the same code "times out"
/// after 3 ms on an idle host and never reaches the device's answer on a
/// loaded one. That is exactly how the V0.8 UHCI enumeration became
/// intermittent (`uhci enumerate_failed err=transfer timeout` under load while
/// the identical image passed when the machine was quiet). Device drivers must
/// bound their waits in real time instead.
///
/// A spin budget backs the tick check so a stopped timer can never wedge the
/// kernel forever — it is a safety net, never the primary bound.
pub struct Deadline {
    end_tick: u64,
    /// TSC value at which this deadline expires, or 0 when the TSC has not
    /// been calibrated yet.
    end_tsc: u64,
    spins_left: u64,
}

/// TSC cycles per millisecond, measured once at boot against the PIT. Zero
/// until [`calibrate_tsc`] runs.
static TSC_PER_MS: AtomicU64 = AtomicU64::new(0);

/// Read the timestamp counter.
///
/// This is the only clock in the kernel that keeps advancing with interrupts
/// disabled, which is exactly the situation inside a syscall: `SFMASK` clears
/// IF on entry, so the timer ISR does not run and [`ticks`] is frozen. A
/// polled wait that trusted the tick count would therefore spin its entire
/// backstop budget — minutes of a wedged machine — instead of timing out.
#[inline]
pub fn tsc() -> u64 {
    // SAFETY: RDTSC is unprivileged, has no side effects, and is available on
    // every x86_64 CPU.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// TSC cycles spanning `ms` milliseconds, or a conservative guess when the
/// counter has not been calibrated. Lets code rate-limit an action without
/// depending on the timer interrupt, which does not run inside a syscall.
pub fn cycles_for_ms(ms: u64) -> u64 {
    let per_ms = TSC_PER_MS.load(Ordering::Relaxed);
    if per_ms == 0 {
        // ~1 GHz: too low means retrying sooner than intended, never later,
        // so an uncalibrated system degrades to "slightly chatty", not "hung".
        ms.saturating_mul(1_000_000)
    } else {
        ms.saturating_mul(per_ms)
    }
}

/// Measure the TSC against the PIT. Must run with interrupts enabled, after
/// the timer is armed; called once during boot.
///
/// A wrong calibration is worse than none (deadlines would fire early or
/// never), so an implausible measurement is discarded and the tick path is
/// left in charge.
pub fn calibrate_tsc() {
    const SAMPLE_MS: u64 = 50;
    let start_tick = ticks();
    // Wait for the next tick edge so the sample is not skewed by however far
    // into the current tick we started.
    while ticks() == start_tick {
        core::hint::spin_loop();
    }
    let t0 = tsc();
    let base = ticks();
    let want = (SAMPLE_MS * TICK_HZ).div_ceil(1000);
    while ticks() - base < want {
        core::hint::spin_loop();
    }
    let elapsed_ticks = ticks() - base;
    let cycles = tsc().saturating_sub(t0);
    let elapsed_ms = elapsed_ticks * 1000 / TICK_HZ;
    if elapsed_ms == 0 {
        return;
    }
    let per_ms = cycles / elapsed_ms;
    // Anything outside 10 MHz–100 GHz is a measurement failure, not a CPU.
    if (10_000..100_000_000).contains(&per_ms) {
        TSC_PER_MS.store(per_ms, Ordering::SeqCst);
        crate::serial_println!("[ITISYOU:INFO] tsc_calibrated cycles_per_ms={per_ms}");
    }
}

/// Backstop for a stopped/unavailable timer. Large enough that it is never the
/// binding constraint while the PIT is running.
const DEADLINE_MAX_SPINS: u64 = 400_000_000;

impl Deadline {
    /// A deadline `ms` milliseconds from now, rounded up to whole ticks (the
    /// PIT's 10 ms granularity) and never zero-length.
    pub fn after_ms(ms: u64) -> Self {
        let ticks_needed = (ms * TICK_HZ).div_ceil(1000).max(1);
        let per_ms = TSC_PER_MS.load(Ordering::Relaxed);
        Self {
            end_tick: ticks().saturating_add(ticks_needed),
            end_tsc: if per_ms == 0 {
                0
            } else {
                tsc().saturating_add(per_ms.saturating_mul(ms.max(1)))
            },
            spins_left: DEADLINE_MAX_SPINS,
        }
    }

    /// True while budget remains; spends one spin of the backstop and issues a
    /// `pause` so the loop is a well-behaved busy-wait.
    pub fn pending(&mut self) -> bool {
        // Either clock expiring ends the wait. The TSC is authoritative when
        // interrupts are off (the tick count cannot advance there); the tick
        // count still bounds the wait if calibration was skipped.
        if self.spins_left == 0 {
            return false;
        }
        if self.end_tsc != 0 && tsc() >= self.end_tsc {
            return false;
        }
        if ticks() >= self.end_tick {
            return false;
        }
        self.spins_left -= 1;
        core::hint::spin_loop();
        true
    }
}

/// Busy-wait for at least `ms` milliseconds of real time. Used for the fixed
/// settle delays hardware specifications state in milliseconds (USB port
/// reset/recovery), which an iteration count cannot express.
pub fn delay_ms(ms: u64) {
    let mut deadline = Deadline::after_ms(ms);
    while deadline.pending() {}
}
