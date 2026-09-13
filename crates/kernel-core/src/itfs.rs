//! ITFS — the ITISYOU Trivial File System (V0.4, ADR-0009).
//!
//! A deliberately minimal, correctness-first filesystem: a fixed on-disk
//! directory of contiguous, append-only files, committed through a
//! double-buffered CRC-protected superblock so metadata updates are atomic
//! against a crash. This crate is the pure on-disk-format layer (encode /
//! decode / validation), host-unit-tested against malformed images; the
//! kernel's `fs_disk` module performs the block I/O.
//!
//! On-disk layout (512-byte blocks):
//!   block 0 : superblock slot A
//!   block 1 : superblock slot B   (double buffer)
//!   block 2+: contiguous file data
//!
//! A commit writes the new superblock (generation+1) to the slot NOT holding
//! the current committed state. A crash mid-write leaves the other slot's
//! older-but-consistent superblock intact; `mount` picks the valid slot with
//! the highest generation.
//!
//! Space reclamation (V0.10). Every file is one contiguous extent and the
//! superblock lists every live extent, so free space is exactly the gaps
//! between live extents: it is recomputed from the directory whenever it is
//! needed and no free list is ever stored — the on-disk format is unchanged.
//! Allocation is first-fit over those gaps.
//!
//! The crash argument that makes reuse safe: new data is only ever written to
//! blocks that NO valid superblock on the disk references. The caller passes
//! the committed superblock and the one in the other slot (the generation a
//! mount falls back to) as `pinned`, and the allocator refuses to hand out any
//! block either of them references — including the old extent of a file that
//! is being overwritten in the same transaction. The data is flushed before
//! the superblock commit swings the reference onto it, and an extent becomes
//! allocatable only once no surviving superblock points at it. So a crash at
//! any point — mid-data, mid-commit — leaves two slots whose every valid
//! superblock references intact data, exactly as the bump allocator did.
//!
//! `next_free_block` is kept as a high-water mark (one past the highest block
//! any live entry reaches). It no longer drives allocation, but it stays
//! consistent so the V0.9 decoder's `end <= next_free_block` check still
//! holds for volumes this code writes.

pub const BLOCK_SIZE: usize = 512;
pub const MAX_FILES: usize = 12;
pub const NAME_LEN: usize = 24;
pub const DATA_START_BLOCK: u32 = 2;
const MAGIC: &[u8; 8] = b"ITFS0001";
const FLAG_USED: u32 = 1;

const OFF_MAGIC: usize = 0;
const OFF_GEN: usize = 8;
const OFF_BLOCK_SIZE: usize = 16;
const OFF_TOTAL_BLOCKS: usize = 20;
const OFF_FILE_COUNT: usize = 24;
const OFF_NEXT_FREE: usize = 28;
const OFF_ENTRIES: usize = 48;
const ENTRY_SIZE: usize = 36; // name[24] + size u32 + start u32 + flags u32
const OFF_CRC: usize = 508;

/// Filesystem/format errors (all are safe rejections).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    BadMagic,
    BadBlockSize,
    BadCrc,
    /// file_count or an entry references out-of-range blocks.
    InconsistentMetadata,
    NameTooLong,
    NameEmpty,
    NotFound,
    Exists,
    /// No free space / too many files.
    NoSpace,
    /// Enough blocks are free in total, but no single gap is large enough.
    /// Files are contiguous, so this is a real (and clean) refusal, not an
    /// accounting error: removing or shrinking a neighbour opens a gap.
    Fragmented,
    /// Neither superblock slot is valid.
    NoValidSuperblock,
    BufferTooSmall,
}

/// One directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry {
    pub name: [u8; NAME_LEN],
    pub size: u32,
    pub start_block: u32,
    pub flags: u32,
}

impl DirEntry {
    pub const EMPTY: DirEntry = DirEntry {
        name: [0; NAME_LEN],
        size: 0,
        start_block: 0,
        flags: 0,
    };
    pub fn used(&self) -> bool {
        self.flags & FLAG_USED != 0
    }
    /// Name as a &str (trimmed at the first NUL), if valid UTF-8.
    pub fn name_str(&self) -> Option<&str> {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).ok()
    }
    pub fn block_count(&self) -> u32 {
        blocks_for(self.size)
    }
    /// The run of data blocks this entry occupies (empty for a 0-byte file).
    pub fn extent(&self) -> Extent {
        Extent {
            start: self.start_block,
            blocks: self.block_count(),
        }
    }
}

/// Blocks needed to hold `size` bytes.
pub fn blocks_for(size: u32) -> u32 {
    size.div_ceil(BLOCK_SIZE as u32)
}

/// A contiguous run of data blocks `[start, start + blocks)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub start: u32,
    pub blocks: u32,
}

impl Extent {
    /// One past the last block, widened so a hostile `start` cannot overflow.
    pub fn end(&self) -> u64 {
        self.start as u64 + self.blocks as u64
    }
    /// True if the two runs share at least one block. Empty runs share none.
    pub fn overlaps(&self, other: &Extent) -> bool {
        self.blocks != 0
            && other.blocks != 0
            && (self.start as u64) < other.end()
            && (other.start as u64) < self.end()
    }
}

/// Space accounting for a mounted volume (all counts in blocks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    /// Blocks in the data area (the volume minus the two superblock slots).
    pub data_blocks: u32,
    /// Blocks referenced by the committed directory.
    pub used: u32,
    /// `data_blocks - used`: what the committed directory does not reference.
    pub free: u32,
    /// Free blocks the allocator may NOT hand out yet because the fallback
    /// superblock (the other slot) still references them. They become
    /// allocatable at the next commit, which overwrites that slot.
    pub pinned: u32,
    /// Largest contiguous run the next allocation can use.
    pub largest_run: u32,
    pub files: u32,
}

/// In-memory superblock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperBlock {
    pub generation: u64,
    pub total_blocks: u32,
    pub file_count: u32,
    pub next_free_block: u32,
    pub entries: [DirEntry; MAX_FILES],
}

impl SuperBlock {
    /// A freshly formatted (empty) filesystem over `total_blocks` blocks.
    pub fn empty(total_blocks: u32) -> SuperBlock {
        SuperBlock {
            generation: 1,
            total_blocks,
            file_count: 0,
            next_free_block: DATA_START_BLOCK,
            entries: [DirEntry::EMPTY; MAX_FILES],
        }
    }

    /// Find a used entry by name.
    pub fn find(&self, name: &str) -> Option<&DirEntry> {
        self.entries
            .iter()
            .find(|e| e.used() && e.name_str() == Some(name))
    }

    /// Live extents of this directory (0-byte files occupy no blocks).
    fn live_extents(&self) -> impl Iterator<Item = Extent> + '_ {
        self.entries
            .iter()
            .filter(|e| e.used() && e.block_count() > 0)
            .map(|e| e.extent())
    }

    /// True if `[start, start + blocks)` lies inside this volume's data area
    /// and no live extent of this directory touches it. This is the check a
    /// caller runs against the COMMITTED superblock before writing a single
    /// data block (the crash-safety invariant, enforced independently of the
    /// allocator that chose the run).
    pub fn run_is_unreferenced(&self, start: u32, blocks: u32) -> bool {
        let run = Extent { start, blocks };
        if blocks == 0 {
            return true;
        }
        start >= DATA_START_BLOCK
            && run.end() <= self.total_blocks as u64
            && !self.live_extents().any(|e| e.overlaps(&run))
    }

    /// Walk the maximal runs of the data area that no live extent of `self`
    /// or of any `pinned` directory references, lowest address first.
    fn for_each_free_run(&self, pinned: &[&SuperBlock], mut f: impl FnMut(u32, u32) -> bool) {
        let total = self.total_blocks as u64;
        let all = || {
            core::iter::once(self)
                .chain(pinned.iter().copied())
                .flat_map(|sb| sb.live_extents())
        };
        let mut pos = DATA_START_BLOCK as u64;
        while pos < total {
            // Inside an extent: skip to the furthest end covering `pos`.
            if let Some(end) = all()
                .filter(|e| (e.start as u64) <= pos && pos < e.end())
                .map(|e| e.end())
                .max()
            {
                pos = end;
                continue;
            }
            // Free: the run lasts until the next extent starts (or the end).
            let next = all()
                .map(|e| e.start as u64)
                .filter(|&s| s > pos)
                .min()
                .unwrap_or(total)
                .min(total);
            // Both bounds are <= total_blocks (a u32), so these cannot truncate.
            if !f(pos as u32, (next - pos) as u32) {
                return;
            }
            pos = next;
        }
    }

    /// First-fit: the lowest run of `blocks` contiguous blocks referenced by
    /// neither this working directory nor any `pinned` one.
    fn find_run(&self, blocks: u32, pinned: &[&SuperBlock]) -> Result<u32, FsError> {
        if blocks == 0 {
            // Nothing is written; any in-range start is valid metadata.
            return Ok(DATA_START_BLOCK);
        }
        let mut found = None;
        let mut reusable: u64 = 0;
        self.for_each_free_run(pinned, |start, len| {
            reusable += len as u64;
            if len >= blocks {
                found = Some(start);
                return false;
            }
            true
        });
        match found {
            Some(start) => Ok(start),
            // Enough in total but not in one piece: say so, rather than
            // claiming the disk is full.
            None if reusable >= blocks as u64 => Err(FsError::Fragmented),
            None => Err(FsError::NoSpace),
        }
    }

    /// Recompute the high-water mark after the directory changed.
    fn recompute_high_water(&mut self) {
        let high = self
            .entries
            .iter()
            .filter(|e| e.used())
            .map(|e| e.extent().end())
            .max()
            .unwrap_or(0)
            .max(DATA_START_BLOCK as u64);
        // Every used entry was placed inside the volume, so high <= total.
        self.next_free_block = u32::try_from(high).unwrap_or(self.total_blocks);
    }

    /// Space accounting, with `pinned` being the other directories whose
    /// extents the allocator must still avoid (normally the fallback slot).
    pub fn space(&self, pinned: &[&SuperBlock]) -> Space {
        let data_blocks = self.total_blocks.saturating_sub(DATA_START_BLOCK);
        let mut free = 0u32;
        self.for_each_free_run(&[], |_, len| {
            free += len;
            true
        });
        let mut reusable = 0u32;
        let mut largest_run = 0u32;
        self.for_each_free_run(pinned, |_, len| {
            reusable += len;
            largest_run = largest_run.max(len);
            true
        });
        Space {
            data_blocks,
            used: data_blocks - free,
            free,
            pinned: free - reusable,
            largest_run,
            files: self.file_count,
        }
    }

    fn check_name(name: &str) -> Result<(), FsError> {
        if name.is_empty() {
            return Err(FsError::NameEmpty);
        }
        if name.len() > NAME_LEN {
            return Err(FsError::NameTooLong);
        }
        Ok(())
    }

    /// Reserve a contiguous run for a new file and return its entry index +
    /// start block. Does not modify data blocks (the caller writes them).
    ///
    /// Only this directory's own extents are avoided, which is correct when
    /// `self` is an unmodified copy of the committed superblock and nothing
    /// else on the disk needs protecting. The kernel uses
    /// [`SuperBlock::allocate_avoiding`] so the fallback slot is honoured too.
    pub fn allocate(&mut self, name: &str, size: u32) -> Result<(usize, u32), FsError> {
        self.allocate_avoiding(name, size, &[])
    }

    /// [`SuperBlock::allocate`], additionally refusing any block that a
    /// `pinned` directory references (the committed and fallback superblocks).
    /// On error `self` is unchanged.
    pub fn allocate_avoiding(
        &mut self,
        name: &str,
        size: u32,
        pinned: &[&SuperBlock],
    ) -> Result<(usize, u32), FsError> {
        Self::check_name(name)?;
        if self.find(name).is_some() {
            return Err(FsError::Exists);
        }
        let slot = self
            .entries
            .iter()
            .position(|e| !e.used())
            .ok_or(FsError::NoSpace)?;
        let start = self.find_run(blocks_for(size), pinned)?;
        let mut name_buf = [0u8; NAME_LEN];
        name_buf[..name.len()].copy_from_slice(name.as_bytes());
        self.entries[slot] = DirEntry {
            name: name_buf,
            size,
            start_block: start,
            flags: FLAG_USED,
        };
        self.file_count += 1;
        self.generation += 1;
        self.recompute_high_water();
        Ok((slot, start))
    }

    /// Remove a file's directory entry (V0.7 — uninstall/rollback support).
    /// The entry is cleared and `file_count` decremented in one superblock
    /// generation, so persisting it is a single atomic double-buffered commit.
    ///
    /// V0.10: the extent becomes a gap, and gaps are free space — but only
    /// for directories that no longer reference it. Until this generation is
    /// committed AND the slot holding the previous one is overwritten, the
    /// caller keeps that previous superblock in `pinned`, so the blocks are
    /// not handed out while a surviving superblock still points at them.
    pub fn remove(&mut self, name: &str) -> Result<(), FsError> {
        let slot = self
            .entries
            .iter()
            .position(|e| e.used() && e.name_str() == Some(name))
            .ok_or(FsError::NotFound)?;
        self.entries[slot] = DirEntry::EMPTY;
        self.file_count -= 1;
        self.generation += 1;
        self.recompute_high_water();
        Ok(())
    }

    /// Reserve a run for `name`, replacing any existing file of that name in
    /// the SAME superblock generation (V0.8).
    ///
    /// This is what makes a userspace overwrite crash-atomic. Doing it as
    /// `remove` then `allocate` would be two commits, and a crash between them
    /// leaves the file gone — the caller asked to replace its contents, not to
    /// risk losing them. Here the old extent is still referenced by the
    /// committed superblock until the moment the new one lands, so a crash at
    /// any point leaves exactly the old file or exactly the new one.
    pub fn replace(&mut self, name: &str, size: u32) -> Result<(usize, u32), FsError> {
        self.replace_avoiding(name, size, &[])
    }

    /// [`SuperBlock::replace`], additionally avoiding `pinned` directories.
    ///
    /// The new run is chosen while the old entry is still present in `self`,
    /// so the file's own still-committed extent can never be handed back to
    /// it in the same transaction (that would overwrite the only good copy
    /// before the commit that stops referencing it). The entry keeps its
    /// directory slot. On error `self` is unchanged.
    pub fn replace_avoiding(
        &mut self,
        name: &str,
        size: u32,
        pinned: &[&SuperBlock],
    ) -> Result<(usize, u32), FsError> {
        Self::check_name(name)?;
        let Some(slot) = self
            .entries
            .iter()
            .position(|e| e.used() && e.name_str() == Some(name))
        else {
            return self.allocate_avoiding(name, size, pinned);
        };
        let start = self.find_run(blocks_for(size), pinned)?;
        self.entries[slot].size = size;
        self.entries[slot].start_block = start;
        self.generation += 1;
        self.recompute_high_water();
        Ok((slot, start))
    }

    /// Encode to a 512-byte block with a fresh CRC.
    pub fn encode(&self) -> [u8; BLOCK_SIZE] {
        let mut b = [0u8; BLOCK_SIZE];
        b[OFF_MAGIC..OFF_MAGIC + 8].copy_from_slice(MAGIC);
        b[OFF_GEN..OFF_GEN + 8].copy_from_slice(&self.generation.to_le_bytes());
        b[OFF_BLOCK_SIZE..OFF_BLOCK_SIZE + 4].copy_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
        b[OFF_TOTAL_BLOCKS..OFF_TOTAL_BLOCKS + 4].copy_from_slice(&self.total_blocks.to_le_bytes());
        b[OFF_FILE_COUNT..OFF_FILE_COUNT + 4].copy_from_slice(&self.file_count.to_le_bytes());
        b[OFF_NEXT_FREE..OFF_NEXT_FREE + 4].copy_from_slice(&self.next_free_block.to_le_bytes());
        for (i, e) in self.entries.iter().enumerate() {
            let o = OFF_ENTRIES + i * ENTRY_SIZE;
            b[o..o + NAME_LEN].copy_from_slice(&e.name);
            b[o + 24..o + 28].copy_from_slice(&e.size.to_le_bytes());
            b[o + 28..o + 32].copy_from_slice(&e.start_block.to_le_bytes());
            b[o + 32..o + 36].copy_from_slice(&e.flags.to_le_bytes());
        }
        let crc = crc32(&b[..OFF_CRC]);
        b[OFF_CRC..OFF_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Decode + strictly validate a superblock block.
    pub fn decode(b: &[u8]) -> Result<SuperBlock, FsError> {
        if b.len() < BLOCK_SIZE {
            return Err(FsError::BufferTooSmall);
        }
        if &b[OFF_MAGIC..OFF_MAGIC + 8] != MAGIC {
            return Err(FsError::BadMagic);
        }
        let block_size =
            u32::from_le_bytes(b[OFF_BLOCK_SIZE..OFF_BLOCK_SIZE + 4].try_into().unwrap());
        if block_size != BLOCK_SIZE as u32 {
            return Err(FsError::BadBlockSize);
        }
        let stored_crc = u32::from_le_bytes(b[OFF_CRC..OFF_CRC + 4].try_into().unwrap());
        if crc32(&b[..OFF_CRC]) != stored_crc {
            return Err(FsError::BadCrc);
        }
        let generation = u64::from_le_bytes(b[OFF_GEN..OFF_GEN + 8].try_into().unwrap());
        let total_blocks = u32::from_le_bytes(
            b[OFF_TOTAL_BLOCKS..OFF_TOTAL_BLOCKS + 4]
                .try_into()
                .unwrap(),
        );
        let file_count =
            u32::from_le_bytes(b[OFF_FILE_COUNT..OFF_FILE_COUNT + 4].try_into().unwrap());
        let next_free_block =
            u32::from_le_bytes(b[OFF_NEXT_FREE..OFF_NEXT_FREE + 4].try_into().unwrap());

        let mut entries = [DirEntry::EMPTY; MAX_FILES];
        let mut used = 0u32;
        for (i, e) in entries.iter_mut().enumerate() {
            let o = OFF_ENTRIES + i * ENTRY_SIZE;
            let mut name = [0u8; NAME_LEN];
            name.copy_from_slice(&b[o..o + NAME_LEN]);
            let size = u32::from_le_bytes(b[o + 24..o + 28].try_into().unwrap());
            let start_block = u32::from_le_bytes(b[o + 28..o + 32].try_into().unwrap());
            let flags = u32::from_le_bytes(b[o + 32..o + 36].try_into().unwrap());
            *e = DirEntry {
                name,
                size,
                start_block,
                flags,
            };
            if e.used() {
                used += 1;
                // Every used file must fit inside the volume, in the data area.
                let end = start_block
                    .checked_add(e.block_count())
                    .ok_or(FsError::InconsistentMetadata)?;
                if start_block < DATA_START_BLOCK || end > total_blocks || end > next_free_block {
                    return Err(FsError::InconsistentMetadata);
                }
                if e.name_str().is_none() {
                    return Err(FsError::InconsistentMetadata);
                }
            }
        }
        if used != file_count
            || next_free_block < DATA_START_BLOCK
            || next_free_block > total_blocks
            || total_blocks < DATA_START_BLOCK
        {
            return Err(FsError::InconsistentMetadata);
        }
        // V0.10: free space is derived from the gaps between live extents,
        // so the allocator must never be handed a directory in which two
        // files claim the same block — a write to one would silently corrupt
        // the other. No well-formed volume (bump- or gap-allocated) contains
        // an overlap, so one is refused at mount rather than trusted.
        for (i, a) in entries.iter().enumerate() {
            if !a.used() {
                continue;
            }
            for b in entries[i + 1..].iter().filter(|b| b.used()) {
                if a.extent().overlaps(&b.extent()) {
                    return Err(FsError::InconsistentMetadata);
                }
            }
        }
        Ok(SuperBlock {
            generation,
            total_blocks,
            file_count,
            next_free_block,
            entries,
        })
    }
}

/// Given the two superblock slots, choose the valid one with the highest
/// generation (crash-recovery rule).
pub fn choose(slot_a: &[u8], slot_b: &[u8]) -> Result<(SuperBlock, u8), FsError> {
    let a = SuperBlock::decode(slot_a).ok();
    let b = SuperBlock::decode(slot_b).ok();
    match (a, b) {
        (Some(a), Some(b)) => {
            if a.generation >= b.generation {
                Ok((a, 0))
            } else {
                Ok((b, 1))
            }
        }
        (Some(a), None) => Ok((a, 0)),
        (None, Some(b)) => Ok((b, 1)),
        (None, None) => Err(FsError::NoValidSuperblock),
    }
}

/// CRC-32 (IEEE 802.3, reflected).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        // CRC-32/ISO-HDLC of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_superblock_round_trips() {
        let sb = SuperBlock::empty(2048);
        let block = sb.encode();
        let back = SuperBlock::decode(&block).unwrap();
        assert_eq!(back.generation, 1);
        assert_eq!(back.file_count, 0);
        assert_eq!(back.total_blocks, 2048);
        assert_eq!(back.next_free_block, DATA_START_BLOCK);
    }

    #[test]
    fn allocate_and_find() {
        let mut sb = SuperBlock::empty(2048);
        let (slot, start) = sb.allocate("hello.txt", 600).unwrap();
        assert_eq!(start, DATA_START_BLOCK);
        assert!(sb.entries[slot].used());
        assert_eq!(sb.file_count, 1);
        assert_eq!(sb.next_free_block, DATA_START_BLOCK + 2); // 600 B -> 2 blocks
        assert_eq!(sb.generation, 2);
        let e = sb.find("hello.txt").unwrap();
        assert_eq!(e.size, 600);
        assert!(sb.find("missing").is_none());
    }

    #[test]
    fn rejects_duplicate_and_oversize_name() {
        let mut sb = SuperBlock::empty(2048);
        sb.allocate("a", 10).unwrap();
        assert_eq!(sb.allocate("a", 10), Err(FsError::Exists));
        assert_eq!(sb.allocate("", 10), Err(FsError::NameEmpty));
        let long = "x".repeat(NAME_LEN + 1);
        assert_eq!(sb.allocate(&long, 10), Err(FsError::NameTooLong));
    }

    #[test]
    fn rejects_no_space() {
        let mut sb = SuperBlock::empty(4); // only blocks 2,3 for data
        assert!(sb.allocate("big", 2000).is_err()); // needs 4 blocks
        sb.allocate("fits", 512).unwrap();
        sb.allocate("fits2", 512).unwrap();
        assert_eq!(sb.allocate("nope", 1), Err(FsError::NoSpace));
    }

    #[test]
    fn decode_rejects_bad_magic_and_crc() {
        let sb = SuperBlock::empty(2048);
        let mut block = sb.encode();
        block[0] = b'X';
        assert_eq!(SuperBlock::decode(&block), Err(FsError::BadMagic));

        let mut block = sb.encode();
        block[100] ^= 0xFF; // corrupt an entry byte -> CRC mismatch
        assert_eq!(SuperBlock::decode(&block), Err(FsError::BadCrc));
    }

    #[test]
    fn decode_rejects_inconsistent_metadata() {
        let mut sb = SuperBlock::empty(2048);
        sb.allocate("a", 100).unwrap();
        // Point the file past the volume, then re-checksum so only the
        // consistency check (not CRC) can catch it.
        let mut block = sb.encode();
        let o = OFF_ENTRIES; // first entry start_block
        block[o + 28..o + 32].copy_from_slice(&9000u32.to_le_bytes());
        let crc = crc32(&block[..OFF_CRC]);
        block[OFF_CRC..OFF_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            SuperBlock::decode(&block),
            Err(FsError::InconsistentMetadata)
        );
    }

    #[test]
    fn choose_picks_highest_valid_generation() {
        let a = SuperBlock::empty(2048).encode();
        let mut sb_b = SuperBlock::empty(2048);
        sb_b.allocate("x", 1).unwrap(); // generation 2
        let b = sb_b.encode();
        let (chosen, slot) = choose(&a, &b).unwrap();
        assert_eq!(chosen.generation, 2);
        assert_eq!(slot, 1);

        // A torn (corrupt) slot B falls back to slot A.
        let mut torn = b;
        torn[100] ^= 0xFF;
        let (chosen, slot) = choose(&a, &torn).unwrap();
        assert_eq!(chosen.generation, 1);
        assert_eq!(slot, 0);

        // Both corrupt -> error.
        let mut torn_a = a;
        torn_a[0] = b'Z';
        assert_eq!(choose(&torn_a, &torn), Err(FsError::NoValidSuperblock));
    }
    #[test]
    fn remove_clears_entry_and_round_trips() {
        let mut sb = SuperBlock::empty(2048);
        sb.allocate("hello.1.pkg", 600).unwrap();
        let (_, ok_start) = sb.allocate("hello.1.ok", 1).unwrap();
        let gen_before = sb.generation;

        sb.remove("hello.1.ok").unwrap();
        assert!(sb.find("hello.1.ok").is_none());
        assert!(sb.find("hello.1.pkg").is_some());
        assert_eq!(sb.file_count, 1);
        assert_eq!(sb.generation, gen_before + 1);
        // V0.10: the high-water mark falls back to the last live extent, so
        // the removed file's block is a gap again (no longer leaked).
        assert_eq!(sb.next_free_block, ok_start);
        assert!(sb.run_is_unreferenced(ok_start, 1));

        // The gapped superblock still encodes/decodes as valid.
        let decoded = SuperBlock::decode(&sb.encode()).unwrap();
        assert_eq!(decoded, sb);

        // Removing a missing file fails cleanly.
        assert_eq!(sb.remove("ghost"), Err(FsError::NotFound));
        // The slot is reusable for a new file.
        sb.allocate("hello.2.pkg", 100).unwrap();
        assert_eq!(sb.file_count, 2);
    }

    #[test]
    fn replace_swaps_the_extent_in_one_generation() {
        let mut sb = SuperBlock::empty(64);
        let (_, first_start) = sb.allocate("notes", 600).unwrap();
        let gen_after_create = sb.generation;
        let (_, second_start) = sb.replace("notes", 100).unwrap();
        // A new extent, so the old contents are untouched until this
        // superblock is committed.
        assert_ne!(first_start, second_start);
        assert_eq!(sb.file_count, 1, "replace must not double-count the file");
        assert_eq!(sb.find("notes").unwrap().size, 100);
        assert_eq!(sb.find("notes").unwrap().start_block, second_start);
        // Two internal steps, but the caller commits once; the generation
        // moving by more than one is fine — what matters is that only ONE
        // encoded superblock ever reaches the device.
        assert!(sb.generation > gen_after_create);
    }

    #[test]
    fn replace_creates_when_absent() {
        let mut sb = SuperBlock::empty(64);
        let (_, start) = sb.replace("fresh", 10).unwrap();
        assert_eq!(sb.file_count, 1);
        assert_eq!(sb.find("fresh").unwrap().start_block, start);
    }

    #[test]
    fn replace_reports_no_space_without_losing_the_old_entry() {
        let mut sb = SuperBlock::empty(8);
        sb.allocate("keep", 512).unwrap();
        // Far more than the remaining blocks.
        let before = sb;
        let err = sb.replace("keep", 100_000).unwrap_err();
        assert_eq!(err, FsError::NoSpace);
        // V0.10: the run is chosen before the entry is touched, so a failed
        // replace leaves even the working copy exactly as it was. (The kernel
        // still works on a copy and discards it on error.)
        assert_eq!(sb, before);
        assert!(sb.find("keep").is_some());
    }

    // ---- V0.10 space reclamation -------------------------------------------

    /// Mirrors `fs_disk`'s transaction protocol: work on a copy of the
    /// committed superblock, pin both on-disk slots, and on commit the old
    /// committed superblock becomes the fallback (the slot a mount would drop
    /// back to). Every step asserts the crash invariant independently of the
    /// allocator: the new data run is unreferenced by BOTH surviving slots.
    struct Volume {
        committed: SuperBlock,
        fallback: SuperBlock,
    }

    impl Volume {
        fn new(total: u32) -> Volume {
            let sb = SuperBlock::empty(total);
            Volume {
                committed: sb,
                fallback: sb,
            }
        }
        fn commit(&mut self, next: SuperBlock) {
            // What reaches the disk must mount again, unchanged.
            assert_eq!(SuperBlock::decode(&next.encode()).unwrap(), next);
            self.fallback = self.committed;
            self.committed = next;
        }
        fn write(&mut self, name: &str, size: u32) -> Result<u32, FsError> {
            let mut next = self.committed;
            let (_, start) =
                next.replace_avoiding(name, size, &[&self.committed, &self.fallback])?;
            let blocks = blocks_for(size);
            assert!(self.committed.run_is_unreferenced(start, blocks));
            assert!(self.fallback.run_is_unreferenced(start, blocks));
            self.commit(next);
            Ok(start)
        }
        fn remove(&mut self, name: &str) -> Result<(), FsError> {
            let mut next = self.committed;
            next.remove(name)?;
            self.commit(next);
            Ok(())
        }
        fn space(&self) -> Space {
            self.committed.space(&[&self.fallback])
        }
    }

    #[test]
    fn allocation_reuses_a_removed_files_extent() {
        let mut v = Volume::new(2 + 8);
        let a = v.write("a", 1024).unwrap(); // blocks 2..4
        v.write("b", 512).unwrap(); // block 4
        v.remove("a").unwrap();
        // Right after the remove, the fallback slot still references `a`, so
        // its blocks are pinned: a crash-recovery mount could still need them.
        let s = v.space();
        assert_eq!((s.used, s.free, s.pinned), (1, 7, 2));
        // One more commit overwrites that slot; now the gap is reusable and
        // first-fit hands out exactly the removed extent.
        v.write("c", 10).unwrap(); // goes past `b` (a's blocks still pinned)
        let d = v.write("d", 1024).unwrap();
        assert_eq!(d, a, "the removed file's extent is reused");
    }

    #[test]
    fn overwrite_never_reuses_its_own_committed_extent_in_the_same_commit() {
        let mut sb = SuperBlock::empty(2 + 4);
        let (_, first) = sb.allocate("f", 512).unwrap();
        // The only free space that fits is AFTER the old extent; the old
        // extent itself (still committed) must not be offered back.
        let committed = sb;
        let (_, second) = sb.replace_avoiding("f", 512, &[&committed]).unwrap();
        assert_ne!(first, second);
        assert!(committed.run_is_unreferenced(second, 1));

        // Even when the old extent is the ONLY space left, it is refused.
        let mut tight = SuperBlock::empty(2 + 1);
        tight.allocate("f", 512).unwrap();
        let committed = tight;
        assert_eq!(
            tight.replace_avoiding("f", 512, &[&committed]),
            Err(FsError::NoSpace)
        );
        assert_eq!(tight, committed, "failed replace leaves the copy intact");
    }

    #[test]
    fn old_extent_is_reusable_once_no_surviving_slot_references_it() {
        // Data area of three blocks, one 1-block file overwritten forever.
        let mut v = Volume::new(2 + 3);
        let s1 = v.write("f", 100).unwrap();
        let s2 = v.write("f", 100).unwrap();
        assert_ne!(s1, s2, "same commit: old extent still committed");
        let s3 = v.write("f", 100).unwrap();
        assert!(
            s3 != s1 && s3 != s2,
            "old extent still held by the fallback slot"
        );
        let s4 = v.write("f", 100).unwrap();
        assert_eq!(s4, s1, "two commits later, nothing references it");
    }

    #[test]
    fn many_overwrite_cycles_on_a_small_disk_never_run_out() {
        // 14 data blocks, the same size as the QEMU reclaim leg's disk.
        let mut v = Volume::new(16);
        v.write("keep.txt", 19).unwrap();
        let mut bump_would_need = 1u32;
        for rev in 1..=1000u32 {
            let size = 1 + (rev * 37) % 512; // always one block
            v.write("churn.txt", size).unwrap();
            bump_would_need += 1;
            let s = v.space();
            assert_eq!(s.used, 2);
            assert!(s.largest_run >= 10);
        }
        // The V0.9 bump allocator never reused anything, so it would have
        // needed a block per write — failing long before this.
        assert!(bump_would_need > 16 - DATA_START_BLOCK);
        assert_eq!(v.committed.find("keep.txt").unwrap().size, 19);
    }

    #[test]
    fn qemu_reclaim_leg_space_report_is_predicted() {
        // The exact sequence the `fs-reclaim` QEMU leg drives through the
        // console on a 16-block disk, so its required serial line is a
        // host-checked prediction rather than a copied observation.
        let mut v = Volume::new(16);
        v.write("keep.txt", "kept-across-reclaim".len() as u32)
            .unwrap();
        for rev in 1..=20 {
            let text = alloc_rev_text(rev);
            v.write("churn.txt", text.len() as u32).unwrap();
        }
        let s = v.space();
        assert_eq!(
            (
                s.data_blocks,
                s.used,
                s.free,
                s.pinned,
                s.largest_run,
                s.files
            ),
            (14, 2, 12, 1, 11, 2)
        );
        // Format is generation 1; each of the 21 writes is one commit.
        assert_eq!(v.committed.generation, 22);
        // The last revision's text is the 25-byte line the leg reads back.
        assert_eq!(alloc_rev_text(20), "rev20-of-20-reclaim-cycle");
        assert_eq!(alloc_rev_text(20).len(), 25);
        // 21 one-block writes into 14 data blocks: bump allocation (V0.9)
        // would have refused the 15th.
        assert!(21 > s.data_blocks);
    }

    fn alloc_rev_text(rev: u32) -> std::string::String {
        std::format!("rev{rev:02}-of-20-reclaim-cycle")
    }

    #[test]
    fn fragmentation_fails_cleanly_with_a_distinct_error() {
        // 6 data blocks: a b c d e f, one block each.
        let mut sb = SuperBlock::empty(2 + 6);
        for n in ["a", "b", "c", "d", "e", "f"] {
            sb.allocate(n, 512).unwrap();
        }
        sb.remove("b").unwrap();
        sb.remove("d").unwrap();
        sb.remove("f").unwrap();
        // Three free blocks in total, but no two are adjacent.
        let s = sb.space(&[]);
        assert_eq!((s.free, s.largest_run), (3, 1));
        let before = sb;
        assert_eq!(sb.allocate("two", 1024), Err(FsError::Fragmented));
        assert_eq!(sb, before, "a refused allocation changes nothing");
        // More than the total free space is plain NoSpace.
        assert_eq!(sb.allocate("four", 4 * 512), Err(FsError::NoSpace));
        // A request that fits a gap still succeeds, first-fit.
        let (_, start) = sb.allocate("one", 512).unwrap();
        assert_eq!(start, DATA_START_BLOCK + 1);
    }

    fn reseal(block: &mut [u8; BLOCK_SIZE]) {
        let crc = crc32(&block[..OFF_CRC]);
        block[OFF_CRC..OFF_CRC + 4].copy_from_slice(&crc.to_le_bytes());
    }

    fn set_entry_start(block: &mut [u8; BLOCK_SIZE], index: usize, start: u32) {
        let o = OFF_ENTRIES + index * ENTRY_SIZE;
        block[o + 28..o + 32].copy_from_slice(&start.to_le_bytes());
        reseal(block);
    }

    #[test]
    fn mount_refuses_overlapping_extents() {
        let mut sb = SuperBlock::empty(64);
        sb.allocate("a", 1024).unwrap(); // blocks 2,3
        sb.allocate("b", 1024).unwrap(); // blocks 4,5
        let mut block = sb.encode();
        // `b` now claims block 3 too — a CRC-valid but hostile directory.
        set_entry_start(&mut block, 1, 3);
        assert_eq!(
            SuperBlock::decode(&block),
            Err(FsError::InconsistentMetadata)
        );
        // Exactly adjacent is fine (that is how every volume is laid out).
        let mut ok = sb.encode();
        set_entry_start(&mut ok, 1, 4);
        assert!(SuperBlock::decode(&ok).is_ok());
        // A 0-byte file occupies no blocks, so it overlaps nothing.
        let mut z = SuperBlock::empty(64);
        z.allocate("a", 1024).unwrap();
        z.allocate("empty", 0).unwrap();
        assert!(SuperBlock::decode(&z.encode()).is_ok());
    }

    #[test]
    fn mount_refuses_extents_past_the_end() {
        let mut sb = SuperBlock::empty(8);
        sb.allocate("a", 512).unwrap();
        let mut block = sb.encode();
        // Start inside the volume but run off its end (blocks 7..9 of 8).
        let o = OFF_ENTRIES;
        block[o + 24..o + 28].copy_from_slice(&1024u32.to_le_bytes());
        block[o + 28..o + 32].copy_from_slice(&7u32.to_le_bytes());
        block[OFF_NEXT_FREE..OFF_NEXT_FREE + 4].copy_from_slice(&8u32.to_le_bytes());
        reseal(&mut block);
        assert_eq!(
            SuperBlock::decode(&block),
            Err(FsError::InconsistentMetadata)
        );
        // A start so large that start + blocks overflows u32 is refused too.
        let mut block = sb.encode();
        set_entry_start(&mut block, 0, u32::MAX);
        assert_eq!(
            SuperBlock::decode(&block),
            Err(FsError::InconsistentMetadata)
        );
        // And an extent inside the superblock slots.
        let mut block = sb.encode();
        set_entry_start(&mut block, 0, 1);
        assert_eq!(
            SuperBlock::decode(&block),
            Err(FsError::InconsistentMetadata)
        );
    }

    #[test]
    fn v09_volume_with_leaked_gaps_mounts_and_its_gaps_are_reused() {
        // A V0.9 bump-allocated image: a (2..4) then b (4) then a removed —
        // next_free_block stays at 5 and blocks 2,3 are leaked.
        let mut sb = SuperBlock::empty(2 + 4);
        sb.allocate("a", 1024).unwrap();
        sb.allocate("b", 512).unwrap();
        let mut block = sb.encode();
        let o = OFF_ENTRIES; // clear entry 0 by hand, keeping next_free = 5
        block[o..o + ENTRY_SIZE].fill(0);
        block[OFF_FILE_COUNT..OFF_FILE_COUNT + 4].copy_from_slice(&1u32.to_le_bytes());
        reseal(&mut block);
        let mut v09 = SuperBlock::decode(&block).unwrap();
        assert_eq!(v09.next_free_block, 5);
        // V0.9 could only offer block 5 (one block). V0.10 sees the gap.
        let s = v09.space(&[]);
        assert_eq!((s.free, s.largest_run), (3, 2));
        let committed = v09;
        let (_, start) = v09.allocate_avoiding("c", 1024, &[&committed]).unwrap();
        assert_eq!(start, DATA_START_BLOCK);
        // The high-water mark stays >= every live end, so the V0.9 decoder's
        // `end <= next_free_block` rule still accepts what V0.10 writes.
        assert!(SuperBlock::decode(&v09.encode()).is_ok());
    }

    /// Randomised model check: a long mixed workload on a small volume, with
    /// the space report cross-checked against a brute-force block bitmap.
    #[test]
    fn random_workload_keeps_every_invariant() {
        let mut v = Volume::new(2 + 40);
        let names = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"];
        let mut seed = 0x9E37_79B9u32;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut writes = 0u32;
        for _ in 0..5000 {
            let name = names[(rand() % names.len() as u32) as usize];
            if rand() % 4 == 0 {
                let _ = v.remove(name);
            } else {
                let size = rand() % (8 * 512);
                match v.write(name, size) {
                    Ok(_) => writes += 1,
                    Err(FsError::NoSpace | FsError::Fragmented) => {}
                    Err(e) => panic!("unexpected {e:?}"),
                }
            }
            // Brute-force accounting.
            let mut used = [false; 42];
            let mut held = [false; 42];
            for e in v.committed.entries.iter().filter(|e| e.used()) {
                for b in e.start_block..e.start_block + e.block_count() {
                    assert!(!used[b as usize], "committed directory overlaps");
                    used[b as usize] = true;
                }
            }
            for e in v.fallback.entries.iter().filter(|e| e.used()) {
                for b in e.start_block..e.start_block + e.block_count() {
                    held[b as usize] = true;
                }
            }
            let s = v.space();
            let used_n = used.iter().filter(|&&u| u).count() as u32;
            let pinned_n = (2..42).filter(|&b| !used[b] && held[b]).count() as u32;
            assert_eq!(s.used, used_n);
            assert_eq!(s.free, 40 - used_n);
            assert_eq!(s.pinned, pinned_n);
        }
        // The workload writes far more than 40 blocks' worth over its life.
        assert!(writes > 1000);
    }
}
