//! Strict ELF64 executable parser (V0.2 userspace loader front-end).
//!
//! Structural validation only — address-window and W^X policy checks live in
//! the kernel loader. Pure and allocation-free so malformed-input behavior is
//! fully unit-tested on the host (requirement: malformed ELF rejection).

/// Parse failure. Every variant is a hard rejection — no best-effort loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooShort,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    BadVersion,
    /// Only static ET_EXEC executables are supported in V0.2 (no PIE).
    NotExecutable,
    WrongMachine,
    BadHeaderLayout,
    ProgramHeadersOutOfFile,
    /// filesz > memsz, or ranges overflow u64.
    SegmentSizeInvalid {
        index: usize,
    },
    /// Segment file data extends past the end of the image.
    SegmentDataOutOfFile {
        index: usize,
    },
    /// p_align not a power of two, or vaddr/offset misaligned to it.
    SegmentAlignmentInvalid {
        index: usize,
    },
    /// Entry point is zero or not covered by any executable PT_LOAD segment.
    EntryNotExecutable,
    NoLoadSegments,
}

/// One PT_LOAD segment (borrowing file data from the image).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSegment<'a> {
    pub vaddr: u64,
    pub mem_size: u64,
    pub data: &'a [u8],
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

/// A validated ELF64 executable image.
#[derive(Debug, Clone, Copy)]
pub struct ElfImage<'a> {
    pub entry: u64,
    bytes: &'a [u8],
    phoff: usize,
    phentsize: usize,
    phnum: usize,
}

const EHDR_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;

fn u16le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn u64le(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

/// Validate an ELF64 image completely (header + every program header).
pub fn parse(bytes: &[u8]) -> Result<ElfImage<'_>, ElfError> {
    if bytes.len() < EHDR_SIZE {
        return Err(ElfError::TooShort);
    }
    if &bytes[0..4] != b"\x7fELF" {
        return Err(ElfError::BadMagic);
    }
    if bytes[4] != 2 {
        return Err(ElfError::Not64Bit);
    }
    if bytes[5] != 1 {
        return Err(ElfError::NotLittleEndian);
    }
    if bytes[6] != 1 {
        return Err(ElfError::BadVersion);
    }
    if u16le(bytes, 16) != ET_EXEC {
        return Err(ElfError::NotExecutable);
    }
    if u16le(bytes, 18) != EM_X86_64 {
        return Err(ElfError::WrongMachine);
    }

    let entry = u64le(bytes, 24);
    let phoff = u64le(bytes, 32);
    let phentsize = u16le(bytes, 54) as usize;
    let phnum = u16le(bytes, 56) as usize;

    if phentsize < PHDR_SIZE || phnum == 0 || phnum > 64 {
        return Err(ElfError::BadHeaderLayout);
    }
    let table_size = (phentsize as u64).checked_mul(phnum as u64);
    let table_end = table_size.and_then(|s| phoff.checked_add(s));
    match table_end {
        Some(end) if end <= bytes.len() as u64 => {}
        _ => return Err(ElfError::ProgramHeadersOutOfFile),
    }
    let phoff = phoff as usize;

    let image = ElfImage {
        entry,
        bytes,
        phoff,
        phentsize,
        phnum,
    };

    // Validate every PT_LOAD now so `load_segments` cannot fail later.
    let mut load_count = 0usize;
    let mut entry_in_exec = false;
    for (index, ph) in image.raw_program_headers().enumerate() {
        if u32le(ph, 0) != PT_LOAD {
            continue;
        }
        load_count += 1;
        let flags = u32le(ph, 4);
        let offset = u64le(ph, 8);
        let vaddr = u64le(ph, 16);
        let filesz = u64le(ph, 32);
        let memsz = u64le(ph, 40);
        let align = u64le(ph, 48);

        if filesz > memsz
            || vaddr.checked_add(memsz).is_none()
            || offset.checked_add(filesz).is_none()
        {
            return Err(ElfError::SegmentSizeInvalid { index });
        }
        if offset + filesz > bytes.len() as u64 {
            return Err(ElfError::SegmentDataOutOfFile { index });
        }
        if align > 1 {
            if !align.is_power_of_two() {
                return Err(ElfError::SegmentAlignmentInvalid { index });
            }
            if vaddr % align != offset % align {
                return Err(ElfError::SegmentAlignmentInvalid { index });
            }
        }
        if flags & PF_X != 0 && entry >= vaddr && entry < vaddr + memsz {
            entry_in_exec = true;
        }
    }
    if load_count == 0 {
        return Err(ElfError::NoLoadSegments);
    }
    if entry == 0 || !entry_in_exec {
        return Err(ElfError::EntryNotExecutable);
    }
    Ok(image)
}

impl<'a> ElfImage<'a> {
    fn raw_program_headers(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        (0..self.phnum).map(move |i| {
            let start = self.phoff + i * self.phentsize;
            &self.bytes[start..start + PHDR_SIZE]
        })
    }

    /// Iterate validated PT_LOAD segments.
    pub fn load_segments(&self) -> impl Iterator<Item = LoadSegment<'a>> + '_ {
        self.raw_program_headers().filter_map(move |ph| {
            if u32le(ph, 0) != PT_LOAD {
                return None;
            }
            let flags = u32le(ph, 4);
            let offset = u64le(ph, 8) as usize;
            let vaddr = u64le(ph, 16);
            let filesz = u64le(ph, 32) as usize;
            let memsz = u64le(ph, 40);
            Some(LoadSegment {
                vaddr,
                mem_size: memsz,
                data: &self.bytes[offset..offset + filesz],
                readable: flags & PF_R != 0,
                writable: flags & PF_W != 0,
                executable: flags & PF_X != 0,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid ELF64 EXEC image: one RX code segment at
    /// 0x400000 containing 16 bytes, entry at 0x400000.
    fn minimal_elf() -> Vec<u8> {
        let mut e = vec![0u8; EHDR_SIZE + PHDR_SIZE + 16];
        e[0..4].copy_from_slice(b"\x7fELF");
        e[4] = 2; // 64-bit
        e[5] = 1; // little-endian
        e[6] = 1; // version
        e[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        e[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        e[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
        e[24..32].copy_from_slice(&0x400000u64.to_le_bytes()); // entry
        e[32..40].copy_from_slice(&(EHDR_SIZE as u64).to_le_bytes()); // phoff
        e[52..54].copy_from_slice(&(EHDR_SIZE as u16).to_le_bytes()); // ehsize
        e[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes()); // phentsize
        e[56..58].copy_from_slice(&1u16.to_le_bytes()); // phnum

        let ph = EHDR_SIZE;
        e[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        e[ph + 4..ph + 8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
        let data_off = (EHDR_SIZE + PHDR_SIZE) as u64;
        e[ph + 8..ph + 16].copy_from_slice(&data_off.to_le_bytes()); // offset
        e[ph + 16..ph + 24].copy_from_slice(&0x400000u64.to_le_bytes()); // vaddr
        e[ph + 32..ph + 40].copy_from_slice(&16u64.to_le_bytes()); // filesz
        e[ph + 40..ph + 48].copy_from_slice(&16u64.to_le_bytes()); // memsz
        e[ph + 48..ph + 56].copy_from_slice(&0u64.to_le_bytes()); // align
        e
    }

    #[test]
    fn accepts_minimal_valid_executable() {
        let bytes = minimal_elf();
        let image = parse(&bytes).unwrap();
        assert_eq!(image.entry, 0x400000);
        let segs: Vec<_> = image.load_segments().collect();
        assert_eq!(segs.len(), 1);
        assert!(segs[0].executable && segs[0].readable && !segs[0].writable);
        assert_eq!(segs[0].data.len(), 16);
        assert_eq!(segs[0].mem_size, 16);
    }

    #[test]
    fn rejects_truncated_and_bad_magic() {
        assert!(matches!(parse(&[]), Err(ElfError::TooShort)));
        let mut bytes = minimal_elf();
        bytes[0] = b'X';
        assert!(matches!(parse(&bytes), Err(ElfError::BadMagic)));
    }

    #[test]
    fn rejects_wrong_class_endian_type_machine() {
        let mut b = minimal_elf();
        b[4] = 1;
        assert!(matches!(parse(&b), Err(ElfError::Not64Bit)));
        let mut b = minimal_elf();
        b[5] = 2;
        assert!(matches!(parse(&b), Err(ElfError::NotLittleEndian)));
        let mut b = minimal_elf();
        b[16] = 3; // ET_DYN (PIE) — rejected in V0.2
        assert!(matches!(parse(&b), Err(ElfError::NotExecutable)));
        let mut b = minimal_elf();
        b[18] = 40; // EM_ARM
        assert!(matches!(parse(&b), Err(ElfError::WrongMachine)));
    }

    #[test]
    fn rejects_program_headers_out_of_file() {
        let mut b = minimal_elf();
        b[32..40].copy_from_slice(&(u32::MAX as u64).to_le_bytes());
        assert!(matches!(parse(&b), Err(ElfError::ProgramHeadersOutOfFile)));
    }

    #[test]
    fn rejects_filesz_larger_than_memsz() {
        let mut b = minimal_elf();
        let ph = EHDR_SIZE;
        b[ph + 40..ph + 48].copy_from_slice(&8u64.to_le_bytes()); // memsz < filesz
        assert!(matches!(
            parse(&b),
            Err(ElfError::SegmentSizeInvalid { index: 0 })
        ));
    }

    #[test]
    fn rejects_segment_data_past_eof() {
        let mut b = minimal_elf();
        let ph = EHDR_SIZE;
        b[ph + 32..ph + 40].copy_from_slice(&4096u64.to_le_bytes());
        b[ph + 40..ph + 48].copy_from_slice(&4096u64.to_le_bytes());
        assert!(matches!(
            parse(&b),
            Err(ElfError::SegmentDataOutOfFile { index: 0 })
        ));
    }

    #[test]
    fn rejects_overflowing_vaddr() {
        let mut b = minimal_elf();
        let ph = EHDR_SIZE;
        b[ph + 16..ph + 24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            parse(&b),
            Err(ElfError::SegmentSizeInvalid { index: 0 })
        ));
    }

    #[test]
    fn rejects_bad_alignment() {
        let mut b = minimal_elf();
        let ph = EHDR_SIZE;
        b[ph + 48..ph + 56].copy_from_slice(&3u64.to_le_bytes()); // not pow2
        assert!(matches!(
            parse(&b),
            Err(ElfError::SegmentAlignmentInvalid { index: 0 })
        ));
    }

    #[test]
    fn rejects_entry_outside_executable_segment() {
        let mut b = minimal_elf();
        b[24..32].copy_from_slice(&0x500000u64.to_le_bytes());
        assert!(matches!(parse(&b), Err(ElfError::EntryNotExecutable)));
        let mut b = minimal_elf();
        b[24..32].copy_from_slice(&0u64.to_le_bytes());
        assert!(matches!(parse(&b), Err(ElfError::EntryNotExecutable)));
    }

    #[test]
    fn rejects_no_load_segments() {
        let mut b = minimal_elf();
        let ph = EHDR_SIZE;
        b[ph..ph + 4].copy_from_slice(&2u32.to_le_bytes()); // PT_DYNAMIC
        assert!(matches!(parse(&b), Err(ElfError::NoLoadSegments)));
    }

    #[test]
    fn rejects_zero_phnum() {
        let mut b = minimal_elf();
        b[56..58].copy_from_slice(&0u16.to_le_bytes());
        assert!(matches!(parse(&b), Err(ElfError::BadHeaderLayout)));
    }
}
