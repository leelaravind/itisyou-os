//! Kernel heap (B070): fixed virtual range backed by PMM frames, managed by
//! `linked_list_allocator` (correctness-focused, proven design — plan §10.5).

use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::VirtAddr;

use super::paging;

/// Heap placement: a fixed, otherwise-unused kernel virtual range far from
/// the bootloader's kernel/stack/physical-memory mappings. Grown to 32 MiB
/// in V0.5 to hold the compositor back buffer and window backing stores
/// (a 1024×768×4 buffer alone is 3 MiB).
pub const HEAP_START: u64 = 0x_4444_4444_0000;
pub const HEAP_SIZE: u64 = 32 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

#[derive(Debug)]
pub enum HeapError {
    Paging(paging::PagingError),
    Pmm(super::PmmError),
}

/// Map and initialize the heap. Called once after paging + PMM are up.
pub fn init() -> Result<(), HeapError> {
    let start = VirtAddr::new(HEAP_START);
    let end = VirtAddr::new(HEAP_START + HEAP_SIZE - 1);
    let first_page: Page = Page::containing_address(start);
    let last_page: Page = Page::containing_address(end);

    for page in Page::range_inclusive(first_page, last_page) {
        let frame = super::alloc_frame().map_err(HeapError::Pmm)?;
        paging::map_page(
            page,
            frame,
            PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
        )
        .map_err(HeapError::Paging)?;
    }
    // SAFETY: the range [HEAP_START, HEAP_START+HEAP_SIZE) is now mapped,
    // exclusively owned by the allocator, and initialized exactly once.
    unsafe {
        ALLOCATOR
            .lock()
            .init(HEAP_START as *mut u8, HEAP_SIZE as usize);
    }
    Ok(())
}

/// (used bytes, free bytes) for diagnostics.
pub fn stats() -> (usize, usize) {
    let heap = ALLOCATOR.lock();
    (heap.used(), heap.free())
}
