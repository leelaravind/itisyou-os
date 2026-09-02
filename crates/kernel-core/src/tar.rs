//! Minimal, strictly-validating ustar (POSIX tar) reader for the initramfs.
//!
//! Read-only, allocation-free iteration over archive entries. The kernel
//! mounts the embedded archive through this parser; malformed input must be
//! rejected, never trusted (plan §11.2).

/// One archive entry borrowing from the archive bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// Entry path as stored (validated UTF-8, NUL-trimmed).
    pub name: &'a str,
    /// File contents (empty for directories).
    pub data: &'a [u8],
    /// True if this entry is a directory.
    pub is_dir: bool,
}

/// Archive parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarError {
    /// Header block is truncated (archive smaller than 512-byte boundary).
    TruncatedHeader { offset: usize },
    /// Entry data extends past the end of the archive.
    TruncatedData { offset: usize },
    /// Size field is not valid octal.
    BadSize { offset: usize },
    /// Header checksum mismatch — corrupt archive.
    BadChecksum { offset: usize },
    /// Entry name is not valid UTF-8.
    BadName { offset: usize },
}

const BLOCK: usize = 512;

/// Iterator over the entries of a ustar archive.
pub struct TarIter<'a> {
    bytes: &'a [u8],
    offset: usize,
    done: bool,
}

/// Iterate archive entries. Unsupported entry types (links, devices, pax
/// extensions) are yielded as errors-free skips only for pax metadata; V0.1
/// initramfs archives contain only files and directories.
pub fn entries(bytes: &[u8]) -> TarIter<'_> {
    TarIter {
        bytes,
        offset: 0,
        done: false,
    }
}

impl<'a> Iterator for TarIter<'a> {
    type Item = Result<Entry<'a>, TarError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }
            let off = self.offset;
            let rest = &self.bytes[off.min(self.bytes.len())..];
            if rest.is_empty() {
                // Archive may legally end without the two zero blocks; treat
                // clean end-of-bytes as end-of-archive.
                self.done = true;
                return None;
            }
            if rest.len() < BLOCK {
                self.done = true;
                return Some(Err(TarError::TruncatedHeader { offset: off }));
            }
            let header = &rest[..BLOCK];
            if header.iter().all(|&b| b == 0) {
                // Zero block terminates the archive.
                self.done = true;
                return None;
            }

            if !checksum_ok(header) {
                self.done = true;
                return Some(Err(TarError::BadChecksum { offset: off }));
            }

            let size = match parse_octal(&header[124..136]) {
                Some(s) => s as usize,
                None => {
                    self.done = true;
                    return Some(Err(TarError::BadSize { offset: off }));
                }
            };
            let name = match trimmed_str(&header[0..100]) {
                Some(n) => n,
                None => {
                    self.done = true;
                    return Some(Err(TarError::BadName { offset: off }));
                }
            };
            let typeflag = header[156];

            let data_start = off + BLOCK;
            let data_end = data_start + size;
            if data_end > self.bytes.len() {
                self.done = true;
                return Some(Err(TarError::TruncatedData { offset: off }));
            }
            // Advance to the next 512-aligned block after the data.
            self.offset = data_start + size.div_ceil(BLOCK) * BLOCK;

            match typeflag {
                // regular file ('0' or NUL for old archives)
                b'0' | 0 => {
                    return Some(Ok(Entry {
                        name,
                        data: &self.bytes[data_start..data_end],
                        is_dir: false,
                    }));
                }
                // directory
                b'5' => {
                    return Some(Ok(Entry {
                        name,
                        data: &[],
                        is_dir: true,
                    }));
                }
                // anything else (pax headers, links): skip safely.
                _ => continue,
            }
        }
    }
}

fn trimmed_str(field: &[u8]) -> Option<&str> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..end]).ok()
}

fn parse_octal(field: &[u8]) -> Option<u64> {
    let mut value: u64 = 0;
    let mut seen_digit = false;
    for &b in field {
        match b {
            b'0'..=b'7' => {
                value = value.checked_mul(8)?.checked_add((b - b'0') as u64)?;
                seen_digit = true;
            }
            b' ' | 0 => {
                if seen_digit {
                    break;
                }
            }
            _ => return None,
        }
    }
    seen_digit.then_some(value)
}

fn checksum_ok(header: &[u8]) -> bool {
    let stored = match parse_octal(&header[148..156]) {
        Some(v) => v,
        None => return false,
    };
    let mut sum: u64 = 0;
    for (i, &b) in header.iter().enumerate() {
        // Checksum field itself counts as spaces.
        sum += if (148..156).contains(&i) {
            b' ' as u64
        } else {
            b as u64
        };
    }
    sum == stored
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid ustar header + data blocks for a file.
    fn make_entry(name: &str, data: &[u8], typeflag: u8) -> Vec<u8> {
        let mut header = [0u8; BLOCK];
        header[0..name.len()].copy_from_slice(name.as_bytes());
        // mode/uid/gid: octal zeros
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        let size_field = format!("{:011o}", data.len());
        header[124..135].copy_from_slice(size_field.as_bytes());
        header[136..147].copy_from_slice(b"00000000000");
        header[156] = typeflag;
        header[257..262].copy_from_slice(b"ustar");
        header[263..265].copy_from_slice(b"00");
        // checksum: spaces during computation
        for b in &mut header[148..156] {
            *b = b' ';
        }
        let sum: u64 = header.iter().map(|&b| b as u64).sum();
        let chk = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(chk.as_bytes());

        let mut out = header.to_vec();
        out.extend_from_slice(data);
        while out.len() % BLOCK != 0 {
            out.push(0);
        }
        out
    }

    fn archive(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out: Vec<u8> = entries.concat();
        out.extend_from_slice(&[0u8; BLOCK * 2]);
        out
    }

    #[test]
    fn parses_files_and_dirs() {
        let bytes = archive(&[
            make_entry("etc/", &[], b'5'),
            make_entry("etc/version", b"0.1.0-dev\n", b'0'),
        ]);
        let got: Vec<_> = entries(&bytes).collect::<Result<_, _>>().unwrap();
        assert_eq!(got.len(), 2);
        assert!(got[0].is_dir);
        assert_eq!(got[0].name, "etc/");
        assert_eq!(got[1].name, "etc/version");
        assert_eq!(got[1].data, b"0.1.0-dev\n");
    }

    #[test]
    fn empty_archive_yields_nothing() {
        let bytes = [0u8; BLOCK * 2];
        assert_eq!(entries(&bytes).count(), 0);
        assert_eq!(entries(&[]).count(), 0);
    }

    #[test]
    fn rejects_truncated_header() {
        let bytes = archive(&[make_entry("a", b"x", b'0')]);
        let cut = &bytes[..100];
        let result: Vec<_> = entries(cut).collect();
        assert_eq!(result, vec![Err(TarError::TruncatedHeader { offset: 0 })]);
    }

    #[test]
    fn rejects_truncated_data() {
        let full = make_entry("a", &[b'x'; 600], b'0');
        // Keep header + only part of the data.
        let cut = &full[..BLOCK + 100];
        let result: Vec<_> = entries(cut).collect();
        assert_eq!(result, vec![Err(TarError::TruncatedData { offset: 0 })]);
    }

    #[test]
    fn rejects_corrupted_checksum() {
        let mut bytes = archive(&[make_entry("a", b"x", b'0')]);
        bytes[0] ^= 0xFF; // corrupt the name -> checksum mismatch
        let result: Vec<_> = entries(&bytes).collect();
        assert_eq!(result, vec![Err(TarError::BadChecksum { offset: 0 })]);
    }

    #[test]
    fn rejects_bad_size_field() {
        let mut bytes = archive(&[make_entry("a", b"x", b'0')]);
        // Overwrite size field with non-octal garbage, then fix checksum so
        // the size error (not checksum) is exercised.
        bytes[124..129].copy_from_slice(b"9zzzz");
        for b in &mut bytes[148..156] {
            *b = b' ';
        }
        let sum: u64 = bytes[..BLOCK]
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                if (148..156).contains(&i) {
                    b' ' as u64
                } else {
                    b as u64
                }
            })
            .sum();
        let chk = format!("{sum:06o}\0 ");
        bytes[148..156].copy_from_slice(chk.as_bytes());
        let result: Vec<_> = entries(&bytes).collect();
        assert_eq!(result, vec![Err(TarError::BadSize { offset: 0 })]);
    }

    #[test]
    fn skips_unsupported_entry_types() {
        let bytes = archive(&[
            make_entry("link", b"", b'2'),
            make_entry("real", b"data", b'0'),
        ]);
        let got: Vec<_> = entries(&bytes).collect::<Result<_, _>>().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "real");
    }
}
