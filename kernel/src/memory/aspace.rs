//! Per-process address spaces (V0.3, ADR-0005).
//!
//! Each process owns a private level-4 table: entries 1..512 are copied from
//! the boot L4 (kernel mappings shared by reference — supervisor-only, so
//! Ring 3 can never touch them), while entry 0 (the user window, 0..512 GiB)
//! is exclusively this process's subtree. Isolation between processes is
//! structural: their user subtrees share no table frames.
//!
//! Teardown walks the entry-0 subtree and returns every leaf and table frame
//! to the PMM — no Vec bookkeeping, no leaks by construction.

use super::paging::{self, PagingError};
use crate::memory::{self, PmmError};
use kernel_core::memmap::FRAME_SIZE;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

#[derive(Debug)]
pub enum AspaceError {
    Memory(PmmError),
    Paging(PagingError),
    /// Page already mapped in this space.
    AlreadyMapped {
        vaddr: u64,
    },
}

/// A private address space. Dropping does NOT free — call `teardown`.
pub struct AddressSpace {
    l4_phys: u64,
    /// Number of user leaf pages currently mapped (diagnostics).
    pub user_pages: u64,
}

impl AddressSpace {
    /// Create a fresh space: zeroed L4 with kernel entries shared from the
    /// boot table and an empty user window.
    pub fn new() -> Result<AddressSpace, AspaceError> {
        let frame = memory::alloc_frame().map_err(AspaceError::Memory)?;
        let l4_phys = frame.start_address().as_u64();
        let new_l4 = unsafe { table_mut(l4_phys) };
        let boot_l4 = unsafe { table_mut(paging::boot_l4_frame()) };
        new_l4.zero();
        for i in 1..512 {
            new_l4[i] = boot_l4[i].clone();
        }
        Ok(AddressSpace {
            l4_phys,
            user_pages: 0,
        })
    }

    /// True when this space's top-level entry `idx` is present and equal to
    /// the boot (kernel) table's — i.e. the kernel region it covers is shared.
    pub fn shares_kernel_l4_entry(&self, idx: usize) -> bool {
        if idx >= 512 {
            return false;
        }
        // SAFETY: both frames are live L4 tables owned by the kernel; read only.
        let (mine, boot) = unsafe {
            (
                table_mut(self.l4_phys)[idx].clone(),
                table_mut(paging::boot_l4_frame())[idx].clone(),
            )
        };
        !mine.is_unused() && mine.addr() == boot.addr() && mine.flags() == boot.flags()
    }

    pub fn l4_phys(&self) -> u64 {
        self.l4_phys
    }

    /// True while this space is the active CR3.
    pub fn is_active(&self) -> bool {
        Cr3::read().0.start_address().as_u64() == self.l4_phys
    }

    /// Map a user page in THIS space. Same flag policy as the shared-space
    /// user mapper (PRESENT|USER, W^X enforced). Returns the backing frame
    /// so the loader can fill it through the physical alias.
    pub fn map_user_page(
        &mut self,
        vaddr: u64,
        writable: bool,
        executable: bool,
    ) -> Result<PhysFrame, AspaceError> {
        if writable && executable {
            return Err(AspaceError::Paging(PagingError::WxViolation));
        }
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(vaddr));
        let mut mapper = unsafe { self.mapper() };
        if mapper.translate_addr(page.start_address()).is_some() {
            return Err(AspaceError::AlreadyMapped { vaddr });
        }
        let frame = memory::alloc_frame().map_err(AspaceError::Memory)?;
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if writable {
            flags |= PageTableFlags::WRITABLE;
        }
        if !executable {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        let parent =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        let mut frame_source = crate::memory::PmmFrameSource;
        // SAFETY: mapping a PMM-owned frame at an unmapped user vaddr in a
        // table this space exclusively owns. Flush only matters if active.
        let result = unsafe {
            mapper.map_to_with_table_flags(page, frame, flags, parent, &mut frame_source)
        };
        match result {
            Ok(flush) => {
                if self.is_active() {
                    flush.flush();
                } else {
                    flush.ignore();
                }
                // Zero through the physical alias so stale data never leaks
                // into userspace, regardless of which CR3 is active.
                // SAFETY: fresh exclusively-owned frame.
                unsafe {
                    core::ptr::write_bytes(
                        paging::phys_to_virt(frame.start_address().as_u64()),
                        0,
                        FRAME_SIZE as usize,
                    );
                }
                self.user_pages += 1;
                Ok(frame)
            }
            Err(e) => {
                let _ = memory::free_frame(frame);
                Err(AspaceError::Paging(PagingError::MapTo(e)))
            }
        }
    }

    /// Tighten an existing user mapping's protection (post-copy).
    pub fn update_user_flags(
        &mut self,
        vaddr: u64,
        writable: bool,
        executable: bool,
    ) -> Result<(), AspaceError> {
        if writable && executable {
            return Err(AspaceError::Paging(PagingError::WxViolation));
        }
        let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(vaddr));
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if writable {
            flags |= PageTableFlags::WRITABLE;
        }
        if !executable {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        let mut mapper = unsafe { self.mapper() };
        // SAFETY: flag-only change on a mapping this space owns.
        match unsafe { mapper.update_flags(page, flags) } {
            Ok(flush) => {
                if self.is_active() {
                    flush.flush();
                } else {
                    flush.ignore();
                }
                Ok(())
            }
            Err(e) => Err(AspaceError::Paging(PagingError::FlagUpdate(e))),
        }
    }

    /// Translate a user virtual address in THIS space (active or not).
    pub fn translate(&self, vaddr: u64) -> Option<u64> {
        let mapper = unsafe { self.mapper_shared() };
        mapper
            .translate_addr(VirtAddr::new(vaddr))
            .map(|p| p.as_u64())
    }

    /// Free the entire user subtree (leaf frames + intermediate tables) and
    /// the L4 frame itself. The space must not be active.
    pub fn teardown(self) {
        assert!(
            !self.is_active(),
            "cannot tear down the active address space"
        );
        let l4 = unsafe { table_mut(self.l4_phys) };
        let entry0 = &mut l4[0];
        if !entry0.is_unused() {
            free_subtree(entry0.addr(), 3);
            entry0.set_unused();
        }
        free_phys(self.l4_phys);
    }

    /// Build a mapper over this space's tables (mutable operations).
    ///
    /// # Safety
    /// Caller must not create aliasing mappers concurrently (single CPU,
    /// spaces are owned by one process host at a time).
    unsafe fn mapper(&mut self) -> OffsetPageTable<'_> {
        let table = unsafe { table_mut(self.l4_phys) };
        unsafe { OffsetPageTable::new(table, VirtAddr::new(paging::phys_offset())) }
    }

    /// Read-only mapper (translate).
    unsafe fn mapper_shared(&self) -> OffsetPageTable<'_> {
        let table = unsafe { table_mut(self.l4_phys) };
        unsafe { OffsetPageTable::new(table, VirtAddr::new(paging::phys_offset())) }
    }
}

/// View a physical frame as a page table via the physical-memory alias.
///
/// # Safety
/// `phys` must be a page-table frame owned by the caller's space (or the
/// boot table, read-shared); single CPU serializes access.
unsafe fn table_mut(phys: u64) -> &'static mut PageTable {
    unsafe { &mut *(paging::phys_to_virt(phys) as *mut PageTable) }
}

/// Recursively free a user page-table subtree. `level` 3 = the table pointed
/// to by an L4 entry; level 1's entries point at leaf data frames.
fn free_subtree(table_phys: PhysAddr, level: u8) {
    let table = unsafe { table_mut(table_phys.as_u64()) };
    for entry in table.iter() {
        if entry.is_unused() {
            continue;
        }
        // Huge pages are never created for user mappings (4 KiB only).
        if level > 1 {
            free_subtree(entry.addr(), level - 1);
        } else {
            free_phys(entry.addr().as_u64());
        }
    }
    free_phys(table_phys.as_u64());
}

fn free_phys(phys: u64) {
    let frame = PhysFrame::containing_address(PhysAddr::new(phys));
    // Frames the PMM does not track (shouldn't happen for user subtrees)
    // surface as errors in stats rather than corrupting the allocator.
    let _ = memory::free_frame(frame);
}

/// Switch CR3 to the given L4 (no-op when already active).
pub fn activate_l4(l4_phys: u64) {
    let (current, flags) = Cr3::read();
    if current.start_address().as_u64() != l4_phys {
        let frame = PhysFrame::containing_address(PhysAddr::new(l4_phys));
        // SAFETY: every process L4 shares the kernel mappings, so kernel
        // execution continues unaffected across the switch.
        unsafe { Cr3::write(frame, flags) };
    }
}
