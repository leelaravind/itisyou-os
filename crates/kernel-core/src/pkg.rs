//! ITPKG — the smallest secure installable package format (V0.7).
//!
//! One file = header + app manifest + ELF payload, integrity-protected by a
//! SHA-256 digest over `manifest || payload`. The kernel verifies the digest
//! and validates the manifest BEFORE install and again before every launch;
//! a package that fails either check is refused outright. Parsing is strict
//! (exact lengths, no trailing bytes) and allocation-free. Authenticity
//! (signatures) requires key infrastructure and is deferred — see ADR-0012.
//!
//! Layout (little-endian):
//! ```text
//! 0..8    magic  "ITPKG001"
//! 8..12   manifest_len: u32
//! 12..16  payload_len:  u32
//! 16..48  sha256(manifest || payload)
//! 48..    manifest bytes, then payload bytes (exactly; no trailing data)
//! ```

use crate::manifest::{self, Manifest, ManifestError};
use crate::sha256::Sha256;

pub const MAGIC: &[u8; 8] = b"ITPKG001";
pub const HEADER_LEN: usize = 48;
/// Defensive payload bound (an ELF for this platform is well under this).
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkgError {
    TooShort,
    BadMagic,
    /// Declared lengths are inconsistent with the file size (truncated,
    /// oversized, or trailing bytes).
    BadLength,
    /// Recomputed SHA-256 does not match the header digest.
    DigestMismatch,
    Manifest(ManifestError),
    ManifestNotUtf8,
}

/// A parsed, digest-verified package borrowing from the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct Package<'a> {
    pub manifest: Manifest<'a>,
    pub manifest_raw: &'a [u8],
    pub payload: &'a [u8],
    pub digest: [u8; 32],
}

/// Build the 48-byte header for `pack`ing (used by build tooling and tests).
pub fn header(manifest_len: u32, payload_len: u32, digest: &[u8; 32]) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..8].copy_from_slice(MAGIC);
    h[8..12].copy_from_slice(&manifest_len.to_le_bytes());
    h[12..16].copy_from_slice(&payload_len.to_le_bytes());
    h[16..48].copy_from_slice(digest);
    h
}

/// Digest over `manifest || payload` (what the header must carry).
pub fn content_digest(manifest_bytes: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(manifest_bytes);
    h.update(payload);
    h.finalize()
}

/// Parse and FULLY verify a package: magic, exact lengths, digest, manifest.
pub fn parse(bytes: &[u8]) -> Result<Package<'_>, PkgError> {
    if bytes.len() < HEADER_LEN {
        return Err(PkgError::TooShort);
    }
    if &bytes[0..8] != MAGIC {
        return Err(PkgError::BadMagic);
    }
    let mlen = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let plen = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if mlen > manifest::MAX_MANIFEST_LEN || plen == 0 || plen > MAX_PAYLOAD {
        return Err(PkgError::BadLength);
    }
    // Exact-size check (also guards the additions against overflow).
    let expect = HEADER_LEN
        .checked_add(mlen)
        .and_then(|v| v.checked_add(plen))
        .ok_or(PkgError::BadLength)?;
    if bytes.len() != expect {
        return Err(PkgError::BadLength);
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&bytes[16..48]);
    let manifest_raw = &bytes[HEADER_LEN..HEADER_LEN + mlen];
    let payload = &bytes[HEADER_LEN + mlen..];
    if content_digest(manifest_raw, payload) != digest {
        return Err(PkgError::DigestMismatch);
    }
    let text = core::str::from_utf8(manifest_raw).map_err(|_| PkgError::ManifestNotUtf8)?;
    let manifest = manifest::parse(text).map_err(PkgError::Manifest)?;
    Ok(Package {
        manifest,
        manifest_raw,
        payload,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(manifest: &str, payload: &[u8]) -> Vec<u8> {
        let d = content_digest(manifest.as_bytes(), payload);
        let mut out = header(manifest.len() as u32, payload.len() as u32, &d).to_vec();
        out.extend_from_slice(manifest.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    const MAN: &str = "name=hello-app\nversion=1.0.0\ncaps=gui\n";

    #[test]
    fn round_trips_valid_package() {
        let bytes = pack(MAN, b"ELFPAYLOAD");
        let p = parse(&bytes).unwrap();
        assert_eq!(p.manifest.name, "hello-app");
        assert_eq!(p.payload, b"ELFPAYLOAD");
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        let mut bytes = pack(MAN, b"X");
        bytes[0] = b'X';
        assert_eq!(parse(&bytes).unwrap_err(), PkgError::BadMagic);
        assert_eq!(
            parse(&pack(MAN, b"XY")[..30]).unwrap_err(),
            PkgError::TooShort
        );
        // Truncated payload.
        let bytes = pack(MAN, b"PAYLOAD");
        assert_eq!(
            parse(&bytes[..bytes.len() - 1]).unwrap_err(),
            PkgError::BadLength
        );
        // Trailing junk.
        let mut bytes = pack(MAN, b"PAYLOAD");
        bytes.push(0);
        assert_eq!(parse(&bytes).unwrap_err(), PkgError::BadLength);
    }

    #[test]
    fn rejects_corrupted_payload_and_manifest() {
        // Flip a payload byte → digest mismatch.
        let mut bytes = pack(MAN, b"PAYLOAD");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert_eq!(parse(&bytes).unwrap_err(), PkgError::DigestMismatch);
        // Flip a manifest byte → digest mismatch (before manifest parsing).
        let mut bytes = pack(MAN, b"PAYLOAD");
        bytes[HEADER_LEN] ^= 0x01;
        assert_eq!(parse(&bytes).unwrap_err(), PkgError::DigestMismatch);
        // A digest-valid package whose manifest requests an unknown capability.
        let bytes = pack("name=x\nversion=1\ncaps=kernel\n", b"P");
        assert!(matches!(parse(&bytes).unwrap_err(), PkgError::Manifest(_)));
    }

    #[test]
    fn rejects_hostile_lengths() {
        // manifest_len larger than the whole file.
        let mut bytes = pack(MAN, b"PAYLOAD");
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            parse(&bytes).unwrap_err(),
            PkgError::BadLength | PkgError::DigestMismatch
        ));
        // Zero payload.
        let d = content_digest(MAN.as_bytes(), b"");
        let mut z = header(MAN.len() as u32, 0, &d).to_vec();
        z.extend_from_slice(MAN.as_bytes());
        assert_eq!(parse(&z).unwrap_err(), PkgError::BadLength);
    }
}
