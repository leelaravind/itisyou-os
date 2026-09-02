//! Memory management: boot-map validation (B040), physical frame allocator
//! (B050), paging abstraction (B060), kernel heap (B070).

pub mod heap;
pub mod paging;

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use kernel_core::bitmap::{FrameBitmap, FrameError};
use kernel_core::memmap::{self, MemMapStats, RawRegion, RegionKind, FRAME_SIZE};
use spin::Mutex;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;

/// Supported physical address space: 4 GiB (1 Mi frames, 128 KiB bitmap in
/// .bss). Frames above this are counted and ignored — a documented V0.1
/// limitation, far above the QEMU test configuration (256 MiB).
const BITMAP_WORDS: usize = (4 * 1024 * 1024 * 1024 / FRAME_SIZE as usize) / 64;

/// Maximum boot regions we normalize. BIOS handoffs carry ~10 regions but
/// UEFI/OVMF hands over 100+ (observed: 104), so the bound is generous.
/// 256 × 24 B = 6 KiB of stack during init — safe on the 128 KiB boot stack.
const MAX_REGIONS: usize = 256;

/// The PMM lives directly in the static (`.bss`): the frame bitmap is 128 KiB
/// and must NEVER be constructed on the kernel stack (doing so overflowed the
/// boot stack and triple-faulted — see DEVELOPMENT_STORY 2026-09-02).
static PMM: Mutex<PhysicalMemoryManager> = Mutex::new(PhysicalMemoryManager {
    bitmap: FrameBitmap::new(),
    stats: PmmStats {
        total_usable_frames: 0,
        allocated_frames: 0,
        ignored_high_frames: 0,
    },
    initialized: false,
});

pub struct PhysicalMemoryManager {
    bitmap: FrameBitmap<BITMAP_WORDS>,
    stats: PmmStats,
    initialized: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PmmStats {
    pub total_usable_frames: u64,
    pub allocated_frames: u64,
    pub ignored_high_frames: u64,
}

/// Failure initializing or using the PMM.
#[derive(Debug, Clone, Copy)]
pub enum PmmError {
    InvalidMemoryMap(memmap::MemMapError),
    TooManyRegions,
    Frame(FrameError),
    NotInitialized,
}

/// Validate the boot memory map (emits stats for B040) and initialize the
/// physical frame allocator (B050).
pub fn init(regions: &MemoryRegions) -> Result<(MemMapStats, PmmStats), PmmError> {
    // Convert bootloader regions to the pure model. Everything not `Usable`
    // is treated as reserved: bootloader structures, kernel image, firmware.
    let mut raw = [RawRegion {
        start: 0,
        end: 0,
        kind: RegionKind::Reserved,
    }; MAX_REGIONS];
    if regions.len() > MAX_REGIONS {
        return Err(PmmError::TooManyRegions);
    }
    let mut n = 0;
    for region in regions.iter() {
        // Zero-length regions occasionally appear in handoffs; drop them
        // before validation (validation rejects them as malformed).
        if region.start == region.end {
            continue;
        }
        raw[n] = RawRegion {
            start: region.start,
            end: region.end,
            kind: match region.kind {
                MemoryRegionKind::Usable => RegionKind::Usable,
                _ => RegionKind::Reserved,
            },
        };
        n += 1;
    }
    let raw = &mut raw[..n];
    let map_stats = memmap::validate(raw).map_err(PmmError::InvalidMemoryMap)?;

    // Initialize the static PMM in place — no large stack temporaries.
    let mut pmm = PMM.lock();
    for frame_addr in memmap::usable_frames(raw) {
        let index = frame_addr / FRAME_SIZE;
        if index >= FrameBitmap::<BITMAP_WORDS>::CAPACITY {
            pmm.stats.ignored_high_frames += 1;
            continue;
        }
        // Frame 0 stays reserved: a null physical address is never handed out.
        if frame_addr == 0 {
            continue;
        }
        pmm.bitmap.mark_free(index).map_err(PmmError::Frame)?;
    }
    pmm.stats.total_usable_frames = pmm.bitmap.free_count();
    pmm.initialized = true;
    Ok((map_stats, pmm.stats))
}

/// Allocate one physical frame.
pub fn alloc_frame() -> Result<PhysFrame, PmmError> {
    let mut pmm = PMM.lock();
    if !pmm.initialized {
        return Err(PmmError::NotInitialized);
    }
    let index = pmm.bitmap.alloc().map_err(PmmError::Frame)?;
    pmm.stats.allocated_frames += 1;
    Ok(PhysFrame::containing_address(PhysAddr::new(
        index * FRAME_SIZE,
    )))
}

/// Free a previously allocated frame. Double frees are detected and returned
/// as errors (and would indicate a kernel bug).
pub fn free_frame(frame: PhysFrame) -> Result<(), PmmError> {
    let mut pmm = PMM.lock();
    if !pmm.initialized {
        return Err(PmmError::NotInitialized);
    }
    let index = frame.start_address().as_u64() / FRAME_SIZE;
    pmm.bitmap.free(index).map_err(PmmError::Frame)?;
    pmm.stats.allocated_frames = pmm.stats.allocated_frames.saturating_sub(1);
    Ok(())
}

/// Snapshot of allocator statistics for diagnostics/shell.
pub fn stats() -> Option<(u64, PmmStats)> {
    let pmm = PMM.lock();
    pmm.initialized
        .then(|| (pmm.bitmap.free_count(), pmm.stats))
}

/// Adapter so the paging layer can pull frames from the PMM.
pub struct PmmFrameSource;

unsafe impl FrameAllocator<Size4KiB> for PmmFrameSource {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        alloc_frame().ok()
    }
}
