//! Make the UEFI loader embedded in a disk image bit-for-bit reproducible.
//!
//! The UEFI image carries the bootloader's `BOOTX64.EFI`, a PE32+ file that
//! the linker stamps with its link time (COFF header and every debug directory
//! entry) and a CodeView GUID derived from it. Two clean builds of the same
//! commit therefore differed in exactly those ten bytes (observed 2026-09-13
//! from two fresh clones, after the GPT GUIDs were already normalized). They
//! carry no meaning for booting, so they are set to fixed or content-derived
//! values, the same way `gpt_normalize` treats the partition table.
//!
//! The PE is found where a FAT file must start — on a 512-byte boundary — and
//! is only rewritten after its headers, section table and CodeView record all
//! check out; anything unexpected is an error, not a guess, because a silently
//! half-normalized image would look reproducible until the next rebuild.

const SECTOR: usize = 512;
const MACHINE_X86_64: u16 = 0x8664;
const PE32_PLUS: u16 = 0x20B;
const DEBUG_DIR_INDEX: usize = 6;
const DEBUG_ENTRY_LEN: usize = 28;
const CODEVIEW: u32 = 2;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Where the fields to normalize live, as offsets from the PE's `MZ`.
#[derive(Debug, Default)]
struct Fields {
    timestamps: Vec<usize>,
    checksum: usize,
    guids: Vec<usize>,
    /// End of the last section's raw data: the extent of the file.
    end: usize,
}

/// Parse a PE32+ at the start of `pe`, returning the offsets to rewrite, or
/// `None` if this is not an x86_64 PE32+ at all. `Err` means it IS one but
/// its structure cannot be trusted.
fn locate(pe: &[u8]) -> Result<Option<Fields>, String> {
    if pe.get(..2) != Some(b"MZ") {
        return Ok(None);
    }
    let Some(lfanew) = u32_at(pe, 0x3C).map(|v| v as usize) else {
        return Ok(None);
    };
    if lfanew > 4096 || pe.get(lfanew..lfanew + 4) != Some(b"PE\0\0") {
        return Ok(None);
    }
    let coff = lfanew + 4;
    if u16_at(pe, coff) != Some(MACHINE_X86_64) {
        return Ok(None);
    }
    let sections = u16_at(pe, coff + 2).ok_or("truncated COFF header")? as usize;
    let opt_size = u16_at(pe, coff + 16).ok_or("truncated COFF header")? as usize;
    let opt = coff + 20;
    if u16_at(pe, opt) != Some(PE32_PLUS) {
        return Err("x86_64 PE without a PE32+ optional header".into());
    }
    let mut f = Fields {
        timestamps: vec![coff + 4],
        checksum: opt + 64,
        ..Fields::default()
    };
    // Section table: needed to turn the debug directory's RVA into a file
    // offset, and to know where the file ends.
    let table = opt + opt_size;
    let mut secs = Vec::with_capacity(sections);
    for i in 0..sections {
        let s = table + i * 40;
        let vsize = u32_at(pe, s + 8).ok_or("truncated section table")? as usize;
        let va = u32_at(pe, s + 12).ok_or("truncated section table")? as usize;
        let raw_size = u32_at(pe, s + 16).ok_or("truncated section table")? as usize;
        let raw_ptr = u32_at(pe, s + 20).ok_or("truncated section table")? as usize;
        f.end = f.end.max(raw_ptr + raw_size);
        secs.push((va, vsize.max(raw_size), raw_ptr));
    }
    if f.end > pe.len() {
        return Err(format!(
            "sections extend past the image ({} > {})",
            f.end,
            pe.len()
        ));
    }
    let rva_to_off = |rva: usize| {
        secs.iter()
            .find(|&&(va, size, _)| rva >= va && rva < va + size)
            .map(|&(va, _, raw)| raw + (rva - va))
    };
    let dirs = u32_at(pe, opt + 108).ok_or("truncated optional header")? as usize;
    if dirs > DEBUG_DIR_INDEX {
        let dd = opt + 112 + DEBUG_DIR_INDEX * 8;
        let rva = u32_at(pe, dd).ok_or("truncated data directory")? as usize;
        let size = u32_at(pe, dd + 4).ok_or("truncated data directory")? as usize;
        if rva != 0 && size != 0 {
            if !size.is_multiple_of(DEBUG_ENTRY_LEN) {
                return Err(format!("debug directory size {size} is not whole entries"));
            }
            let at = rva_to_off(rva).ok_or("debug directory outside every section")?;
            for i in 0..size / DEBUG_ENTRY_LEN {
                let e = at + i * DEBUG_ENTRY_LEN;
                if e + DEBUG_ENTRY_LEN > pe.len() {
                    return Err("debug directory entry past the image".into());
                }
                f.timestamps.push(e + 4);
                if u32_at(pe, e + 12) == Some(CODEVIEW) {
                    let data = u32_at(pe, e + 24).ok_or("truncated debug entry")? as usize;
                    if pe.get(data..data + 4) != Some(b"RSDS") || data + 20 > pe.len() {
                        return Err("CodeView entry does not point at an RSDS record".into());
                    }
                    f.guids.push(data + 4);
                }
            }
        }
    }
    Ok(Some(f))
}

/// Normalize every x86_64 PE32+ that starts on a sector boundary of `image`.
/// Returns how many were rewritten.
pub fn normalize_embedded(
    image: &mut [u8],
    digest: impl Fn(&[u8]) -> [u8; 32],
) -> Result<usize, String> {
    let mut count = 0;
    let mut at = 0;
    while at + SECTOR <= image.len() {
        if let Some(f) = locate(&image[at..])? {
            let pe = &mut image[at..at + f.end];
            for &t in &f.timestamps {
                pe[t..t + 4].fill(0);
            }
            pe[f.checksum..f.checksum + 4].fill(0);
            for &g in &f.guids {
                pe[g..g + 16].fill(0);
            }
            let seed = digest(pe);
            for &g in &f.guids {
                pe[g..g + 16].copy_from_slice(&seed[..16]);
            }
            count += 1;
            at += f.end.div_ceil(SECTOR) * SECTOR;
        } else {
            at += SECTOR;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(data: &[u8]) -> [u8; 32] {
        crate::sha256(data)
    }

    /// A minimal PE32+: one section holding a debug directory with a CodeView
    /// entry, `stamp` standing in for the link time and GUID the linker writes.
    fn synthetic_pe(stamp: u8) -> Vec<u8> {
        let mut pe = vec![0u8; 0x400];
        pe[..2].copy_from_slice(b"MZ");
        let lfanew = 0x80usize;
        pe[0x3C..0x40].copy_from_slice(&(lfanew as u32).to_le_bytes());
        pe[lfanew..lfanew + 4].copy_from_slice(b"PE\0\0");
        let coff = lfanew + 4;
        pe[coff..coff + 2].copy_from_slice(&MACHINE_X86_64.to_le_bytes());
        pe[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // sections
        pe[coff + 4..coff + 8].fill(stamp); // TimeDateStamp
        let opt_size = 112 + 16 * 8;
        pe[coff + 16..coff + 18].copy_from_slice(&(opt_size as u16).to_le_bytes());
        let opt = coff + 20;
        pe[opt..opt + 2].copy_from_slice(&PE32_PLUS.to_le_bytes());
        pe[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        // Debug directory: RVA 0x1100 (inside the section), one entry.
        let dd = opt + 112 + DEBUG_DIR_INDEX * 8;
        pe[dd..dd + 4].copy_from_slice(&0x1100u32.to_le_bytes());
        pe[dd + 4..dd + 8].copy_from_slice(&(DEBUG_ENTRY_LEN as u32).to_le_bytes());
        // One section: VA 0x1000, raw at 0x200, 0x200 bytes.
        let s = opt + opt_size;
        pe[s..s + 8].copy_from_slice(b".rdata\0\0");
        pe[s + 8..s + 12].copy_from_slice(&0x200u32.to_le_bytes());
        pe[s + 12..s + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        pe[s + 16..s + 20].copy_from_slice(&0x200u32.to_le_bytes());
        pe[s + 20..s + 24].copy_from_slice(&0x200u32.to_le_bytes());
        // The debug entry sits at file offset 0x200 + 0x100.
        let e = 0x300;
        pe[e + 4..e + 8].fill(stamp);
        pe[e + 12..e + 16].copy_from_slice(&CODEVIEW.to_le_bytes());
        pe[e + 24..e + 28].copy_from_slice(&0x340u32.to_le_bytes());
        pe[0x340..0x344].copy_from_slice(b"RSDS");
        pe[0x344..0x354].fill(stamp.wrapping_mul(3));
        pe[0x358..0x360].copy_from_slice(b"boot.pdb");
        pe
    }

    fn in_image(pe: &[u8]) -> Vec<u8> {
        let mut img = vec![0u8; 8 * SECTOR];
        img[2 * SECTOR..2 * SECTOR + pe.len()].copy_from_slice(pe);
        img
    }

    #[test]
    fn two_links_normalize_to_identical_bytes() {
        let mut a = in_image(&synthetic_pe(0x23));
        let mut b = in_image(&synthetic_pe(0x9C));
        assert_ne!(a, b);
        assert_eq!(normalize_embedded(&mut a, digest), Ok(1));
        assert_eq!(normalize_embedded(&mut b, digest), Ok(1));
        assert_eq!(a, b);
        // The GUID is a real, content-derived value, not zeros.
        let guid = &a[2 * SECTOR + 0x344..2 * SECTOR + 0x354];
        assert!(guid.iter().any(|&x| x != 0));
        // Everything that is not link metadata is untouched.
        assert_eq!(&a[2 * SECTOR + 0x358..2 * SECTOR + 0x360], b"boot.pdb");
    }

    #[test]
    fn images_without_a_pe_are_left_alone() {
        let mut img = vec![0x5Au8; 8 * SECTOR];
        let before = img.clone();
        assert_eq!(normalize_embedded(&mut img, digest), Ok(0));
        assert_eq!(img, before);
    }

    #[test]
    fn a_codeview_entry_pointing_at_garbage_is_an_error() {
        let mut pe = synthetic_pe(1);
        pe[0x340..0x344].copy_from_slice(b"XXXX");
        assert!(normalize_embedded(&mut in_image(&pe), digest).is_err());
    }

    #[test]
    fn sections_past_the_image_are_an_error() {
        let mut pe = synthetic_pe(1);
        let s = 0x80 + 4 + 20 + 112 + 16 * 8;
        pe[s + 16..s + 20].copy_from_slice(&0x10_0000u32.to_le_bytes());
        assert!(normalize_embedded(&mut in_image(&pe), digest).is_err());
    }
}
