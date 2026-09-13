//! ACPI table parsing (V0.9): RSDP → RSDT/XSDT → MADT and FADT, plus the
//! `\_S5_` sleep type from the DSDT.
//!
//! Firmware tables are input, not truth: every length is checked against the
//! bytes actually available, every table's checksum must sum to zero, entry
//! lengths of zero (which would make a naive walker spin forever) are refused,
//! and unknown entry types are skipped by their declared length rather than
//! guessed at. Nothing here dereferences memory — the kernel hands in byte
//! slices it has already bounded, so this is all host-testable.

/// Why a table was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// Fewer bytes than the structure's fixed part or declared length.
    Truncated,
    /// The signature is not the one this parser was asked to read.
    BadSignature,
    /// The bytes do not sum to zero (mod 256).
    BadChecksum,
    /// A length field that cannot be right (shorter than the header, or an
    /// entry claiming fewer bytes than its own type/length prefix).
    BadLength,
}

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b)) == 0
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

// --- RSDP --------------------------------------------------------------------

/// Root System Description Pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsdp {
    pub revision: u8,
    pub rsdt_address: u32,
    /// Present (non-zero) only for ACPI 2.0+ (`revision >= 2`).
    pub xsdt_address: u64,
}

pub const RSDP_V1_LEN: usize = 20;
pub const RSDP_V2_LEN: usize = 36;

impl Rsdp {
    /// Parse and validate an RSDP. `bytes` should hold at least
    /// [`RSDP_V2_LEN`] bytes when available; a v1 RSDP needs only 20.
    pub fn parse(bytes: &[u8]) -> Result<Rsdp, AcpiError> {
        if bytes.len() < RSDP_V1_LEN {
            return Err(AcpiError::Truncated);
        }
        if &bytes[..8] != b"RSD PTR " {
            return Err(AcpiError::BadSignature);
        }
        if !checksum_ok(&bytes[..RSDP_V1_LEN]) {
            return Err(AcpiError::BadChecksum);
        }
        let revision = bytes[15];
        let rsdt_address = u32_at(bytes, 16);
        let mut xsdt_address = 0;
        if revision >= 2 {
            if bytes.len() < RSDP_V2_LEN {
                return Err(AcpiError::Truncated);
            }
            let length = u32_at(bytes, 20) as usize;
            if length < RSDP_V2_LEN {
                return Err(AcpiError::BadLength);
            }
            if bytes.len() < length {
                return Err(AcpiError::Truncated);
            }
            if !checksum_ok(&bytes[..length]) {
                return Err(AcpiError::BadChecksum);
            }
            xsdt_address = u64_at(bytes, 24);
        }
        Ok(Rsdp {
            revision,
            rsdt_address,
            xsdt_address,
        })
    }
}

// --- System description tables ---------------------------------------------

pub const SDT_HEADER_LEN: usize = 36;

/// The 36-byte header every system description table starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
}

/// Read just the header (to learn a table's length before bounding it).
pub fn peek_header(bytes: &[u8]) -> Result<SdtHeader, AcpiError> {
    if bytes.len() < SDT_HEADER_LEN {
        return Err(AcpiError::Truncated);
    }
    let length = u32_at(bytes, 4);
    if (length as usize) < SDT_HEADER_LEN {
        return Err(AcpiError::BadLength);
    }
    Ok(SdtHeader {
        signature: [bytes[0], bytes[1], bytes[2], bytes[3]],
        length,
        revision: bytes[8],
    })
}

/// Validate a whole table: signature (if given), declared length within the
/// buffer, checksum. Returns exactly the table's bytes.
pub fn validate_table<'a>(
    bytes: &'a [u8],
    signature: Option<&[u8; 4]>,
) -> Result<&'a [u8], AcpiError> {
    let h = peek_header(bytes)?;
    if let Some(sig) = signature {
        if &h.signature != sig {
            return Err(AcpiError::BadSignature);
        }
    }
    let len = h.length as usize;
    if bytes.len() < len {
        return Err(AcpiError::Truncated);
    }
    let table = &bytes[..len];
    if !checksum_ok(table) {
        return Err(AcpiError::BadChecksum);
    }
    Ok(table)
}

/// The physical addresses listed by a validated RSDT (`XSDT == false`, 32-bit
/// entries) or XSDT (64-bit entries), written into `out`. Returns how many.
pub fn root_entries(table: &[u8], xsdt: bool, out: &mut [u64]) -> Result<usize, AcpiError> {
    let sig: &[u8; 4] = if xsdt { b"XSDT" } else { b"RSDT" };
    let table = validate_table(table, Some(sig))?;
    let width = if xsdt { 8 } else { 4 };
    let body = &table[SDT_HEADER_LEN..];
    if body.len() % width != 0 {
        return Err(AcpiError::BadLength);
    }
    let mut n = 0;
    for chunk in body.chunks_exact(width) {
        if n == out.len() {
            break;
        }
        out[n] = if xsdt {
            u64_at(chunk, 0)
        } else {
            u32_at(chunk, 0) as u64
        };
        n += 1;
    }
    Ok(n)
}

// --- MADT ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApicEntry {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

/// An ISA interrupt remapped to a different global system interrupt, with its
/// electrical characteristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Override {
    pub source: u8,
    pub gsi: u32,
    /// `true` = active low. ISA default (flags "conforms") is active high.
    pub active_low: bool,
    /// `true` = level triggered. ISA default is edge.
    pub level: bool,
}

pub const MAX_IOAPICS: usize = 4;
pub const MAX_OVERRIDES: usize = 16;
pub const MAX_CPUS: usize = 16;

/// What the kernel needs from the MADT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Madt {
    pub local_apic_address: u64,
    /// The machine also has dual 8259 PICs (PCAT_COMPAT) that must be masked
    /// before the I/O APIC takes over.
    pub pcat_compat: bool,
    pub cpus: usize,
    pub ioapics: [Option<IoApicEntry>; MAX_IOAPICS],
    pub overrides: [Option<Override>; MAX_OVERRIDES],
}

impl Madt {
    pub fn parse(bytes: &[u8]) -> Result<Madt, AcpiError> {
        let table = validate_table(bytes, Some(b"APIC"))?;
        if table.len() < SDT_HEADER_LEN + 8 {
            return Err(AcpiError::Truncated);
        }
        let mut madt = Madt {
            local_apic_address: u32_at(table, 36) as u64,
            pcat_compat: u32_at(table, 40) & 1 != 0,
            cpus: 0,
            ioapics: [None; MAX_IOAPICS],
            overrides: [None; MAX_OVERRIDES],
        };
        let mut at = SDT_HEADER_LEN + 8;
        while at < table.len() {
            if at + 2 > table.len() {
                return Err(AcpiError::Truncated);
            }
            let kind = table[at];
            let len = table[at + 1] as usize;
            // A zero or one-byte entry would never advance the walk.
            if len < 2 {
                return Err(AcpiError::BadLength);
            }
            if at + len > table.len() {
                return Err(AcpiError::Truncated);
            }
            let e = &table[at..at + len];
            match kind {
                0 if len >= 8 => {
                    // Processor local APIC; bit 0 of flags = enabled.
                    if u32_at(e, 4) & 1 != 0 {
                        madt.cpus = (madt.cpus + 1).min(MAX_CPUS);
                    }
                }
                1 if len >= 12 => {
                    if let Some(slot) = madt.ioapics.iter_mut().find(|s| s.is_none()) {
                        *slot = Some(IoApicEntry {
                            id: e[2],
                            address: u32_at(e, 4),
                            gsi_base: u32_at(e, 8),
                        });
                    }
                }
                2 if len >= 10 => {
                    let flags = u16_at(e, 8);
                    if let Some(slot) = madt.overrides.iter_mut().find(|s| s.is_none()) {
                        *slot = Some(Override {
                            source: e[3],
                            gsi: u32_at(e, 4),
                            active_low: flags & 0b11 == 0b11,
                            level: (flags >> 2) & 0b11 == 0b11,
                        });
                    }
                }
                5 if len >= 12 => madt.local_apic_address = u64_at(e, 4),
                // A known type that is too short for its fields is malformed.
                0..=2 | 5 => return Err(AcpiError::BadLength),
                // Anything else (NMI sources, x2APIC, GIC, ...) is skipped by
                // its declared length.
                _ => {}
            }
            at += len;
        }
        Ok(madt)
    }

    /// The GSI (and polarity/trigger) an ISA IRQ actually arrives on: the
    /// override if the firmware declared one, otherwise identity-mapped,
    /// active high, edge triggered.
    pub fn isa_route(&self, irq: u8) -> Override {
        self.overrides
            .iter()
            .flatten()
            .find(|o| o.source == irq)
            .copied()
            .unwrap_or(Override {
                source: irq,
                gsi: irq as u32,
                active_low: false,
                level: false,
            })
    }

    /// The I/O APIC whose GSI window contains `gsi`, given each chip's number
    /// of redirection entries (read from the chip, since the MADT does not
    /// carry it).
    pub fn ioapic_for(
        &self,
        gsi: u32,
        entries: impl Fn(&IoApicEntry) -> u32,
    ) -> Option<IoApicEntry> {
        self.ioapics
            .iter()
            .flatten()
            .find(|a| gsi >= a.gsi_base && gsi < a.gsi_base + entries(a))
            .copied()
    }
}

// --- FADT ----------------------------------------------------------------------

/// The FADT fields power management needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fadt {
    pub dsdt: u64,
    pub sci_interrupt: u16,
    pub smi_command: u32,
    pub acpi_enable: u8,
    pub pm1a_control: u32,
    pub pm1b_control: u32,
}

impl Fadt {
    pub fn parse(bytes: &[u8]) -> Result<Fadt, AcpiError> {
        let table = validate_table(bytes, Some(b"FACP"))?;
        // Fields through PM1b_CNT_BLK (offset 68..72) are in every revision.
        if table.len() < 72 {
            return Err(AcpiError::Truncated);
        }
        let mut dsdt = u32_at(table, 40) as u64;
        // ACPI 2.0+ carries a 64-bit X_DSDT at 140; prefer it when present.
        if table.len() >= 148 {
            let x = u64_at(table, 140);
            if x != 0 {
                dsdt = x;
            }
        }
        Ok(Fadt {
            dsdt,
            sci_interrupt: u16_at(table, 46),
            smi_command: u32_at(table, 48),
            acpi_enable: table[52],
            pm1a_control: u32_at(table, 64),
            pm1b_control: u32_at(table, 68),
        })
    }
}

// --- DSDT: \_S5_ -------------------------------------------------------------

/// `SLP_TYPa`/`SLP_TYPb` for the S5 (soft-off) state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepType {
    pub a: u8,
    pub b: u8,
}

/// Decode an AML PkgLength at `at`; returns (length, bytes consumed).
fn pkg_length(aml: &[u8], at: usize) -> Option<(usize, usize)> {
    let lead = *aml.get(at)?;
    let follow = (lead >> 6) as usize;
    if follow == 0 {
        return Some(((lead & 0x3F) as usize, 1));
    }
    let mut len = (lead & 0x0F) as usize;
    for i in 0..follow {
        len |= (*aml.get(at + 1 + i)? as usize) << (4 + 8 * i);
    }
    Some((len, 1 + follow))
}

/// Decode one small AML integer (ZeroOp, OneOp, BytePrefix, WordPrefix);
/// returns (value, bytes consumed).
fn small_integer(aml: &[u8], at: usize) -> Option<(u64, usize)> {
    match *aml.get(at)? {
        0x00 => Some((0, 1)),
        0x01 => Some((1, 1)),
        0x0A => Some((*aml.get(at + 1)? as u64, 2)),
        0x0B => Some((
            u16::from_le_bytes([*aml.get(at + 1)?, *aml.get(at + 2)?]) as u64,
            3,
        )),
        _ => None,
    }
}

/// Find `Name(_S5_, Package(){SLP_TYPa, SLP_TYPb, ...})` in a validated DSDT.
///
/// This is a pattern search, not an AML interpreter: it looks for the NameOp
/// that defines `_S5_` and decodes the package that follows it. That is enough
/// for a static package — which is what firmware ships for `_S5_` — and it
/// refuses anything it does not fully understand instead of guessing a value
/// that would be written to a power-control register.
pub fn find_s5(dsdt: &[u8]) -> Result<SleepType, AcpiError> {
    let table = validate_table(dsdt, Some(b"DSDT"))?;
    let aml = &table[SDT_HEADER_LEN..];
    let mut i = 0;
    while i + 4 <= aml.len() {
        if &aml[i..i + 4] == b"_S5_" {
            // Preceded by NameOp (0x08), optionally with a root prefix '\'.
            let name_op = (i >= 1 && aml[i - 1] == 0x08)
                || (i >= 2 && aml[i - 1] == b'\\' && aml[i - 2] == 0x08);
            if name_op && aml.get(i + 4) == Some(&0x12) {
                let (pkg_len, used) = pkg_length(aml, i + 5).ok_or(AcpiError::Truncated)?;
                let start = i + 5 + used;
                let end = i + 5 + pkg_len;
                if pkg_len < used + 1 || end > aml.len() {
                    return Err(AcpiError::BadLength);
                }
                let count = aml[start] as usize;
                if count < 2 {
                    return Err(AcpiError::BadLength);
                }
                let (a, n) = small_integer(aml, start + 1).ok_or(AcpiError::BadLength)?;
                let (b, _) = small_integer(aml, start + 1 + n).ok_or(AcpiError::BadLength)?;
                if a > 7 || b > 7 {
                    // SLP_TYP is a 3-bit field.
                    return Err(AcpiError::BadLength);
                }
                return Ok(SleepType {
                    a: a as u8,
                    b: b as u8,
                });
            }
        }
        i += 1;
    }
    Err(AcpiError::BadSignature)
}

/// The PM1 control value that requests sleep state `slp_typ` (SLP_EN = bit 13,
/// SLP_TYP = bits 10..12), preserving the other bits of `current`.
pub fn pm1_sleep_value(current: u16, slp_typ: u8) -> u16 {
    (current & !(0b111 << 10)) | ((slp_typ as u16 & 0b111) << 10) | (1 << 13)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn fix_checksum(t: &mut [u8], at: usize) {
        t[at] = 0;
        let sum = t.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        t[at] = 0u8.wrapping_sub(sum);
    }

    fn table(sig: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(sig);
        t.extend_from_slice(&((SDT_HEADER_LEN + body.len()) as u32).to_le_bytes());
        t.push(1); // revision
        t.push(0); // checksum
        t.extend_from_slice(b"ITISYO"); // OEM id
        t.extend_from_slice(b"TESTTBL "); // OEM table id
        t.extend_from_slice(&[0; 12]); // OEM rev, creator id, creator rev
        t.extend_from_slice(body);
        fix_checksum(&mut t, 9);
        t
    }

    fn rsdp_v2(rsdt: u32, xsdt: u64) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(b"RSD PTR ");
        r.push(0);
        r.extend_from_slice(b"ITISYO");
        r.push(2);
        r.extend_from_slice(&rsdt.to_le_bytes());
        r.extend_from_slice(&36u32.to_le_bytes());
        r.extend_from_slice(&xsdt.to_le_bytes());
        r.push(0);
        r.extend_from_slice(&[0; 3]);
        fix_checksum(&mut r[..20], 8);
        fix_checksum(&mut r, 32);
        r
    }

    /// A MADT shaped like QEMU's `pc` machine: one CPU, one I/O APIC at
    /// 0xFEC00000, ISA IRQ0 overridden to GSI 2, and IRQ9 (SCI) level/high.
    fn qemu_madt() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes()); // PCAT_COMPAT
        body.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]); // CPU 0, enabled
        let mut io = [1u8, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        io[4..8].copy_from_slice(&0xFEC0_0000u32.to_le_bytes());
        body.extend_from_slice(&io);
        body.extend_from_slice(&[2, 10, 0, 0, 2, 0, 0, 0, 0, 0]); // IRQ0 -> GSI2
        body.extend_from_slice(&[2, 10, 0, 9, 9, 0, 0, 0, 0x0D, 0]); // IRQ9 level, high
        body.extend_from_slice(&[4, 6, 0xFF, 0, 0, 1]); // LAPIC NMI (skipped)
        table(b"APIC", &body)
    }

    #[test]
    fn rsdp_v1_and_v2_validate() {
        let r = rsdp_v2(0x1000, 0x2000);
        let p = Rsdp::parse(&r).unwrap();
        assert_eq!(
            (p.revision, p.rsdt_address, p.xsdt_address),
            (2, 0x1000, 0x2000)
        );
        let mut v1 = r[..20].to_vec();
        v1[15] = 0;
        fix_checksum(&mut v1, 8);
        assert_eq!(Rsdp::parse(&v1).unwrap().xsdt_address, 0);
    }

    #[test]
    fn rsdp_refuses_bad_signature_checksum_and_truncation() {
        let mut r = rsdp_v2(0x1000, 0x2000);
        assert_eq!(Rsdp::parse(&r[..19]), Err(AcpiError::Truncated));
        assert_eq!(Rsdp::parse(&r[..30]), Err(AcpiError::Truncated));
        r[24] ^= 1; // extended checksum no longer holds
        assert_eq!(Rsdp::parse(&r), Err(AcpiError::BadChecksum));
        let mut s = rsdp_v2(0x1000, 0x2000);
        s[0] = b'X';
        assert_eq!(Rsdp::parse(&s), Err(AcpiError::BadSignature));
    }

    #[test]
    fn root_tables_list_their_entries() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x1111u32.to_le_bytes());
        body.extend_from_slice(&0x2222u32.to_le_bytes());
        let rsdt = table(b"RSDT", &body);
        let mut out = [0u64; 8];
        assert_eq!(root_entries(&rsdt, false, &mut out), Ok(2));
        assert_eq!(&out[..2], &[0x1111, 0x2222]);
        let mut xbody = Vec::new();
        xbody.extend_from_slice(&0x1_0000_0000u64.to_le_bytes());
        let xsdt = table(b"XSDT", &xbody);
        assert_eq!(root_entries(&xsdt, true, &mut out), Ok(1));
        assert_eq!(out[0], 0x1_0000_0000);
        // A body that is not a whole number of entries is malformed.
        let bad = table(b"RSDT", &[1, 2, 3]);
        assert_eq!(
            root_entries(&bad, false, &mut out),
            Err(AcpiError::BadLength)
        );
    }

    #[test]
    fn tables_refuse_lies_about_their_length_and_checksum() {
        let t = table(b"APIC", &[0; 8]);
        assert_eq!(validate_table(&t[..40], None), Err(AcpiError::Truncated));
        let mut short = t.clone();
        short[4..8].copy_from_slice(&10u32.to_le_bytes());
        assert_eq!(validate_table(&short, None), Err(AcpiError::BadLength));
        let mut flipped = t.clone();
        flipped[40] ^= 0x80;
        assert_eq!(validate_table(&flipped, None), Err(AcpiError::BadChecksum));
        assert_eq!(
            validate_table(&t, Some(b"FACP")),
            Err(AcpiError::BadSignature)
        );
    }

    #[test]
    fn madt_like_qemu_routes_the_timer_through_gsi2() {
        let m = Madt::parse(&qemu_madt()).unwrap();
        assert_eq!(m.local_apic_address, 0xFEE0_0000);
        assert!(m.pcat_compat);
        assert_eq!(m.cpus, 1);
        assert_eq!(
            m.ioapics[0],
            Some(IoApicEntry {
                id: 0,
                address: 0xFEC0_0000,
                gsi_base: 0
            })
        );
        let timer = m.isa_route(0);
        assert_eq!(
            (timer.gsi, timer.active_low, timer.level),
            (2, false, false)
        );
        let kbd = m.isa_route(1); // no override: identity, edge, high
        assert_eq!((kbd.gsi, kbd.active_low, kbd.level), (1, false, false));
        let sci = m.isa_route(9);
        assert_eq!((sci.gsi, sci.active_low, sci.level), (9, false, true));
        assert!(m.ioapic_for(2, |_| 24).is_some());
        assert!(m.ioapic_for(24, |_| 24).is_none());
    }

    #[test]
    fn madt_refuses_entries_that_would_never_advance_or_overrun() {
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&[9, 0]); // zero-length entry: an infinite loop for a naive walker
        assert_eq!(
            Madt::parse(&table(b"APIC", &body)),
            Err(AcpiError::BadLength)
        );

        let mut over = Vec::new();
        over.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        over.extend_from_slice(&0u32.to_le_bytes());
        over.extend_from_slice(&[1, 40, 0, 0]); // claims 40 bytes, has 4
        assert_eq!(
            Madt::parse(&table(b"APIC", &over)),
            Err(AcpiError::Truncated)
        );

        let mut short_ioapic = Vec::new();
        short_ioapic.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        short_ioapic.extend_from_slice(&0u32.to_le_bytes());
        short_ioapic.extend_from_slice(&[1, 6, 0, 0, 0, 0]); // IOAPIC needs 12
        assert_eq!(
            Madt::parse(&table(b"APIC", &short_ioapic)),
            Err(AcpiError::BadLength)
        );
    }

    fn fadt(rev2: bool) -> Vec<u8> {
        let mut body = std::vec![0u8; if rev2 { 244 - 36 } else { 116 - 36 }];
        body[40 - 36..44 - 36].copy_from_slice(&0x0FFE_0040u32.to_le_bytes()); // DSDT
        body[46 - 36..48 - 36].copy_from_slice(&9u16.to_le_bytes()); // SCI
        body[48 - 36..52 - 36].copy_from_slice(&0xB2u32.to_le_bytes()); // SMI_CMD
        body[52 - 36] = 0xF1; // ACPI_ENABLE
        body[64 - 36..68 - 36].copy_from_slice(&0x604u32.to_le_bytes()); // PM1a_CNT
        if rev2 {
            body[140 - 36..148 - 36].copy_from_slice(&0x1_0000_0040u64.to_le_bytes());
        }
        table(b"FACP", &body)
    }

    #[test]
    fn fadt_reads_the_power_control_block_and_prefers_x_dsdt() {
        let f = Fadt::parse(&fadt(false)).unwrap();
        assert_eq!(
            (f.dsdt, f.pm1a_control, f.sci_interrupt),
            (0x0FFE_0040, 0x604, 9)
        );
        assert_eq!((f.smi_command, f.acpi_enable), (0xB2, 0xF1));
        assert_eq!(Fadt::parse(&fadt(true)).unwrap().dsdt, 0x1_0000_0040);
        assert_eq!(
            Fadt::parse(&table(b"FACP", &[0; 20])),
            Err(AcpiError::Truncated)
        );
    }

    fn dsdt(aml: &[u8]) -> Vec<u8> {
        table(b"DSDT", aml)
    }

    #[test]
    fn finds_s5_in_the_shapes_firmware_uses() {
        // Name(_S5_, Package(4){Zero, Zero, Zero, Zero}) — QEMU's pc machine.
        let aml = [
            0x10, 0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(find_s5(&dsdt(&aml)), Ok(SleepType { a: 0, b: 0 }));
        // Name(\_S5_, Package(2){0x05, 0x05}) with a root prefix and BytePrefix.
        let aml2 = [
            0x08, b'\\', b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x0A, 0x05, 0x0A, 0x05,
        ];
        assert_eq!(find_s5(&dsdt(&aml2)), Ok(SleepType { a: 5, b: 5 }));
    }

    #[test]
    fn s5_refuses_what_it_does_not_understand() {
        // `_S5_` referenced but not defined by NameOp.
        let not_named = [0x70, b'_', b'S', b'5', b'_', 0x60];
        assert_eq!(find_s5(&dsdt(&not_named)), Err(AcpiError::BadSignature));
        // A value that does not fit the 3-bit SLP_TYP field.
        let wide = [
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x0A, 0x09, 0x0A, 0x05,
        ];
        assert_eq!(find_s5(&dsdt(&wide)), Err(AcpiError::BadLength));
        // A package whose declared length runs past the table.
        let long = [0x08, b'_', b'S', b'5', b'_', 0x12, 0x3F, 0x02, 0x00, 0x00];
        assert_eq!(find_s5(&dsdt(&long)), Err(AcpiError::BadLength));
        // A computed (non-literal) element is not guessed at.
        let computed = [
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x05, 0x02, 0x70, 0x00, 0x00,
        ];
        assert_eq!(find_s5(&dsdt(&computed)), Err(AcpiError::BadLength));
    }

    #[test]
    fn pm1_sleep_value_sets_only_slp_typ_and_slp_en() {
        assert_eq!(pm1_sleep_value(0x0001, 0), 0x2001);
        assert_eq!(pm1_sleep_value(0x1C01, 5), 0x3401);
    }
}
