//! Boot memory-map normalization and validation (requirement MEM-001).
//!
//! Pure logic: the kernel converts bootloader regions into [`RawRegion`]s and
//! this module validates/normalizes them without any allocation, so the same
//! code is unit-tested on the host with malformed fixtures.

/// Classification of a physical region after normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// Free for kernel allocation.
    Usable,
    /// In use by firmware/bootloader/kernel image — never allocate.
    Reserved,
}

/// A raw region as handed over by the bootloader (half-open: `[start, end)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawRegion {
    pub start: u64,
    pub end: u64,
    pub kind: RegionKind,
}

/// Validation failure for a boot memory map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemMapError {
    /// A region has `start >= end`.
    EmptyOrInverted { index: usize },
    /// Two *usable* regions overlap after sorting — the map is corrupt.
    OverlappingUsable { first: usize, second: usize },
    /// No usable memory at all.
    NoUsableMemory,
}

pub const FRAME_SIZE: u64 = 4096;

/// Validate a boot memory map (in-place sort by start address).
///
/// Rules (plan §10.3):
/// - every region must be non-empty and non-inverted;
/// - usable regions must not overlap each other (overlaps between usable and
///   reserved are resolved in favor of reserved by [`usable_frames`]);
/// - at least one usable frame must exist.
pub fn validate(regions: &mut [RawRegion]) -> Result<MemMapStats, MemMapError> {
    for (i, r) in regions.iter().enumerate() {
        if r.start >= r.end {
            return Err(MemMapError::EmptyOrInverted { index: i });
        }
    }
    regions.sort_unstable_by_key(|r| r.start);

    // Overlap check among usable regions only.
    let mut last_usable: Option<(usize, u64)> = None;
    for (i, r) in regions.iter().enumerate() {
        if r.kind != RegionKind::Usable {
            continue;
        }
        if let Some((prev_idx, prev_end)) = last_usable {
            if r.start < prev_end {
                return Err(MemMapError::OverlappingUsable {
                    first: prev_idx,
                    second: i,
                });
            }
        }
        last_usable = Some((i, r.end));
    }

    let stats = stats_of(regions);
    if stats.usable_frames == 0 {
        return Err(MemMapError::NoUsableMemory);
    }
    Ok(stats)
}

/// Summary statistics for a validated map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemMapStats {
    pub usable_frames: u64,
    pub reserved_frames: u64,
    pub highest_usable_addr: u64,
}

fn stats_of(regions: &[RawRegion]) -> MemMapStats {
    let mut usable = 0u64;
    let mut reserved = 0u64;
    let mut highest = 0u64;
    for r in regions {
        let frames = frames_in(r.start, r.end);
        match r.kind {
            RegionKind::Usable => {
                usable += frames;
                if r.end > highest {
                    highest = r.end;
                }
            }
            RegionKind::Reserved => reserved += frames,
        }
    }
    MemMapStats {
        usable_frames: usable,
        reserved_frames: reserved,
        highest_usable_addr: highest,
    }
}

/// Number of whole frames fully contained in `[start, end)`.
/// Partial frames at the edges are excluded for usable accounting (a frame is
/// only usable if every byte of it is usable).
pub fn frames_in(start: u64, end: u64) -> u64 {
    let first = start.div_ceil(FRAME_SIZE);
    let last = end / FRAME_SIZE;
    last.saturating_sub(first)
}

/// Iterate the frame-aligned usable frame start addresses of a validated map,
/// excluding any frame that intersects a reserved region.
pub fn usable_frames<'a>(regions: &'a [RawRegion]) -> impl Iterator<Item = u64> + 'a {
    regions
        .iter()
        .filter(|r| r.kind == RegionKind::Usable)
        .flat_map(move |r| {
            let first = r.start.div_ceil(FRAME_SIZE);
            let last = r.end / FRAME_SIZE;
            (first..last).map(|f| f * FRAME_SIZE).filter(move |&addr| {
                !regions.iter().any(|res| {
                    res.kind == RegionKind::Reserved
                        && addr < res.end
                        && addr + FRAME_SIZE > res.start
                })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usable(start: u64, end: u64) -> RawRegion {
        RawRegion {
            start,
            end,
            kind: RegionKind::Usable,
        }
    }
    fn reserved(start: u64, end: u64) -> RawRegion {
        RawRegion {
            start,
            end,
            kind: RegionKind::Reserved,
        }
    }

    #[test]
    fn valid_map_passes_and_reports_stats() {
        let mut map = [
            reserved(0, 0x1000),
            usable(0x1000, 0x5000),
            usable(0x8000, 0xA000),
        ];
        let stats = validate(&mut map).unwrap();
        assert_eq!(stats.usable_frames, 4 + 2);
        assert_eq!(stats.reserved_frames, 1);
        assert_eq!(stats.highest_usable_addr, 0xA000);
    }

    #[test]
    fn unsorted_input_is_normalized() {
        let mut map = [usable(0x8000, 0xA000), usable(0x1000, 0x5000)];
        validate(&mut map).unwrap();
        assert!(map[0].start < map[1].start);
    }

    #[test]
    fn rejects_inverted_region() {
        let mut map = [usable(0x5000, 0x1000)];
        assert_eq!(
            validate(&mut map),
            Err(MemMapError::EmptyOrInverted { index: 0 })
        );
    }

    #[test]
    fn rejects_empty_region() {
        let mut map = [usable(0x1000, 0x1000)];
        assert_eq!(
            validate(&mut map),
            Err(MemMapError::EmptyOrInverted { index: 0 })
        );
    }

    #[test]
    fn rejects_overlapping_usable() {
        let mut map = [usable(0x1000, 0x5000), usable(0x4000, 0x8000)];
        assert!(matches!(
            validate(&mut map),
            Err(MemMapError::OverlappingUsable { .. })
        ));
    }

    #[test]
    fn rejects_map_with_no_usable_memory() {
        let mut map = [reserved(0, 0x100000)];
        assert_eq!(validate(&mut map), Err(MemMapError::NoUsableMemory));
    }

    #[test]
    fn partial_edge_frames_are_not_usable() {
        // 0x1800..0x2800 fully contains no aligned frame except 0x2000? No:
        // frames are [0x2000, 0x3000) — but end is 0x2800, so zero frames.
        assert_eq!(frames_in(0x1800, 0x2800), 0);
        // 0x1800..0x3000 contains exactly frame 0x2000.
        assert_eq!(frames_in(0x1800, 0x3000), 1);
    }

    #[test]
    fn usable_frames_exclude_reserved_intersections() {
        let mut map = [usable(0x1000, 0x6000), reserved(0x3000, 0x4000)];
        validate(&mut map).unwrap();
        let frames: Vec<u64> = usable_frames(&map).collect();
        assert_eq!(frames, vec![0x1000, 0x2000, 0x4000, 0x5000]);
    }

    #[test]
    fn reserved_overlap_with_usable_is_tolerated_but_excluded() {
        let mut map = [usable(0x1000, 0x4000), reserved(0x3800, 0x5000)];
        validate(&mut map).unwrap();
        let frames: Vec<u64> = usable_frames(&map).collect();
        // Frame 0x3000 intersects reserved [0x3800, 0x5000) -> excluded.
        assert_eq!(frames, vec![0x1000, 0x2000]);
    }
}
