//! Guarded kernel task stacks (V0.10).
//!
//! Until V0.10 a kernel task's stack was a heap `Vec`: an overflow ran
//! straight into whatever the allocator had placed below it — exactly the
//! silent corruption V0.9 removed from the static syscall and double-fault
//! stacks (HARD09-001). Task stacks now live in a dedicated virtual window,
//! one fixed slot per task id: the first page of every slot is never mapped
//! (the guard), the rest are mapped writable and non-executable. An overflow
//! faults on the guard page, the fault cannot be delivered on the exhausted
//! stack, and the double-fault handler (on its own IST stack) names the task.
//!
//! Slots are never reused — tasks are never removed from the scheduler — so a
//! slot's address is a pure function of the task id and no allocator state is
//! needed. The window is only ever touched under the boot address space:
//! kernel tasks switch cooperatively from kernel context, never while a user
//! address space is active.

use super::{MAX_TASKS, STACK_SIZE};
use crate::memory::paging::{self, PagingError};
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

/// Base of the task-stack window: a kernel-half region used for nothing else
/// (the MMIO window starts at `0xFFFF_E000_0000_0000`).
const WINDOW: u64 = 0xFFFF_D000_0000_0000;
const PAGE: u64 = 4096;
/// One slot: a guard page, then the stack.
const SLOT: u64 = PAGE + STACK_SIZE as u64;

const _: () = assert!(STACK_SIZE.is_multiple_of(PAGE as usize));

/// Why a task stack could not be provided.
#[derive(Debug)]
pub enum StackError {
    SlotOutOfRange,
    NoMemory,
    Map(PagingError),
}

/// Map the stack for task `id` and return its top (16-byte aligned). The
/// slot's first page is left unmapped: the guard.
pub fn map(id: usize) -> Result<u64, StackError> {
    if id >= MAX_TASKS {
        return Err(StackError::SlotOutOfRange);
    }
    let base = WINDOW + id as u64 * SLOT;
    let flags = PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    for i in 0..(STACK_SIZE as u64 / PAGE) {
        let frame = crate::memory::alloc_frame().map_err(|_| StackError::NoMemory)?;
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(base + PAGE + i * PAGE));
        paging::map_page(page, frame, flags).map_err(StackError::Map)?;
    }
    Ok(base + SLOT)
}

/// Create the window's upper page tables NOW, at boot, before any process
/// exists (V0.10, TASK10-001).
///
/// Every process address space copies the kernel's top-level (L4) entries
/// when it is created. Until V0.10 the window's L4 entry appeared only with
/// the first `task::spawn`, so a process created earlier lacked it, and a
/// task switch while that process's page tables were active would fault on
/// its own stack. Mapping and unmapping one page builds the shared tables
/// through the ordinary mapper; the L4 entry then exists before anything can
/// copy the L4. The frame used for the probe page is returned to the pool.
pub fn init() -> Result<(), StackError> {
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(WINDOW));
    let frame = crate::memory::alloc_frame().map_err(|_| StackError::NoMemory)?;
    let flags = PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    paging::map_page(page, frame, flags).map_err(StackError::Map)?;
    let back = paging::unmap_page(page).map_err(StackError::Map)?;
    let _ = crate::memory::free_frame(back);
    crate::serial_println!(
        "[ITISYOU:HARDEN] task_stack_window l4_index={} shared_before_processes=true",
        (WINDOW >> 39) & 0x1FF
    );
    Ok(())
}

/// The window's top-level (L4) index.
pub const fn l4_index() -> usize {
    ((WINDOW >> 39) & 0x1FF) as usize
}

/// The task whose guard page contains `addr`, if any.
pub fn guard_hit(addr: u64) -> Option<usize> {
    let end = WINDOW + MAX_TASKS as u64 * SLOT;
    if !(WINDOW..end).contains(&addr) {
        return None;
    }
    let offset = addr - WINDOW;
    (offset % SLOT < PAGE).then_some((offset / SLOT) as usize)
}

/// True when the guard page of task `id` is unmapped (evidence for the log).
pub fn guard_unmapped(id: usize) -> bool {
    paging::is_unmapped(VirtAddr::new(WINDOW + id as u64 * SLOT))
}
