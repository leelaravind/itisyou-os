//! On-disk filesystem driver (V0.4, ADR-0009): ITFS over any `BlockDevice`.
//!
//! Ties the pure on-disk format (`kernel_core::itfs`) to real block I/O:
//! format, mount, create, read, list. Metadata commits are atomic against a
//! crash via double-buffered CRC-protected superblocks. The higher-level
//! logic is device-agnostic, so it works unchanged over NVMe or the RamDisk.

use crate::device::block::{BlockDevice, BlockError, BLOCK_SIZE};
use alloc::vec;
use alloc::vec::Vec;
use kernel_core::itfs::{self, FsError, SuperBlock};

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
        })
    }

    /// Mount an existing ITFS: read both superblock slots and pick the valid
    /// one with the highest generation (crash recovery).
    pub fn mount(dev: &'a dyn BlockDevice) -> Result<FileSystem<'a>, Error> {
        let mut a = [0u8; BLOCK_SIZE];
        let mut b = [0u8; BLOCK_SIZE];
        dev.read_block(0, &mut a)?;
        dev.read_block(1, &mut b)?;
        let (sb, slot) = itfs::choose(&a, &b)?;
        Ok(FileSystem {
            dev,
            sb,
            committed_slot: slot,
        })
    }

    pub fn file_count(&self) -> u32 {
        self.sb.file_count
    }
    pub fn generation(&self) -> u64 {
        self.sb.generation
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
        let (_slot, start) = next.allocate(name, data.len() as u32)?;

        // Write data blocks (zero-padded to a block boundary).
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
        // Flush data before the metadata commit so the file body is on stable
        // media before the superblock references it.
        self.dev.flush()?;

        // Commit the new superblock to the ALTERNATE slot, then flush.
        let commit_slot = 1 - self.committed_slot;
        self.dev.write_block(commit_slot as u64, &next.encode())?;
        self.dev.flush()?;

        self.sb = next;
        self.committed_slot = commit_slot;
        Ok(())
    }

    /// Create or overwrite a file in ONE crash-atomic commit (V0.8).
    ///
    /// The difference from `remove` + `create` is the whole point: those are
    /// two superblock commits, and a crash between them leaves the file gone.
    /// Here the data is written to a fresh extent first, then a single
    /// superblock commit swings the directory entry onto it, so an
    /// interrupted overwrite leaves exactly the old contents or exactly the
    /// new ones — never a truncated file and never no file.
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        // Work on a copy: `replace` clears the old entry in the working
        // superblock, so an allocation failure must not touch the live one.
        let mut next = self.sb;
        let (_slot, start) = next.replace(name, data.len() as u32)?;

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

        let commit_slot = 1 - self.committed_slot;
        self.dev.write_block(commit_slot as u64, &next.encode())?;
        self.dev.flush()?;

        self.sb = next;
        self.committed_slot = commit_slot;
        Ok(())
    }

    /// Remove a file (V0.7): clear its directory entry and commit the new
    /// superblock to the alternate slot — one crash-atomic metadata
    /// transition (used for uninstall, rollback, and update recovery). Data
    /// blocks are not reclaimed (documented ITFS limitation).
    pub fn remove(&mut self, name: &str) -> Result<(), Error> {
        let mut next = self.sb;
        next.remove(name)?;
        let commit_slot = 1 - self.committed_slot;
        self.dev.write_block(commit_slot as u64, &next.encode())?;
        self.dev.flush()?;
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
