//! On-disk filesystem driver (V0.4, ADR-0009): ITFS over any `BlockDevice`.
//!
//! Ties the pure on-disk format (`kernel_core::itfs`) to real block I/O:
//! format, mount, create, read, list. Metadata commits are atomic against a
//! crash via double-buffered CRC-protected superblocks. The higher-level
//! logic is device-agnostic, so it works unchanged over NVMe or the RamDisk.
//!
//! V0.10 space reclamation: freed extents are reused (first-fit over the gaps
//! between live extents). Every transaction pins BOTH superblock slots — the
//! committed one and the one a mount would fall back to — and new data is
//! only ever written to blocks neither references; that is re-checked here
//! before the first data block is written, independently of the allocator.

use crate::device::block::{BlockDevice, BlockError, BLOCK_SIZE};
use alloc::vec;
use alloc::vec::Vec;
use kernel_core::itfs::{self, FsError, Space, SuperBlock};

#[derive(Debug)]
pub enum Error {
    Block(BlockError),
    Fs(FsError),
}

impl From<BlockError> for Error {
    fn from(e: BlockError) -> Self {
        Error::Block(e)
    }
}
impl From<FsError> for Error {
    fn from(e: FsError) -> Self {
        Error::Fs(e)
    }
}

/// A mounted ITFS filesystem over a block device.
pub struct FileSystem<'a> {
    dev: &'a dyn BlockDevice,
    sb: SuperBlock,
    /// Slot holding the current committed superblock (0 or 1); the next
    /// commit writes to the other slot.
    committed_slot: u8,
    /// The superblock in the OTHER slot, if it decodes — the generation a
    /// mount falls back to if the committed slot is ever lost. Its extents
    /// stay pinned until the next commit overwrites that slot, so whichever
    /// valid superblock a mount picks references intact data.
    fallback: Option<SuperBlock>,
}

/// Read both superblock slots: (committed, its slot, the other slot if valid).
fn read_slots(dev: &dyn BlockDevice) -> Result<(SuperBlock, u8, Option<SuperBlock>), Error> {
    let mut a = [0u8; BLOCK_SIZE];
    let mut b = [0u8; BLOCK_SIZE];
    dev.read_block(0, &mut a)?;
    dev.read_block(1, &mut b)?;
    let (sb, slot) = itfs::choose(&a, &b)?;
    let other = if slot == 0 { &b } else { &a };
    Ok((sb, slot, SuperBlock::decode(other).ok()))
}

impl<'a> FileSystem<'a> {
    /// Format `dev` as an empty ITFS and return a mounted handle. Writes the
    /// empty superblock to BOTH slots so either is independently valid.
    pub fn format(dev: &'a dyn BlockDevice) -> Result<FileSystem<'a>, Error> {
        let total = u32::try_from(dev.block_count().min(u32::MAX as u64)).unwrap_or(u32::MAX);
        let sb = SuperBlock::empty(total);
        let block = sb.encode();
        dev.write_block(0, &block)?;
        dev.write_block(1, &block)?;
        dev.flush()?;
        Ok(FileSystem {
            dev,
            sb,
            committed_slot: 0,
            fallback: Some(sb),
        })
    }

    /// Mount an existing ITFS: read both superblock slots and pick the valid
    /// one with the highest generation (crash recovery).
    pub fn mount(dev: &'a dyn BlockDevice) -> Result<FileSystem<'a>, Error> {
        let (sb, committed_slot, fallback) = read_slots(dev)?;
        Ok(FileSystem {
            dev,
            sb,
            committed_slot,
            fallback,
        })
    }

    pub fn file_count(&self) -> u32 {
        self.sb.file_count
    }
    pub fn generation(&self) -> u64 {
        self.sb.generation
    }

    /// Space accounting over the committed directory, with the fallback
    /// slot's still-pinned blocks reported separately.
    pub fn space(&self) -> Space {
        let pins = self.pins();
        self.sb.space(&pins[1..])
    }

    /// The directories no new data may touch: committed + fallback.
    fn pins(&self) -> [&SuperBlock; 2] {
        [&self.sb, self.fallback.as_ref().unwrap_or(&self.sb)]
    }

    /// List file names.
    pub fn list(&self) -> Vec<&str> {
        self.sb
            .entries
            .iter()
            .filter(|e| e.used())
            .filter_map(|e| e.name_str())
            .collect()
    }

    /// Create a file with the given contents. Writes data blocks first, then
    /// commits the new superblock to the alternate slot (atomic against a
    /// crash: a torn commit leaves the old superblock intact).
    pub fn create(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        // Work on a copy so a failure leaves the live superblock untouched.
        let mut next = self.sb;
        let (_slot, start) = next.allocate_avoiding(name, data.len() as u32, &self.pins())?;
        self.write_extent(start, data)?;
        self.commit(next)
    }

    /// Create or overwrite a file in ONE crash-atomic commit (V0.8).
    ///
    /// The difference from `remove` + `create` is the whole point: those are
    /// two superblock commits, and a crash between them leaves the file gone.
    /// Here the data is written to a fresh extent first, then a single
    /// superblock commit swings the directory entry onto it, so an
    /// interrupted overwrite leaves exactly the old contents or exactly the
    /// new ones — never a truncated file and never no file. The fresh extent
    /// is never the file's own old one (still committed) nor any extent the
    /// fallback slot references (V0.10).
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        // Work on a copy: an allocation failure must not touch the live one.
        let mut next = self.sb;
        let (_slot, start) = next.replace_avoiding(name, data.len() as u32, &self.pins())?;
        self.write_extent(start, data)?;
        self.commit(next)
    }

    /// Remove a file (V0.7): clear its directory entry and commit the new
    /// superblock to the alternate slot — one crash-atomic metadata
    /// transition (used for uninstall, rollback, and update recovery). The
    /// freed blocks become reusable once no surviving slot references them.
    pub fn remove(&mut self, name: &str) -> Result<(), Error> {
        let mut next = self.sb;
        next.remove(name)?;
        self.commit(next)
    }

    /// Write `data` (zero-padded to a block boundary) at `start`, then flush.
    ///
    /// Refuses outright — before a single block is written — if the run is
    /// referenced by either surviving superblock. The allocator already
    /// guarantees this; the check makes the crash-safety invariant hold even
    /// if a future allocator change got it wrong.
    fn write_extent(&self, start: u32, data: &[u8]) -> Result<(), Error> {
        let blocks = itfs::blocks_for(data.len() as u32);
        if !self
            .pins()
            .iter()
            .all(|sb| sb.run_is_unreferenced(start, blocks))
        {
            return Err(Error::Fs(FsError::InconsistentMetadata));
        }
        let mut lba = start as u64;
        let mut off = 0usize;
        while off < data.len() {
            let mut block = [0u8; BLOCK_SIZE];
            let n = core::cmp::min(BLOCK_SIZE, data.len() - off);
            block[..n].copy_from_slice(&data[off..off + n]);
            self.dev.write_block(lba, &block)?;
            lba += 1;
            off += BLOCK_SIZE;
        }
        // The body must be on stable media before any superblock references
        // it, or a crash could commit a directory entry pointing at garbage.
        self.dev.flush()?;
        Ok(())
    }

    /// Commit `next` to the ALTERNATE slot, then flush. On success the old
    /// committed superblock becomes the fallback (it is what the other slot
    /// now holds).
    fn commit(&mut self, next: SuperBlock) -> Result<(), Error> {
        let commit_slot = 1 - self.committed_slot;
        let written = self
            .dev
            .write_block(commit_slot as u64, &next.encode())
            .and_then(|()| self.dev.flush());
        if let Err(e) = written {
            // The alternate slot now holds the old superblock, a torn block,
            // or `next` itself — unknown. Re-read what a mount would see so a
            // later transaction on this handle pins whatever is really there
            // instead of trusting memory. Best effort: if the device cannot
            // even be read, the original error is still what is reported.
            if let Ok((sb, slot, fallback)) = read_slots(self.dev) {
                self.sb = sb;
                self.committed_slot = slot;
                self.fallback = fallback;
            }
            return Err(e.into());
        }
        self.fallback = Some(self.sb);
        self.sb = next;
        self.committed_slot = commit_slot;
        Ok(())
    }

    /// Read a file's full contents.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, Error> {
        let entry = *self.sb.find(name).ok_or(FsError::NotFound)?;
        let mut out = vec![0u8; entry.size as usize];
        let mut lba = entry.start_block as u64;
        let mut off = 0usize;
        let mut block = [0u8; BLOCK_SIZE];
        while off < out.len() {
            self.dev.read_block(lba, &mut block)?;
            let n = core::cmp::min(BLOCK_SIZE, out.len() - off);
            out[off..off + n].copy_from_slice(&block[..n]);
            lba += 1;
            off += BLOCK_SIZE;
        }
        Ok(out)
    }
}
