//! Virtual memory abstraction (B060) over the bootloader-provided mapping.
//!
//! The bootloader maps all physical memory at a dynamic offset; this module
//! owns the active level-4 table through `OffsetPageTable` and enforces the
//! kernel mapping policy: no writable+executable pages, explicit flushes.

use crate::sync::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::{
    FlagUpdateError, MapToError, TranslateResult, UnmapError,
};
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);

/// Physical-memory mapping offset (set once at init; read-only afterwards).
static PHYS_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The bootloader-provided ("kernel") L4 frame, active at boot.
static BOOT_L4: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Offset at which all physical memory is mapped (valid after init).
pub fn phys_offset() -> u64 {
    PHYS_OFFSET.load(core::sync::atomic::Ordering::Relaxed)
}

/// Physical address of the boot (kernel) level-4 table.
pub fn boot_l4_frame() -> u64 {
    BOOT_L4.load(core::sync::atomic::Ordering::Relaxed)
}

/// Kernel-space alias for a physical address, via the full physical mapping.
pub fn phys_to_virt(phys: u64) -> *mut u8 {
    (phys_offset() + phys) as *mut u8
}

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
    PHYS_OFFSET.store(
        physical_memory_offset,
        core::sync::atomic::Ordering::Relaxed,
    );
    BOOT_L4.store(
        l4_frame.start_address().as_u64(),
        core::sync::atomic::Ordering::Relaxed,
    );
    let l4_virt = offset + l4_frame.start_address().as_u64();
    let l4_table: &'static mut PageTable = unsafe { &mut *l4_virt.as_mut_ptr::<PageTable>() };
    // The BIOS boot path identity-maps handoff structures (boot GDT,
    // context-switch page) inside L4 entry 0. They are needed only until
    // the kernel installs its own GDT/IDT; `release_boot_identity_mappings`
    // unlinks them at B080 so the user window becomes process-exclusive.
    crate::serial_println!(
        "[ITISYOU:INFO] boot_l4_entry0={}",
        if l4_table[0].is_unused() {
            "free"
        } else {
            "boot-identity-mapped"
        }
    );
    let mapper = unsafe { OffsetPageTable::new(l4_table, offset) };
    *MAPPER.lock() = Some(mapper);
}

/// Unlink the bootloader's identity mappings from L4 entry 0 (V0.3).
///
/// Must run AFTER the kernel's own GDT/IDT are loaded (the boot GDT lives in
/// those mappings) and BEFORE the first process is created. The bootloader
/// table frames stay reserved in the memory map — unlink, never free.
pub fn release_boot_identity_mappings() {
    let l4_table = unsafe { &mut *((phys_offset() + boot_l4_frame()) as *mut PageTable) };
    let was_used = !l4_table[0].is_unused();
    l4_table[0].set_unused();
    // Full TLB flush: reload CR3.
    let (frame, flags) = Cr3::read();
    // SAFETY: rewriting the identical root; only stale low-address TLB
    // entries are discarded.
    unsafe { Cr3::write(frame, flags) };
    crate::serial_println!(
        "[ITISYOU:INFO] boot_identity_mappings released={was_used} user_window=exclusive"
    );
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

/// MMIO ranges mapped so far: (physical base, pages, virtual base).
///
/// V1.0 (found by the soak leg, V1-REL-003): a driver that initializes again
/// maps the same registers again - the NVMe store is opened afresh for
/// every store operation - and each call took a fresh stretch of the bump
/// window, so its page tables grew by a frame every few hundred file
/// operations and the window would eventually run out. A range already
/// mapped is now handed back instead. Device registers have one mapping,
/// uncacheable, whoever asks.
static MMIO_MAPS: Mutex<[Option<(u64, u64, u64)>; 32]> = Mutex::new([None; 32]);

/// Map a physical MMIO range into kernel virtual space, uncacheable, at a
/// dedicated MMIO window base. Returns the kernel virtual address of `phys`.
/// Used by device drivers (e.g. NVMe BAR0). Not for RAM. A range inside one
/// already mapped returns that mapping.
pub fn map_mmio(phys: u64, size: u64) -> Result<u64, PagingError> {
    use core::sync::atomic::{AtomicU64, Ordering};
    // Bump-allocated MMIO virtual window well clear of kernel/phys-map/user.
    static NEXT_MMIO_VIRT: AtomicU64 = AtomicU64::new(0xFFFF_E000_0000_0000);

    let page_off = phys & 0xFFF;
    let phys_base = phys & !0xFFF;
    let pages = (size + page_off).div_ceil(4096);
    let span = pages * 4096;
    let mut maps = MMIO_MAPS.lock();
    if let Some((base, _, virt)) = maps
        .iter()
        .flatten()
        .find(|&&(base, n, _)| phys_base >= base && phys_base + span <= base + n * 4096)
    {
        return Ok(virt + (phys_base - base) + page_off);
    }
    let virt_base = NEXT_MMIO_VIRT.fetch_add(span, Ordering::SeqCst);
    // `maps` stays locked across the mapping (lock order: MMIO_MAPS, then
    // MAPPER, and nothing takes them the other way), and the range is
    // recorded only once every page is mapped.

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE
        | PageTableFlags::NO_CACHE;
    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().ok_or(PagingError::NotInitialized)?;
    let mut frame_source = crate::memory::PmmFrameSource;
    for i in 0..pages {
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(virt_base + i * 4096));
        let frame = PhysFrame::containing_address(PhysAddr::new(phys_base + i * 4096));
        // SAFETY: mapping device MMIO (not RAM) into a fresh kernel window;
        // uncacheable so writes reach the device.
        unsafe {
            mapper
                .map_to(page, frame, flags, &mut frame_source)
                .map_err(PagingError::MapTo)?
                .flush();
        }
    }
    if let Some(slot) = maps.iter_mut().find(|m| m.is_none()) {
        *slot = Some((phys_base, pages, virt_base));
    }
    Ok(virt_base + page_off)
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

/// Translate a virtual address through the CURRENTLY ACTIVE page tables
/// (CR3) — the correct view for validating a running process's pointers.
pub fn translate_active(addr: VirtAddr) -> Option<PhysAddr> {
    let (l4_frame, _) = Cr3::read();
    let l4_virt = phys_offset() + l4_frame.start_address().as_u64();
    // SAFETY: read-only translation walk over the active root; single CPU
    // and the owning process is suspended while the kernel runs.
    let table = unsafe { &mut *(l4_virt as *mut PageTable) };
    let mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(phys_offset())) };
    match mapper.translate(addr) {
        TranslateResult::Mapped { frame, offset, .. } => Some(frame.start_address() + offset),
        _ => None,
    }
}

/// May Ring 3 access `addr` in the ACTIVE address space — every level of
/// the walk present and user-accessible, and writable too when `write`?
///
/// V0.10 (SEC10-001): the kernel's copies to and from user memory must
/// refuse a page the program itself could not touch that way. Checking only
/// that a page is mapped let a program point `args` or `cap_list` at its own
/// read-only code; the kernel then wrote there in Ring 0, CR0.WP turned the
/// write into a page fault, and a kernel-mode page fault panics. x86 grants
/// write access only if EVERY level allows it, so every level is checked,
/// not just the leaf.
pub fn user_accessible_active(addr: VirtAddr, write: bool) -> bool {
    let mut need = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if write {
        need |= PageTableFlags::WRITABLE;
    }
    let (l4_frame, _) = Cr3::read();
    let mut table_phys = l4_frame.start_address().as_u64();
    let indices = [
        addr.p4_index(),
        addr.p3_index(),
        addr.p2_index(),
        addr.p1_index(),
    ];
    for (level, index) in indices.into_iter().enumerate() {
        // SAFETY: `table_phys` is the active root or a table the previous
        // level's present entry points at; every physical frame is mapped at
        // `phys_offset`. Read only; single CPU, and the owning process is
        // suspended while the kernel runs (UNSAFE_INVENTORY row 22).
        let table = unsafe { &*((phys_offset() + table_phys) as *const PageTable) };
        let entry = &table[index];
        if !entry.flags().contains(need) {
            return false;
        }
        // A 1 GiB (level 1) or 2 MiB (level 2) page ends the walk.
        if level == 3 || (level > 0 && entry.flags().contains(PageTableFlags::HUGE_PAGE)) {
            return true;
        }
        table_phys = entry.addr().as_u64();
    }
    true
}

/// Translate a virtual address to its physical mapping, if mapped
/// (in the KERNEL/boot table — not process spaces).
pub fn translate(addr: VirtAddr) -> Option<PhysAddr> {
    let guard = MAPPER.lock();
    let mapper = guard.as_ref()?;
    match mapper.translate(addr) {
        TranslateResult::Mapped { frame, offset, .. } => Some(frame.start_address() + offset),
        _ => None,
    }
}
