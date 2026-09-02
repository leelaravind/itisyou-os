//! Virtual memory abstraction (B060) over the bootloader-provided mapping.
//!
//! The bootloader maps all physical memory at a dynamic offset; this module
//! owns the active level-4 table through `OffsetPageTable` and enforces the
//! kernel mapping policy: no writable+executable pages, explicit flushes.

use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::{MapToError, TranslateResult, UnmapError};
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);

/// Mapping failure surfaced to callers.
#[derive(Debug)]
pub enum PagingError {
    NotInitialized,
    /// Policy violation: writable + executable requested.
    WxViolation,
    MapTo(MapToError<Size4KiB>),
    Unmap(UnmapError),
}

/// Initialize the paging abstraction.
///
/// # Safety
/// `physical_memory_offset` must be the offset at which the bootloader mapped
/// the complete physical address space, and this must be called exactly once
/// while that mapping is active.
pub unsafe fn init(physical_memory_offset: u64) {
    let offset = VirtAddr::new(physical_memory_offset);
    let (l4_frame, _flags) = Cr3::read();
    let l4_virt = offset + l4_frame.start_address().as_u64();
    let l4_table: &'static mut PageTable = unsafe { &mut *l4_virt.as_mut_ptr::<PageTable>() };
    let mapper = unsafe { OffsetPageTable::new(l4_table, offset) };
    *MAPPER.lock() = Some(mapper);
}

/// Map `page` to `frame` with `flags` (PRESENT is implied).
///
/// Policy: WRITABLE and missing NO_EXECUTE together are rejected — kernel
/// data mappings must never be executable (plan §11.2).
pub fn map_page(
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), PagingError> {
    let flags = flags | PageTableFlags::PRESENT;
    if flags.contains(PageTableFlags::WRITABLE) && !flags.contains(PageTableFlags::NO_EXECUTE) {
        return Err(PagingError::WxViolation);
    }
    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().ok_or(PagingError::NotInitialized)?;
    let mut frame_source = crate::memory::PmmFrameSource;
    // SAFETY: the caller maps kernel-owned frames into unused kernel virtual
    // space; the PMM guarantees the frame is exclusively owned.
    unsafe {
        mapper
            .map_to(page, frame, flags, &mut frame_source)
            .map_err(PagingError::MapTo)?
            .flush();
    }
    Ok(())
}

/// Unmap `page`, returning the frame that backed it. The TLB entry is
/// flushed before returning.
pub fn unmap_page(page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, PagingError> {
    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().ok_or(PagingError::NotInitialized)?;
    let (frame, flush) = mapper.unmap(page).map_err(PagingError::Unmap)?;
    flush.flush();
    Ok(frame)
}

/// Translate a virtual address to its physical mapping, if mapped.
pub fn translate(addr: VirtAddr) -> Option<PhysAddr> {
    let guard = MAPPER.lock();
    let mapper = guard.as_ref()?;
    match mapper.translate(addr) {
        TranslateResult::Mapped { frame, offset, .. } => Some(frame.start_address() + offset),
        _ => None,
    }
}
