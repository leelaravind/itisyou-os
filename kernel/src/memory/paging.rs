//! Virtual memory abstraction (B060) over the bootloader-provided mapping.
//!
//! The bootloader maps all physical memory at a dynamic offset; this module
//! owns the active level-4 table through `OffsetPageTable` and enforces the
//! kernel mapping policy: no writable+executable pages, explicit flushes.

use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::{
    FlagUpdateError, MapToError, TranslateResult, UnmapError,
};
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
    FlagUpdate(FlagUpdateError),
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

/// Map a userspace page (V0.2). Leaf flags: PRESENT | USER_ACCESSIBLE,
/// plus WRITABLE and/or NO_EXECUTE according to the requested protection.
/// W^X is enforced: writable+executable is rejected.
///
/// Parent table entries are created with USER_ACCESSIBLE (required for
/// Ring 3 translation) — this only affects the page-table path of the user
/// window, never kernel leaf mappings.
pub fn map_user_page(
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    writable: bool,
    executable: bool,
) -> Result<(), PagingError> {
    if writable && executable {
        return Err(PagingError::WxViolation);
    }
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }
    let parent_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().ok_or(PagingError::NotInitialized)?;
    let mut frame_source = crate::memory::PmmFrameSource;
    // SAFETY: caller maps PMM-owned frames into the (verified-unmapped)
    // user window; parent USER flag is confined to that window's path.
    unsafe {
        mapper
            .map_to_with_table_flags(page, frame, flags, parent_flags, &mut frame_source)
            .map_err(PagingError::MapTo)?
            .flush();
    }
    Ok(())
}

/// Change the protection flags of an existing user mapping (used to drop
/// WRITABLE / set executability after segment contents are copied in).
/// W^X and USER/PRESENT invariants are enforced here, not trusted from the
/// caller.
pub fn update_user_flags(
    page: Page<Size4KiB>,
    writable: bool,
    executable: bool,
) -> Result<(), PagingError> {
    if writable && executable {
        return Err(PagingError::WxViolation);
    }
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }
    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().ok_or(PagingError::NotInitialized)?;
    // SAFETY: only flag bits change; the mapping itself was established by
    // map_user_page.
    unsafe {
        mapper
            .update_flags(page, flags)
            .map_err(PagingError::FlagUpdate)?
            .flush();
    }
    Ok(())
}

/// True when no mapping exists for the address (any level).
pub fn is_unmapped(addr: VirtAddr) -> bool {
    translate(addr).is_none()
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
