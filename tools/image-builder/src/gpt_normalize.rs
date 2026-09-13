//! Make a GPT disk image bit-for-bit reproducible.
//!
//! The `gpt` crate the bootloader uses to lay out UEFI images assigns a fresh
//! *random* disk GUID and partition GUID on every build, so two builds of the
//! same commit produced different UEFI images (observed 2026-09-13: identical
//! BIOS images, different UEFI images). A release artifact whose checksum
//! changes every time it is rebuilt cannot be independently verified, so after
//! the image is written its GUIDs are replaced by values derived from the
//! image's own content, and every CRC the GUIDs feed is recomputed.
//!
//! Derivation: zero every GUID and CRC field, SHA-256 the whole image, and take
//! the GUIDs from that digest (with RFC 4122 version/variant bits set so they
//! remain well-formed). Same content ⇒ same GUIDs; different content ⇒
//! different GUIDs, so two distinct releases never share a disk identity.

const SECTOR: usize = 512;
const SIGNATURE: &[u8; 8] = b"EFI PART";
const HDR_CRC: usize = 16;
const HDR_BACKUP_LBA: usize = 32;
const HDR_DISK_GUID: usize = 56;
const HDR_ENTRIES_LBA: usize = 72;
const HDR_NUM_ENTRIES: usize = 80;
const HDR_ENTRY_SIZE: usize = 84;
const HDR_ENTRIES_CRC: usize = 88;
const ENTRY_UNIQUE_GUID: usize = 16;

struct Header {
    at: usize,
    size: usize,
    entries_at: usize,
    num: usize,
    esize: usize,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn header(image: &[u8], lba: u64) -> Result<Header, String> {
    let at = usize::try_from(lba)
        .ok()
        .and_then(|l| l.checked_mul(SECTOR))
        .filter(|&a| a + SECTOR <= image.len())
        .ok_or_else(|| format!("GPT header LBA {lba} outside the image"))?;
    if &image[at..at + 8] != SIGNATURE {
        return Err(format!("no GPT signature at LBA {lba}"));
    }
    let size = u32_at(image, at + 12) as usize;
    if !(92..=SECTOR).contains(&size) {
        return Err(format!("implausible GPT header size {size}"));
    }
    let entries_lba = u64_at(image, at + HDR_ENTRIES_LBA);
    let num = u32_at(image, at + HDR_NUM_ENTRIES) as usize;
    let esize = u32_at(image, at + HDR_ENTRY_SIZE) as usize;
    if esize < 128 || !esize.is_multiple_of(8) || num == 0 || num > 1024 {
        return Err(format!("implausible GPT entry table ({num} x {esize})"));
    }
    let entries_at = usize::try_from(entries_lba)
        .ok()
        .and_then(|l| l.checked_mul(SECTOR))
        .filter(|&a| a + num * esize <= image.len())
        .ok_or_else(|| format!("GPT entry table at LBA {entries_lba} outside the image"))?;
    Ok(Header {
        at,
        size,
        entries_at,
        num,
        esize,
    })
}

fn entry_used(image: &[u8], h: &Header, i: usize) -> bool {
    let e = h.entries_at + i * h.esize;
    image[e..e + 16].iter().any(|&b| b != 0)
}

/// IEEE 802.3 CRC-32 (the one GPT specifies).
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

fn well_formed(mut g: [u8; 16]) -> [u8; 16] {
    // GPT stores the first three GUID fields little-endian, so the version
    // nibble (top of data3) is byte 7 and the variant bits are in byte 8.
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

/// Replace the random GUIDs of a GPT image with content-derived ones and fix
/// every CRC. `digest` computes SHA-256 (passed in so this module carries no
/// hash implementation of its own).
pub fn normalize(image: &mut [u8], digest: impl Fn(&[u8]) -> [u8; 32]) -> Result<(), String> {
    let primary = header(image, 1)?;
    let backup_lba = u64_at(image, primary.at + HDR_BACKUP_LBA);
    let backup = header(image, backup_lba)?;
    if backup.num != primary.num || backup.esize != primary.esize {
        return Err("primary and backup GPT disagree on the entry table".into());
    }

    // 1. Zero everything random (and every CRC that covers it).
    for h in [&primary, &backup] {
        image[h.at + HDR_DISK_GUID..h.at + HDR_DISK_GUID + 16].fill(0);
        image[h.at + HDR_CRC..h.at + HDR_CRC + 4].fill(0);
        image[h.at + HDR_ENTRIES_CRC..h.at + HDR_ENTRIES_CRC + 4].fill(0);
        for i in 0..h.num {
            if entry_used(image, h, i) {
                let g = h.entries_at + i * h.esize + ENTRY_UNIQUE_GUID;
                image[g..g + 16].fill(0);
            }
        }
    }

    // 2. Derive identities from the now-deterministic content.
    let seed = digest(image);
    let disk_guid = well_formed(seed[..16].try_into().unwrap());
    let part_guid = |i: usize| -> [u8; 16] {
        let mut input = seed.to_vec();
        input.extend_from_slice(&(i as u32).to_le_bytes());
        well_formed(digest(&input)[..16].try_into().unwrap())
    };

    // 3. Write them into both copies and recompute the CRCs.
    for h in [&primary, &backup] {
        image[h.at + HDR_DISK_GUID..h.at + HDR_DISK_GUID + 16].copy_from_slice(&disk_guid);
        for i in 0..h.num {
            if entry_used(image, h, i) {
                let g = h.entries_at + i * h.esize + ENTRY_UNIQUE_GUID;
                image[g..g + 16].copy_from_slice(&part_guid(i));
            }
        }
        let table_crc = crc32(&image[h.entries_at..h.entries_at + h.num * h.esize]);
        image[h.at + HDR_ENTRIES_CRC..h.at + HDR_ENTRIES_CRC + 4]
            .copy_from_slice(&table_crc.to_le_bytes());
        let header_crc = crc32(&image[h.at..h.at + h.size]);
        image[h.at + HDR_CRC..h.at + HDR_CRC + 4].copy_from_slice(&header_crc.to_le_bytes());
    }
    Ok(())
}

/// Check both headers' CRCs (used by the tests and by the builder after
/// normalizing, so a malformed result is refused rather than shipped).
pub fn verify(image: &[u8]) -> Result<(), String> {
    let primary = header(image, 1)?;
    let backup = header(image, u64_at(image, primary.at + HDR_BACKUP_LBA))?;
    for (name, h) in [("primary", &primary), ("backup", &backup)] {
        let table = crc32(&image[h.entries_at..h.entries_at + h.num * h.esize]);
        if table != u32_at(image, h.at + HDR_ENTRIES_CRC) {
            return Err(format!("{name} GPT entry-table CRC mismatch"));
        }
        let mut hdr = image[h.at..h.at + h.size].to_vec();
        hdr[HDR_CRC..HDR_CRC + 4].fill(0);
        if crc32(&hdr) != u32_at(image, h.at + HDR_CRC) {
            return Err(format!("{name} GPT header CRC mismatch"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(data: &[u8]) -> [u8; 32] {
        crate::sha256(data)
    }

    /// A minimal two-header GPT with one partition and `guid_byte` standing in
    /// for the randomness the real crate injects.
    fn synthetic(guid_byte: u8) -> Vec<u8> {
        // 128 entries x 128 bytes = 32 sectors per table: primary table at
        // LBA 2..=33, backup at 95..=126 — they must not overlap.
        let sectors = 128usize;
        let mut img = vec![0u8; sectors * SECTOR];
        let last = (sectors - 1) as u64;
        for (hdr_lba, backup_lba, entries_lba) in [(1u64, last, 2u64), (last, 1, last - 32)] {
            let at = hdr_lba as usize * SECTOR;
            img[at..at + 8].copy_from_slice(SIGNATURE);
            img[at + 8..at + 12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
            img[at + 12..at + 16].copy_from_slice(&92u32.to_le_bytes());
            img[at + 24..at + 32].copy_from_slice(&hdr_lba.to_le_bytes());
            img[at + 32..at + 40].copy_from_slice(&backup_lba.to_le_bytes());
            img[at + HDR_DISK_GUID..at + HDR_DISK_GUID + 16].fill(guid_byte);
            img[at + 72..at + 80].copy_from_slice(&entries_lba.to_le_bytes());
            img[at + 80..at + 84].copy_from_slice(&128u32.to_le_bytes());
            img[at + 84..at + 88].copy_from_slice(&128u32.to_le_bytes());
            let e = entries_lba as usize * SECTOR;
            img[e..e + 16].fill(0xEF); // partition type GUID: in use
            img[e + 16..e + 32].fill(guid_byte.wrapping_add(1));
        }
        img
    }

    #[test]
    fn crc32_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn different_random_guids_normalize_to_identical_images() {
        let mut a = synthetic(0x11);
        let mut b = synthetic(0x77);
        assert_ne!(a, b);
        normalize(&mut a, digest).unwrap();
        normalize(&mut b, digest).unwrap();
        assert_eq!(a, b, "same content must yield the same image");
        verify(&a).unwrap();
        // The GUIDs are real identities, not zeros, and well-formed v4.
        let g = &a[SECTOR + HDR_DISK_GUID..SECTOR + HDR_DISK_GUID + 16];
        assert!(g.iter().any(|&x| x != 0));
        assert_eq!(g[7] >> 4, 4);
        assert_eq!(g[8] >> 6, 0b10);
    }

    #[test]
    fn different_content_yields_different_identities() {
        let mut a = synthetic(0x11);
        let mut b = synthetic(0x11);
        let len = b.len();
        b[len / 2] = 0x5A; // payload differs
        normalize(&mut a, digest).unwrap();
        normalize(&mut b, digest).unwrap();
        let ga = &a[SECTOR + HDR_DISK_GUID..SECTOR + HDR_DISK_GUID + 16];
        let gb = &b[SECTOR + HDR_DISK_GUID..SECTOR + HDR_DISK_GUID + 16];
        assert_ne!(ga, gb);
    }

    #[test]
    fn refuses_non_gpt_and_truncated_images() {
        let mut junk = vec![0u8; 8 * SECTOR];
        assert!(normalize(&mut junk, digest).is_err());
        let mut short = synthetic(0x11);
        short.truncate(10 * SECTOR); // backup header now outside the image
        assert!(normalize(&mut short, digest).is_err());
    }

    #[test]
    fn verify_detects_a_corrupted_crc() {
        let mut a = synthetic(0x11);
        normalize(&mut a, digest).unwrap();
        a[SECTOR + HDR_DISK_GUID] ^= 1;
        assert!(verify(&a).is_err());
    }
}
