//! Block-device abstraction (V0.3): a narrow read-only interface plus a
//! deterministic RAM-backed device used to prove the abstraction, the read
//! path, and error handling end-to-end. Real controller drivers (NVMe/AHCI)
//! implement the same trait in later work.

use alloc::vec::Vec;

pub const BLOCK_SIZE: usize = 512;

/// Block device errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// LBA beyond the device's block count.
    OutOfRange { lba: u64, blocks: u64 },
    /// Source/destination buffer is not exactly one block.
    BadBufferLen { len: usize },
    /// Underlying controller/device error.
    DeviceError,
    /// Device does not support writes.
    ReadOnly,
}

/// A block device. `read_block`/`write_block` transfer exactly one
/// BLOCK_SIZE buffer. Writable devices override `write_block`/`flush`.
pub trait BlockDevice {
    fn block_count(&self) -> u64;
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Write exactly one block. Default: read-only.
    fn write_block(&self, _lba: u64, _buf: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::ReadOnly)
    }

    /// Flush volatile write cache to stable media. Default: no-op.
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }

    /// Read `count` consecutive blocks into `buf` (must be count*BLOCK_SIZE).
    fn read_blocks(&self, lba: u64, count: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() != count as usize * BLOCK_SIZE {
            return Err(BlockError::BadBufferLen { len: buf.len() });
        }
        for i in 0..count {
            let off = i as usize * BLOCK_SIZE;
            self.read_block(lba + i, &mut buf[off..off + BLOCK_SIZE])?;
        }
        Ok(())
    }

    /// Write `count` consecutive blocks from `buf` (must be count*BLOCK_SIZE).
    fn write_blocks(&self, lba: u64, count: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() != count as usize * BLOCK_SIZE {
            return Err(BlockError::BadBufferLen { len: buf.len() });
        }
        for i in 0..count {
            let off = i as usize * BLOCK_SIZE;
            self.write_block(lba + i, &buf[off..off + BLOCK_SIZE])?;
        }
        Ok(())
    }
}

/// A RAM-backed read/write block device (deterministic test store). Interior
/// mutability lets it satisfy the `&self` block-device interface; used
/// single-threaded from the selftest path.
pub struct RamDisk {
    data: core::cell::UnsafeCell<Vec<u8>>,
    blocks: u64,
}

// Single CPU, single-threaded test use.
unsafe impl Sync for RamDisk {}

impl RamDisk {
    /// Build a RAM disk of `blocks` blocks; block `i` is filled with the byte
    /// `i as u8 ^ 0x5A` so reads are content-verifiable without a real disk.
    pub fn patterned(blocks: u64) -> RamDisk {
        let mut data = Vec::with_capacity(blocks as usize * BLOCK_SIZE);
        for i in 0..blocks {
            let fill = (i as u8) ^ 0x5A;
            data.extend(core::iter::repeat_n(fill, BLOCK_SIZE));
        }
        RamDisk {
            data: core::cell::UnsafeCell::new(data),
            blocks,
        }
    }

    /// Build a zeroed RAM disk of `blocks` blocks.
    pub fn blank(blocks: u64) -> RamDisk {
        RamDisk {
            data: core::cell::UnsafeCell::new(alloc::vec![0u8; blocks as usize * BLOCK_SIZE]),
            blocks,
        }
    }
}

impl BlockDevice for RamDisk {
    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() != BLOCK_SIZE {
            return Err(BlockError::BadBufferLen { len: buf.len() });
        }
        if lba >= self.blocks {
            return Err(BlockError::OutOfRange {
                lba,
                blocks: self.blocks,
            });
        }
        let off = lba as usize * BLOCK_SIZE;
        // SAFETY: single-threaded; no concurrent &mut alias.
        let data = unsafe { &*self.data.get() };
        buf.copy_from_slice(&data[off..off + BLOCK_SIZE]);
        Ok(())
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() != BLOCK_SIZE {
            return Err(BlockError::BadBufferLen { len: buf.len() });
        }
        if lba >= self.blocks {
            return Err(BlockError::OutOfRange {
                lba,
                blocks: self.blocks,
            });
        }
        let off = lba as usize * BLOCK_SIZE;
        // SAFETY: single-threaded; no concurrent alias.
        let data = unsafe { &mut *self.data.get() };
        data[off..off + BLOCK_SIZE].copy_from_slice(buf);
        Ok(())
    }
}
