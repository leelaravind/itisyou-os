//! x86_64 context switch primitives.
//!
//! ABI: `x86_64-unknown-none` uses the System V calling convention. A context
//! is the callee-saved register set + stack pointer; everything else is dead
//! across the explicit `switch_context` call site.

/// Switch stacks: save callee-saved registers on the current stack, store
/// RSP through `old_rsp_slot`, load `new_rsp`, restore registers, return
/// into the new context.
///
/// # Safety
/// `old_rsp_slot` must be valid writable storage that outlives the switch;
/// `new_rsp` must point at a stack seeded by `spawn` or saved by a previous
/// switch of a live task.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(old_rsp_slot: *mut u64, new_rsp: u64) {
    core::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

/// First frame of every spawned task: runs the entry function, then
/// terminates the task. Never returns.
pub extern "C" fn task_trampoline() -> ! {
    if let Some(entry) = super::take_current_entry() {
        entry();
    }
    super::exit_current()
}
