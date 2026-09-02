//! Block-device abstraction (V0.3): a narrow read-only interface plus a
//! deterministic RAM-backed device used to prove the abstraction, the read
//! path, and error handling end-to-end. Real controller drivers (NVMe/AHCI)
//! implement the same trait in later work.

use alloc::vec::Vec;

pub const BLOCK_SIZE: usize = 512;

/// Read-only block device errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// LBA beyond the device's block count.
    OutOfRange { lba: u64, blocks: u64 },
    /// Destination buffer is not exactly one block.
    BadBufferLen { len: usize },
    /// Underlying controller/device error.
    DeviceError,
}

/// A read-only block device. `read_block` fills exactly one BLOCK_SIZE buffer.
pub trait BlockDevice {
    fn block_count(&self) -> u64;
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;

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
}

/// A RAM-backed read-only block device (deterministic test/vector store).
pub struct RamDisk {
    data: Vec<u8>,
    blocks: u64,
}

impl RamDisk {
    /// Build a RAM disk of `blocks` blocks; block `i` is filled with the byte
    /// `i as u8 ^ 0x5A` so reads are content-verifiable without a real disk.
    pub fn patterned(blocks: u64) -> RamDisk {
        let mut data = Vec::with_capacity(blocks as usize * BLOCK_SIZE);
        for i in 0..blocks {
            let fill = (i as u8) ^ 0x5A;
            data.extend(core::iter::repeat_n(fill, BLOCK_SIZE));
        }
        RamDisk { data, blocks }
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
        buf.copy_from_slice(&self.data[off..off + BLOCK_SIZE]);
        Ok(())
    }
}
