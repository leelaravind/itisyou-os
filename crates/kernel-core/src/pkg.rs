//! ITPKG — the smallest secure installable package format (V0.7).
//!
//! One file = header + app manifest + ELF payload, integrity-protected by a
//! SHA-256 digest over `manifest || payload` and, since V0.8, authenticated by
//! an Ed25519 signature from a trusted key.
//!
//! **Integrity and authenticity are separate steps on purpose.** [`parse`]
//! answers "are these bytes a well-formed, undamaged package?" and
//! [`verify_trust`] answers "did someone we trust vouch for it?". Fusing them
//! would collapse three very different failures — a corrupt download, a
//! package from a stranger, and a forged signature — into one refusal, and an
//! operator needs to tell those apart.
//!
//! Two magics exist:
//!
//! * `ITPKG001` — the V0.7 unsigned format. Still parseable, so an older
//!   package produces `Unsigned` rather than `BadMagic`: "we know exactly what
//!   this is and we will not install it" beats "unrecognised bytes".
//! * `ITPKG002` — signed. The signature covers a context string, the declared
//!   lengths and the content digest, so it cannot be lifted onto a package
//!   with different contents or a different shape.
//!
//! Layout (little-endian):
//! ```text
//! 0..8     magic  "ITPKG001" | "ITPKG002"
//! 8..12    manifest_len: u32
//! 12..16   payload_len:  u32
//! 16..48   sha256(manifest || payload)
//! (ITPKG002 only)
//! 48..80   signer public key (Ed25519)
//! 80..144  signature over `signing_input`
//! then     manifest bytes, then payload bytes (exactly; no trailing data)
//! ```

use crate::manifest::{self, Manifest, ManifestError};
use crate::sha256::Sha256;

/// V0.7 unsigned package magic.
pub const MAGIC: &[u8; 8] = b"ITPKG001";
/// V0.8 signed package magic.
pub const MAGIC_SIGNED: &[u8; 8] = b"ITPKG002";
/// Header length of an unsigned package.
pub const HEADER_LEN: usize = 48;
/// Header length of a signed package: the unsigned header plus a 32-byte
/// public key and a 64-byte signature.
pub const HEADER_LEN_SIGNED: usize = HEADER_LEN + 32 + crate::ed25519::SIGNATURE_LEN;

/// Domain separation for package signatures.
///
/// Without it, a signature this key produced over some unrelated 32-byte blob
/// (a file digest, a nonce) could be presented as a package signature. The
/// context makes "signed as an ITPKG002 package" a distinct statement.
pub const SIGNING_CONTEXT: &[u8] = b"ITISYOU-OS/ITPKG002/v1";

/// The exact bytes a package signature covers: the context, the declared
/// lengths, and the content digest.
///
/// The lengths are included as well as the digest so a signature cannot be
/// moved onto a package that claims a different shape — a header saying the
/// same bytes are a longer manifest and a shorter payload would otherwise
/// carry a valid signature.
pub fn signing_input(manifest_len: u32, payload_len: u32, digest: &[u8; 32]) -> [u8; 62] {
    let mut out = [0u8; 62];
    out[..22].copy_from_slice(SIGNING_CONTEXT);
    out[22..26].copy_from_slice(&manifest_len.to_le_bytes());
    out[26..30].copy_from_slice(&payload_len.to_le_bytes());
    out[30..62].copy_from_slice(digest);
    out
}
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

/// Why a well-formed package is not trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustError {
    /// No signature at all (a V0.7 `ITPKG001` package).
    Unsigned,
    /// Correctly signed, but by a key that is not in the trust store. This is
    /// NOT the same as a bad signature: the package is intact and genuinely
    /// from whoever holds that key — we simply do not know them.
    UntrustedSigner,
    /// The signature does not verify under its own embedded key: the bytes
    /// were altered after signing, or the signature was fabricated.
    BadSignature,
}

impl TrustError {
    /// A short stable name for logs and audit records.
    pub const fn name(self) -> &'static str {
        match self {
            TrustError::Unsigned => "unsigned",
            TrustError::UntrustedSigner => "untrusted_signer",
            TrustError::BadSignature => "bad_signature",
        }
    }
}

/// A package's signature block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureBlock {
    pub public_key: [u8; crate::ed25519::PUBLIC_KEY_LEN],
    pub signature: [u8; crate::ed25519::SIGNATURE_LEN],
}

/// A parsed, digest-verified package borrowing from the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct Package<'a> {
    pub manifest: Manifest<'a>,
    pub manifest_raw: &'a [u8],
    pub payload: &'a [u8],
    pub digest: [u8; 32],
    /// Present only for `ITPKG002`.
    pub signature: Option<SignatureBlock>,
}

/// Check a parsed package against a trust store.
///
/// Called separately from [`parse`] so the three ways a package can fail to be
/// trustworthy stay distinguishable. An empty trust store trusts nothing —
/// deliberately: a system that accepts anything when it has no keys configured
/// is worse than one that installs nothing at all, because the failure is
/// silent.
pub fn verify_trust(
    package: &Package<'_>,
    trusted: &[[u8; crate::ed25519::PUBLIC_KEY_LEN]],
) -> Result<(), TrustError> {
    let Some(block) = package.signature else {
        return Err(TrustError::Unsigned);
    };
    let manifest_len = package.manifest_raw.len() as u32;
    let payload_len = package.payload.len() as u32;
    let input = signing_input(manifest_len, payload_len, &package.digest);
    // Signature first, then trust. A forged signature from a trusted key and a
    // real signature from an unknown one are different problems, and checking
    // in this order means the reported reason is the more serious of the two.
    if !crate::ed25519::verify(&block.public_key, &input, &block.signature) {
        return Err(TrustError::BadSignature);
    }
    if !trusted.contains(&block.public_key) {
        return Err(TrustError::UntrustedSigner);
    }
    Ok(())
}

/// Build the header for a signed package.
pub fn header_signed(
    manifest_len: u32,
    payload_len: u32,
    digest: &[u8; 32],
    public_key: &[u8; crate::ed25519::PUBLIC_KEY_LEN],
    signature: &[u8; crate::ed25519::SIGNATURE_LEN],
) -> [u8; HEADER_LEN_SIGNED] {
    let mut h = [0u8; HEADER_LEN_SIGNED];
    h[0..8].copy_from_slice(MAGIC_SIGNED);
    h[8..12].copy_from_slice(&manifest_len.to_le_bytes());
    h[12..16].copy_from_slice(&payload_len.to_le_bytes());
    h[16..48].copy_from_slice(digest);
    h[48..80].copy_from_slice(public_key);
    h[80..HEADER_LEN_SIGNED].copy_from_slice(signature);
    h
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
    let signed = if &bytes[0..8] == MAGIC_SIGNED {
        true
    } else if &bytes[0..8] == MAGIC {
        false
    } else {
        return Err(PkgError::BadMagic);
    };
    let header_len = if signed {
        HEADER_LEN_SIGNED
    } else {
        HEADER_LEN
    };
    if bytes.len() < header_len {
        return Err(PkgError::TooShort);
    }
    let mlen = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let plen = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if mlen > manifest::MAX_MANIFEST_LEN || plen == 0 || plen > MAX_PAYLOAD {
        return Err(PkgError::BadLength);
    }
    // Exact-size check (also guards the additions against overflow).
    let expect = header_len
        .checked_add(mlen)
        .and_then(|v| v.checked_add(plen))
        .ok_or(PkgError::BadLength)?;
    if bytes.len() != expect {
        return Err(PkgError::BadLength);
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&bytes[16..48]);
    let signature = if signed {
        let mut public_key = [0u8; crate::ed25519::PUBLIC_KEY_LEN];
        public_key.copy_from_slice(&bytes[48..80]);
        let mut sig = [0u8; crate::ed25519::SIGNATURE_LEN];
        sig.copy_from_slice(&bytes[80..HEADER_LEN_SIGNED]);
        Some(SignatureBlock {
            public_key,
            signature: sig,
        })
    } else {
        None
    };
    let manifest_raw = &bytes[header_len..header_len + mlen];
    let payload = &bytes[header_len + mlen..];
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
        signature,
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

    // --- signed packages (V0.8) ------------------------------------------

    /// A development key pair, derived from a fixed seed so the tests are
    /// reproducible. Nothing here is secret: it exists only to produce
    /// signatures the same run then checks.
    fn keypair(seed: u8) -> ([u8; 32], [u8; 32]) {
        let secret = [seed; 32];
        (secret, crate::ed25519::public_key(&secret))
    }

    fn pack_signed(manifest: &str, payload: &[u8], secret: &[u8; 32]) -> Vec<u8> {
        let digest = content_digest(manifest.as_bytes(), payload);
        let input = signing_input(manifest.len() as u32, payload.len() as u32, &digest);
        let signature = crate::ed25519::sign(secret, &input);
        let public = crate::ed25519::public_key(secret);
        let mut out = header_signed(
            manifest.len() as u32,
            payload.len() as u32,
            &digest,
            &public,
            &signature,
        )
        .to_vec();
        out.extend_from_slice(manifest.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    const MANIFEST: &str = "name=hello-app\nversion=1.0.0\ncaps=ipc\n";

    #[test]
    fn a_signed_package_parses_and_is_trusted() {
        let (secret, public) = keypair(7);
        let bytes = pack_signed(MANIFEST, b"payload", &secret);
        let pkg = parse(&bytes).expect("signed package must parse");
        assert!(pkg.signature.is_some());
        assert_eq!(verify_trust(&pkg, &[public]), Ok(()));
    }

    #[test]
    fn an_unsigned_package_is_refused_as_unsigned_not_as_garbage() {
        let bytes = pack(MANIFEST, b"payload");
        let pkg = parse(&bytes).expect("V0.7 packages must still parse");
        assert!(pkg.signature.is_none());
        let (_, public) = keypair(7);
        assert_eq!(verify_trust(&pkg, &[public]), Err(TrustError::Unsigned));
    }

    #[test]
    fn a_signature_from_an_unknown_key_is_untrusted_not_invalid() {
        let (secret, _) = keypair(3);
        let (_, other_public) = keypair(9);
        let bytes = pack_signed(MANIFEST, b"payload", &secret);
        let pkg = parse(&bytes).unwrap();
        // The signature is perfectly valid; we simply do not know the signer.
        assert_eq!(
            verify_trust(&pkg, &[other_public]),
            Err(TrustError::UntrustedSigner)
        );
    }

    #[test]
    fn an_empty_trust_store_trusts_nothing() {
        let (secret, _) = keypair(3);
        let bytes = pack_signed(MANIFEST, b"payload", &secret);
        let pkg = parse(&bytes).unwrap();
        assert_eq!(verify_trust(&pkg, &[]), Err(TrustError::UntrustedSigner));
    }

    #[test]
    fn tampering_with_the_payload_is_caught_before_the_signature() {
        let (secret, public) = keypair(5);
        let mut bytes = pack_signed(MANIFEST, b"payload", &secret);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        // The digest covers the payload, so this is an integrity failure —
        // the signature is never reached.
        assert_eq!(parse(&bytes).unwrap_err(), PkgError::DigestMismatch);
        let _ = public;
    }

    #[test]
    fn a_forged_signature_over_intact_content_is_a_bad_signature() {
        let (secret, public) = keypair(5);
        let mut bytes = pack_signed(MANIFEST, b"payload", &secret);
        // Corrupt only the signature: content and digest stay valid, so the
        // package parses and the failure is authenticity, not integrity.
        bytes[80] ^= 0x01;
        let pkg = parse(&bytes).expect("content is still intact");
        assert_eq!(verify_trust(&pkg, &[public]), Err(TrustError::BadSignature));
    }

    #[test]
    fn a_signature_cannot_be_lifted_onto_a_different_package() {
        let (secret, public) = keypair(11);
        let a = pack_signed(MANIFEST, b"payload-A", &secret);
        let other_manifest = "name=hello-app\nversion=2.0.0\ncaps=ipc\n";
        let b = pack_signed(other_manifest, b"payload-BB", &secret);
        // Splice A's signature block into B. Both are genuinely ours, so only
        // the signature's binding to the digest and lengths can catch it.
        let mut spliced = b.clone();
        spliced[48..HEADER_LEN_SIGNED].copy_from_slice(&a[48..HEADER_LEN_SIGNED]);
        let pkg = parse(&spliced).expect("content still hashes correctly");
        assert_eq!(verify_trust(&pkg, &[public]), Err(TrustError::BadSignature));
    }

    #[test]
    fn a_truncated_signed_header_is_too_short_not_a_bad_magic() {
        let (secret, _) = keypair(2);
        let bytes = pack_signed(MANIFEST, b"payload", &secret);
        for n in 8..HEADER_LEN_SIGNED {
            assert_eq!(
                parse(&bytes[..n]).unwrap_err(),
                PkgError::TooShort,
                "len {n}"
            );
        }
    }

    #[test]
    fn signing_input_binds_the_declared_shape() {
        let digest = [0xABu8; 32];
        let a = signing_input(10, 20, &digest);
        let b = signing_input(20, 10, &digest);
        assert_ne!(a, b, "swapping the lengths must change what is signed");
        let c = signing_input(10, 20, &[0xACu8; 32]);
        assert_ne!(a, c, "a different digest must change what is signed");
        assert_eq!(&a[..22], SIGNING_CONTEXT);
    }

    #[test]
    fn trust_error_names_are_distinct() {
        // The audit trail reports these strings; two failures that share a
        // name would be indistinguishable in the evidence.
        let names = [
            TrustError::Unsigned.name(),
            TrustError::UntrustedSigner.name(),
            TrustError::BadSignature.name(),
        ];
        for (i, a) in names.iter().enumerate() {
            for b in &names[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
