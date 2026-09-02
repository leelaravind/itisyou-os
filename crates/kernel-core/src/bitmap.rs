//! Fixed-size frame bitmap — the allocation core of the physical memory
//! manager. Pure logic, host-tested; the kernel wraps it with locking and
//! `x86_64` frame types.
//!
//! One bit per 4 KiB frame. `1` = free, `0` = allocated/unknown. All frames
//! start allocated; the PMM marks usable frames free during init.

/// Errors surfaced by frame bookkeeping (plan §10.3: double-free protection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Frame index beyond the bitmap's capacity.
    OutOfRange,
    /// Freeing a frame that is already free — double free.
    DoubleFree,
    /// Allocating found no free frame.
    Exhausted,
}

/// Frame bitmap over `WORDS * 64` frames.
pub struct FrameBitmap<const WORDS: usize> {
    words: [u64; WORDS],
    free_count: u64,
    /// Cursor for next-fit search — purely an optimization.
    cursor: usize,
}

impl<const WORDS: usize> FrameBitmap<WORDS> {
    pub const CAPACITY: u64 = (WORDS as u64) * 64;

    pub const fn new() -> Self {
        Self {
            words: [0; WORDS],
            free_count: 0,
            cursor: 0,
        }
    }

    pub fn capacity(&self) -> u64 {
        Self::CAPACITY
    }

    pub fn free_count(&self) -> u64 {
        self.free_count
    }

    fn split(index: u64) -> Result<(usize, u64), FrameError> {
        if index >= Self::CAPACITY {
            return Err(FrameError::OutOfRange);
        }
        Ok(((index / 64) as usize, index % 64))
    }

    pub fn is_free(&self, index: u64) -> Result<bool, FrameError> {
        let (word, bit) = Self::split(index)?;
        Ok(self.words[word] & (1 << bit) != 0)
    }

    /// Mark a frame free during init (idempotent-safe is NOT a goal: marking
    /// an already-free frame free is a bookkeeping bug and reported as such).
    pub fn mark_free(&mut self, index: u64) -> Result<(), FrameError> {
        let (word, bit) = Self::split(index)?;
        if self.words[word] & (1 << bit) != 0 {
            return Err(FrameError::DoubleFree);
        }
        self.words[word] |= 1 << bit;
        self.free_count += 1;
        Ok(())
    }

    /// Allocate an arbitrary free frame (next-fit), returning its index.
    pub fn alloc(&mut self) -> Result<u64, FrameError> {
        if self.free_count == 0 {
            return Err(FrameError::Exhausted);
        }
        for offset in 0..WORDS {
            let word_idx = (self.cursor + offset) % WORDS;
            let word = self.words[word_idx];
            if word != 0 {
                let bit = word.trailing_zeros() as u64;
                self.words[word_idx] &= !(1 << bit);
                self.free_count -= 1;
                self.cursor = word_idx;
                return Ok((word_idx as u64) * 64 + bit);
            }
        }
        // free_count said there was a frame but none found: invariant broken.
        debug_assert!(false, "free_count/bitmap desync");
        Err(FrameError::Exhausted)
    }

    /// Free a previously allocated frame.
    pub fn free(&mut self, index: u64) -> Result<(), FrameError> {
        let (word, bit) = Self::split(index)?;
        if self.words[word] & (1 << bit) != 0 {
            return Err(FrameError::DoubleFree);
        }
        self.words[word] |= 1 << bit;
        self.free_count += 1;
        Ok(())
    }
}

impl<const WORDS: usize> Default for FrameBitmap<WORDS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type SmallBitmap = FrameBitmap<4>; // 256 frames

    #[test]
    fn starts_fully_allocated() {
        let mut bm = SmallBitmap::new();
        assert_eq!(bm.free_count(), 0);
        assert_eq!(bm.alloc(), Err(FrameError::Exhausted));
    }

    #[test]
    fn alloc_free_roundtrip() {
        let mut bm = SmallBitmap::new();
        bm.mark_free(10).unwrap();
        bm.mark_free(11).unwrap();
        assert_eq!(bm.free_count(), 2);

        let a = bm.alloc().unwrap();
        let b = bm.alloc().unwrap();
        assert_ne!(a, b);
        assert!(a == 10 || a == 11);
        assert_eq!(bm.free_count(), 0);
        assert_eq!(bm.alloc(), Err(FrameError::Exhausted));

        bm.free(a).unwrap();
        assert_eq!(bm.free_count(), 1);
        assert_eq!(bm.alloc().unwrap(), a);
    }

    #[test]
    fn double_free_is_detected() {
        let mut bm = SmallBitmap::new();
        bm.mark_free(5).unwrap();
        let f = bm.alloc().unwrap();
        bm.free(f).unwrap();
        assert_eq!(bm.free(f), Err(FrameError::DoubleFree));
    }

    #[test]
    fn double_mark_free_is_detected() {
        let mut bm = SmallBitmap::new();
        bm.mark_free(5).unwrap();
        assert_eq!(bm.mark_free(5), Err(FrameError::DoubleFree));
    }

    #[test]
    fn out_of_range_rejected() {
        let mut bm = SmallBitmap::new();
        assert_eq!(bm.mark_free(256), Err(FrameError::OutOfRange));
        assert_eq!(bm.free(9999), Err(FrameError::OutOfRange));
        assert_eq!(bm.is_free(256), Err(FrameError::OutOfRange));
    }

    #[test]
    fn exhaustion_and_full_reuse() {
        let mut bm = SmallBitmap::new();
        for i in 0..256 {
            bm.mark_free(i).unwrap();
        }
        let mut got = std::collections::HashSet::new();
        for _ in 0..256 {
            assert!(got.insert(bm.alloc().unwrap()));
        }
        assert_eq!(bm.alloc(), Err(FrameError::Exhausted));
        for i in got {
            bm.free(i).unwrap();
        }
        assert_eq!(bm.free_count(), 256);
    }
}
