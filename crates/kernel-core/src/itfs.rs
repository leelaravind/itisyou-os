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
        self.size
            .div_ceil(BLOCK_SIZE as u32)
            .max(if self.size == 0 { 0 } else { 1 })
    }
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

    /// Reserve a contiguous run for a new file and return its entry index +
    /// start block. Does not modify data blocks (the caller writes them).
    pub fn allocate(&mut self, name: &str, size: u32) -> Result<(usize, u32), FsError> {
        if name.is_empty() {
            return Err(FsError::NameEmpty);
        }
        if name.len() > NAME_LEN {
            return Err(FsError::NameTooLong);
        }
        if self.find(name).is_some() {
            return Err(FsError::Exists);
        }
        let blocks = size
            .div_ceil(BLOCK_SIZE as u32)
            .max(if size == 0 { 0 } else { 1 });
        let start = self.next_free_block;
        let end = start.checked_add(blocks).ok_or(FsError::NoSpace)?;
        if end > self.total_blocks {
            return Err(FsError::NoSpace);
        }
        let slot = self
            .entries
            .iter()
            .position(|e| !e.used())
            .ok_or(FsError::NoSpace)?;
        let mut name_buf = [0u8; NAME_LEN];
        name_buf[..name.len()].copy_from_slice(name.as_bytes());
        self.entries[slot] = DirEntry {
            name: name_buf,
            size,
            start_block: start,
            flags: FLAG_USED,
        };
        self.next_free_block = end;
        self.file_count += 1;
        self.generation += 1;
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
}
