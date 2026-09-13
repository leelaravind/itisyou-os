//! The package-signing key hierarchy (V0.9, KEY09-001).
//!
//! V0.8 trusted exactly one signing key, compiled in, whose seed sat in the
//! source tree. That authenticates nothing: anyone can sign with a published
//! key, there is no way to retire it, and the only way to change it is a new
//! kernel. The hierarchy fixes each of those:
//!
//! * **An offline root.** Only the root's PUBLIC key is compiled into the
//!   kernel. Its private key never enters the source tree; it is used offline
//!   to certify signing keys and to sign revocation lists, and nothing else.
//! * **Certified signing keys.** A certificate binds a signing key to an id, a
//!   validity window and a SCOPE — the package names it may vouch for. A
//!   certificate the root did not sign is refused when it is loaded, so a
//!   certificate file dropped onto the image confers nothing.
//! * **Validity in release epochs, not wall-clock time.** The kernel has no
//!   trusted clock (no RTC is read, nothing authenticates the time), so a
//!   certificate's window names the kernel release epochs it is valid for. A
//!   window checked against a clock the attacker can set would be theatre.
//! * **Signed revocation.** A root-signed list of revoked key ids. A newer list
//!   replaces an older one; an older one never replaces a newer one, so an
//!   attacker cannot un-revoke a key by replaying last year's list.
//!
//! This is what lets the build keep a PUBLISHED test key — needed so anyone can
//! rebuild the image's fixture packages and reproduce it bit for bit — without
//! that key being able to vouch for anything but the fixtures: its certificate's
//! scope is the fixtures' name prefix, and the kernel enforces it.
//!
//! Wire formats (all integers little-endian):
//!
//! ```text
//! certificate        0..8    "ITCERT01"
//!                    8..12   key id
//!                    12..44  signing public key (Ed25519)
//!                    44..48  first valid epoch
//!                    48..52  last valid epoch (inclusive)
//!                    52      scope length (<= 32)
//!                    53      label length (<= 32)
//!                    54..    scope (ASCII package-name prefix; empty = any)
//!                    ..      label (ASCII, for logs)
//!                    +64     root signature over CERT_CONTEXT || the above
//!
//! revocation list    0..8    "ITREVL01"
//!                    8..12   sequence number
//!                    12..14  count (<= MAX_REVOKED)
//!                    14..    count x key id (u32)
//!                    +64     root signature over REVOCATION_CONTEXT || the above
//! ```

use crate::ed25519::{self, PUBLIC_KEY_LEN, SIGNATURE_LEN};
use crate::pkg::{self, Package, TrustError};

pub const CERT_MAGIC: &[u8; 8] = b"ITCERT01";
/// Domain separation: a root signature over a certificate can never be passed
/// off as a signature over a package or a revocation list, or vice versa.
pub const CERT_CONTEXT: &[u8] = b"ITISYOU-OS/ITCERT01/v1";
pub const REVOCATION_MAGIC: &[u8; 8] = b"ITREVL01";
pub const REVOCATION_CONTEXT: &[u8] = b"ITISYOU-OS/ITREVL01/v1";

const CERT_FIXED: usize = 54;
pub const MAX_SCOPE_LEN: usize = 32;
pub const MAX_LABEL_LEN: usize = 32;
/// Largest certificate body (before the signature).
pub const MAX_CERT_BODY: usize = CERT_FIXED + MAX_SCOPE_LEN + MAX_LABEL_LEN;
/// Largest encoded certificate.
pub const MAX_CERT_LEN: usize = MAX_CERT_BODY + SIGNATURE_LEN;
/// Certified keys a store holds.
pub const MAX_CERTS: usize = 8;
/// Key ids a revocation list may name.
pub const MAX_REVOKED: usize = 64;
const REVOCATION_FIXED: usize = 14;
/// Largest revocation-list body (before the signature).
pub const MAX_REVOCATION_BODY: usize = REVOCATION_FIXED + 4 * MAX_REVOKED;

/// Why a certificate or revocation list was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertError {
    TooShort,
    BadMagic,
    /// Declared lengths disagree with the size (truncated or trailing bytes).
    BadLength,
    /// Scope or label longer than allowed, or not printable ASCII.
    BadText,
    /// The validity window ends before it starts.
    BadWindow,
    /// Not signed by the compiled-in root.
    BadRootSignature,
    /// The store already has [`MAX_CERTS`] keys.
    StoreFull,
    /// Another certificate already claims this key id or this public key.
    Duplicate,
    /// A revocation list older than the one already applied.
    Rollback,
    TooManyRevoked,
}

impl CertError {
    /// A short stable name for logs and audit records.
    pub const fn name(self) -> &'static str {
        match self {
            CertError::TooShort => "too_short",
            CertError::BadMagic => "bad_magic",
            CertError::BadLength => "bad_length",
            CertError::BadText => "bad_text",
            CertError::BadWindow => "bad_window",
            CertError::BadRootSignature => "bad_root_signature",
            CertError::StoreFull => "store_full",
            CertError::Duplicate => "duplicate",
            CertError::Rollback => "rollback",
            CertError::TooManyRevoked => "too_many_revoked",
        }
    }
}

fn printable(text: &[u8]) -> bool {
    text.iter().all(|b| (0x21..=0x7e).contains(b))
}

/// A certified signing key as the store keeps it (owned, fixed size).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CertifiedKey {
    pub key_id: u32,
    pub public_key: [u8; PUBLIC_KEY_LEN],
    pub first_epoch: u32,
    pub last_epoch: u32,
    scope: [u8; MAX_SCOPE_LEN],
    scope_len: u8,
    label: [u8; MAX_LABEL_LEN],
    label_len: u8,
}

impl CertifiedKey {
    /// The package-name prefix this key may sign; empty means any name.
    pub fn scope(&self) -> &str {
        let bytes = self.scope.get(..self.scope_len as usize).unwrap_or(&[]);
        core::str::from_utf8(bytes).unwrap_or("")
    }

    /// Human-readable name for logs.
    pub fn label(&self) -> &str {
        let bytes = self.label.get(..self.label_len as usize).unwrap_or(&[]);
        core::str::from_utf8(bytes).unwrap_or("")
    }

    /// True when `name` falls inside this key's scope.
    pub fn covers(&self, name: &str) -> bool {
        name.starts_with(self.scope())
    }
}

/// Encode a certificate body into `out` (for offline tooling and tests); the
/// caller signs [`cert_signing_input`] of it with the root and appends the
/// signature. Returns the body length.
pub fn encode_cert_body(
    out: &mut [u8; MAX_CERT_BODY],
    key_id: u32,
    public_key: &[u8; PUBLIC_KEY_LEN],
    first_epoch: u32,
    last_epoch: u32,
    scope: &str,
    label: &str,
) -> Result<usize, CertError> {
    let (scope, label) = (scope.as_bytes(), label.as_bytes());
    if scope.len() > MAX_SCOPE_LEN
        || label.len() > MAX_LABEL_LEN
        || !printable(scope)
        || !printable(label)
    {
        return Err(CertError::BadText);
    }
    if last_epoch < first_epoch {
        return Err(CertError::BadWindow);
    }
    out[0..8].copy_from_slice(CERT_MAGIC);
    out[8..12].copy_from_slice(&key_id.to_le_bytes());
    out[12..44].copy_from_slice(public_key);
    out[44..48].copy_from_slice(&first_epoch.to_le_bytes());
    out[48..52].copy_from_slice(&last_epoch.to_le_bytes());
    out[52] = scope.len() as u8;
    out[53] = label.len() as u8;
    let s_end = CERT_FIXED + scope.len();
    out[CERT_FIXED..s_end].copy_from_slice(scope);
    out[s_end..s_end + label.len()].copy_from_slice(label);
    Ok(s_end + label.len())
}

/// The bytes the root signs for a certificate: the context, then the body.
pub fn cert_signing_input<'a>(
    body: &[u8],
    buf: &'a mut [u8; CERT_CONTEXT.len() + MAX_CERT_BODY],
) -> &'a [u8] {
    let n = body.len().min(MAX_CERT_BODY);
    buf[..CERT_CONTEXT.len()].copy_from_slice(CERT_CONTEXT);
    buf[CERT_CONTEXT.len()..CERT_CONTEXT.len() + n].copy_from_slice(&body[..n]);
    &buf[..CERT_CONTEXT.len() + n]
}

/// Parse a certificate and check the root's signature over it.
pub fn verify_certificate(
    bytes: &[u8],
    root: &[u8; PUBLIC_KEY_LEN],
) -> Result<CertifiedKey, CertError> {
    if bytes.len() < CERT_FIXED + SIGNATURE_LEN {
        return Err(CertError::TooShort);
    }
    if &bytes[0..8] != CERT_MAGIC {
        return Err(CertError::BadMagic);
    }
    let (scope_len, label_len) = (bytes[52] as usize, bytes[53] as usize);
    if scope_len > MAX_SCOPE_LEN || label_len > MAX_LABEL_LEN {
        return Err(CertError::BadText);
    }
    let body_len = CERT_FIXED + scope_len + label_len;
    if bytes.len() != body_len + SIGNATURE_LEN {
        return Err(CertError::BadLength);
    }
    let (body, sig) = bytes.split_at(body_len);
    let signature: [u8; SIGNATURE_LEN] = sig.try_into().map_err(|_| CertError::BadLength)?;
    let mut buf = [0u8; CERT_CONTEXT.len() + MAX_CERT_BODY];
    if !ed25519::verify(root, cert_signing_input(body, &mut buf), &signature) {
        return Err(CertError::BadRootSignature);
    }
    let u32_at = |i: usize| u32::from_le_bytes([body[i], body[i + 1], body[i + 2], body[i + 3]]);
    let scope = &body[CERT_FIXED..CERT_FIXED + scope_len];
    let label = &body[CERT_FIXED + scope_len..body_len];
    if !printable(scope) || !printable(label) {
        return Err(CertError::BadText);
    }
    let (first_epoch, last_epoch) = (u32_at(44), u32_at(48));
    if last_epoch < first_epoch {
        return Err(CertError::BadWindow);
    }
    let mut key = CertifiedKey {
        key_id: u32_at(8),
        public_key: [0; PUBLIC_KEY_LEN],
        first_epoch,
        last_epoch,
        scope: [0; MAX_SCOPE_LEN],
        scope_len: scope_len as u8,
        label: [0; MAX_LABEL_LEN],
        label_len: label_len as u8,
    };
    key.public_key.copy_from_slice(&body[12..44]);
    key.scope[..scope_len].copy_from_slice(scope);
    key.label[..label_len].copy_from_slice(label);
    Ok(key)
}

/// Encode a revocation-list body into `out`; returns its length.
pub fn encode_revocation_body(
    out: &mut [u8; MAX_REVOCATION_BODY],
    sequence: u32,
    revoked: &[u32],
) -> Result<usize, CertError> {
    if revoked.len() > MAX_REVOKED {
        return Err(CertError::TooManyRevoked);
    }
    out[0..8].copy_from_slice(REVOCATION_MAGIC);
    out[8..12].copy_from_slice(&sequence.to_le_bytes());
    out[12..14].copy_from_slice(&(revoked.len() as u16).to_le_bytes());
    for (i, id) in revoked.iter().enumerate() {
        let at = REVOCATION_FIXED + 4 * i;
        out[at..at + 4].copy_from_slice(&id.to_le_bytes());
    }
    Ok(REVOCATION_FIXED + 4 * revoked.len())
}

/// The bytes the root signs for a revocation list.
pub fn revocation_signing_input<'a>(
    body: &[u8],
    buf: &'a mut [u8; REVOCATION_CONTEXT.len() + MAX_REVOCATION_BODY],
) -> &'a [u8] {
    let n = body.len().min(MAX_REVOCATION_BODY);
    buf[..REVOCATION_CONTEXT.len()].copy_from_slice(REVOCATION_CONTEXT);
    buf[REVOCATION_CONTEXT.len()..REVOCATION_CONTEXT.len() + n].copy_from_slice(&body[..n]);
    &buf[..REVOCATION_CONTEXT.len() + n]
}

/// A verified revocation list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revocations {
    pub sequence: u32,
    ids: [u32; MAX_REVOKED],
    count: usize,
}

impl Revocations {
    /// No list applied yet: sequence 0, nothing revoked.
    pub const EMPTY: Revocations = Revocations {
        sequence: 0,
        ids: [0; MAX_REVOKED],
        count: 0,
    };

    pub fn revoked(&self) -> &[u32] {
        &self.ids[..self.count]
    }

    pub fn contains(&self, key_id: u32) -> bool {
        self.revoked().contains(&key_id)
    }
}

/// Parse a revocation list and check the root's signature over it.
pub fn verify_revocations(
    bytes: &[u8],
    root: &[u8; PUBLIC_KEY_LEN],
) -> Result<Revocations, CertError> {
    if bytes.len() < REVOCATION_FIXED + SIGNATURE_LEN {
        return Err(CertError::TooShort);
    }
    if &bytes[0..8] != REVOCATION_MAGIC {
        return Err(CertError::BadMagic);
    }
    let count = u16::from_le_bytes([bytes[12], bytes[13]]) as usize;
    if count > MAX_REVOKED {
        return Err(CertError::TooManyRevoked);
    }
    let body_len = REVOCATION_FIXED + 4 * count;
    if bytes.len() != body_len + SIGNATURE_LEN {
        return Err(CertError::BadLength);
    }
    let (body, sig) = bytes.split_at(body_len);
    let signature: [u8; SIGNATURE_LEN] = sig.try_into().map_err(|_| CertError::BadLength)?;
    let mut buf = [0u8; REVOCATION_CONTEXT.len() + MAX_REVOCATION_BODY];
    if !ed25519::verify(root, revocation_signing_input(body, &mut buf), &signature) {
        return Err(CertError::BadRootSignature);
    }
    let mut list = Revocations::EMPTY;
    list.sequence = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
    let (ids, _) = body[REVOCATION_FIXED..].as_chunks::<4>();
    for (slot, id) in list.ids.iter_mut().zip(ids) {
        *slot = u32::from_le_bytes(*id);
    }
    list.count = count;
    Ok(list)
}

/// The kernel's view of who may sign packages: the root, the certificates it
/// has accepted, the newest revocation list, and the current release epoch.
#[derive(Debug, Clone)]
pub struct TrustStore {
    root: [u8; PUBLIC_KEY_LEN],
    epoch: u32,
    keys: [Option<CertifiedKey>; MAX_CERTS],
    revocations: Revocations,
}

impl TrustStore {
    /// A store anchored at `root` for kernel release `epoch`. It trusts
    /// nothing until certificates are added — an empty store refusing every
    /// package is the safe failure.
    pub const fn new(root: [u8; PUBLIC_KEY_LEN], epoch: u32) -> TrustStore {
        TrustStore {
            root,
            epoch,
            keys: [None; MAX_CERTS],
            revocations: Revocations::EMPTY,
        }
    }

    pub fn root(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.root
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Accepted certificates, in the order they were added.
    pub fn keys(&self) -> impl Iterator<Item = &CertifiedKey> {
        self.keys.iter().flatten()
    }

    pub fn revocations(&self) -> &Revocations {
        &self.revocations
    }

    /// Verify a certificate against the root and add it. Returns the key.
    pub fn add_certificate(&mut self, bytes: &[u8]) -> Result<CertifiedKey, CertError> {
        let key = verify_certificate(bytes, &self.root)?;
        if self
            .keys()
            .any(|k| k.key_id == key.key_id || k.public_key == key.public_key)
        {
            return Err(CertError::Duplicate);
        }
        let slot = self
            .keys
            .iter_mut()
            .find(|k| k.is_none())
            .ok_or(CertError::StoreFull)?;
        *slot = Some(key);
        Ok(key)
    }

    /// Verify a revocation list and apply it, unless it is older than the one
    /// already applied (replaying an old list must not un-revoke a key).
    /// Re-applying the current sequence is allowed and changes nothing.
    pub fn apply_revocations(&mut self, bytes: &[u8]) -> Result<u32, CertError> {
        let list = verify_revocations(bytes, &self.root)?;
        if list.sequence < self.revocations.sequence {
            return Err(CertError::Rollback);
        }
        self.revocations = list;
        Ok(list.sequence)
    }

    /// Decide whether `package` is trusted, returning the key that vouched
    /// for it. Checks run most-serious-first, so the reason reported is the
    /// one an operator most needs: a forged signature before an unknown
    /// signer, a revoked key before an expired one.
    pub fn verify_package(&self, package: &Package<'_>) -> Result<CertifiedKey, TrustError> {
        let Some(block) = package.signature else {
            return Err(TrustError::Unsigned);
        };
        let input = pkg::signing_input(
            package.manifest_raw.len() as u32,
            package.payload.len() as u32,
            &package.digest,
        );
        if !ed25519::verify(&block.public_key, &input, &block.signature) {
            return Err(TrustError::BadSignature);
        }
        let key = *self
            .keys()
            .find(|k| k.public_key == block.public_key)
            .ok_or(TrustError::UntrustedSigner)?;
        if self.revocations.contains(key.key_id) {
            return Err(TrustError::RevokedSigner);
        }
        if self.epoch < key.first_epoch {
            return Err(TrustError::CertificateNotYetValid);
        }
        if self.epoch > key.last_epoch {
            return Err(TrustError::CertificateExpired);
        }
        if !key.covers(package.manifest.name) {
            return Err(TrustError::OutOfScope);
        }
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: [u8; 32] = *b"test root seed ---------------01";
    const OTHER_ROOT: [u8; 32] = *b"an impostor's root seed -------2";
    const SIGNER: [u8; 32] = *b"a fixture signer seed ---------3";
    const SIGNER_B: [u8; 32] = *b"a second signer seed ----------4";

    fn cert(
        root: &[u8; 32],
        id: u32,
        signer: &[u8; 32],
        window: (u32, u32),
        scope: &str,
    ) -> Vec<u8> {
        let mut body = [0u8; MAX_CERT_BODY];
        let pk = ed25519::public_key(signer);
        let n = encode_cert_body(&mut body, id, &pk, window.0, window.1, scope, "label").unwrap();
        let mut buf = [0u8; CERT_CONTEXT.len() + MAX_CERT_BODY];
        let sig = ed25519::sign(root, cert_signing_input(&body[..n], &mut buf));
        let mut out = body[..n].to_vec();
        out.extend_from_slice(&sig);
        out
    }

    fn revocations(root: &[u8; 32], seq: u32, ids: &[u32]) -> Vec<u8> {
        let mut body = [0u8; MAX_REVOCATION_BODY];
        let n = encode_revocation_body(&mut body, seq, ids).unwrap();
        let mut buf = [0u8; REVOCATION_CONTEXT.len() + MAX_REVOCATION_BODY];
        let sig = ed25519::sign(root, revocation_signing_input(&body[..n], &mut buf));
        let mut out = body[..n].to_vec();
        out.extend_from_slice(&sig);
        out
    }

    fn package(name: &str, signer: &[u8; 32]) -> Vec<u8> {
        let manifest = format!("name={name}\nversion=1\ncaps=fs_read\n");
        let payload = b"payload bytes";
        let digest = pkg::content_digest(manifest.as_bytes(), payload);
        let input = pkg::signing_input(manifest.len() as u32, payload.len() as u32, &digest);
        let sig = ed25519::sign(signer, &input);
        let pk = ed25519::public_key(signer);
        let mut out = pkg::header_signed(
            manifest.len() as u32,
            payload.len() as u32,
            &digest,
            &pk,
            &sig,
        )
        .to_vec();
        out.extend_from_slice(manifest.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn store() -> TrustStore {
        let mut s = TrustStore::new(ed25519::public_key(&ROOT), 9);
        s.add_certificate(&cert(&ROOT, 1, &SIGNER, (9, 10), "hello-"))
            .unwrap();
        s
    }

    fn check(s: &TrustStore, name: &str, signer: &[u8; 32]) -> Result<u32, TrustError> {
        let bytes = package(name, signer);
        let p = pkg::parse(&bytes).unwrap();
        s.verify_package(&p).map(|k| k.key_id)
    }

    #[test]
    fn a_certified_key_signs_inside_its_scope_and_window() {
        let s = store();
        assert_eq!(check(&s, "hello-app", &SIGNER), Ok(1));
        let key = s.keys().next().unwrap();
        assert_eq!((key.scope(), key.label()), ("hello-", "label"));
    }

    #[test]
    fn a_key_without_a_certificate_is_untrusted() {
        assert_eq!(
            check(&store(), "hello-app", &SIGNER_B),
            Err(TrustError::UntrustedSigner)
        );
    }

    #[test]
    fn scope_is_enforced() {
        assert_eq!(
            check(&store(), "other-app", &SIGNER),
            Err(TrustError::OutOfScope)
        );
        // An empty scope covers every name.
        let mut s = TrustStore::new(ed25519::public_key(&ROOT), 9);
        s.add_certificate(&cert(&ROOT, 7, &SIGNER_B, (1, 99), ""))
            .unwrap();
        assert_eq!(check(&s, "anything", &SIGNER_B), Ok(7));
    }

    #[test]
    fn the_validity_window_is_checked_against_the_release_epoch() {
        let mut s = TrustStore::new(ed25519::public_key(&ROOT), 9);
        s.add_certificate(&cert(&ROOT, 1, &SIGNER, (7, 8), ""))
            .unwrap();
        s.add_certificate(&cert(&ROOT, 2, &SIGNER_B, (10, 12), ""))
            .unwrap();
        assert_eq!(check(&s, "x", &SIGNER), Err(TrustError::CertificateExpired));
        assert_eq!(
            check(&s, "x", &SIGNER_B),
            Err(TrustError::CertificateNotYetValid)
        );
        // Boundaries are inclusive.
        let mut edge = TrustStore::new(ed25519::public_key(&ROOT), 9);
        edge.add_certificate(&cert(&ROOT, 1, &SIGNER, (9, 9), ""))
            .unwrap();
        assert_eq!(check(&edge, "x", &SIGNER), Ok(1));
    }

    #[test]
    fn a_certificate_the_root_did_not_sign_is_refused() {
        let mut s = TrustStore::new(ed25519::public_key(&ROOT), 9);
        let forged = cert(&OTHER_ROOT, 1, &SIGNER, (9, 10), "");
        assert_eq!(s.add_certificate(&forged), Err(CertError::BadRootSignature));
        // Editing a genuine certificate (widening its scope) breaks it too.
        let mut widened = cert(&ROOT, 1, &SIGNER, (9, 10), "hello-");
        widened[52] = 0; // scope length 0 = "any name"...
        assert!(s.add_certificate(&widened).is_err());
        let mut extended = cert(&ROOT, 1, &SIGNER, (9, 10), "hello-");
        extended[48] = 0xff; // ...or a later last epoch
        assert_eq!(
            s.add_certificate(&extended),
            Err(CertError::BadRootSignature)
        );
        assert_eq!(s.keys().count(), 0);
    }

    #[test]
    fn revocation_beats_a_valid_certificate() {
        let mut s = store();
        assert_eq!(s.apply_revocations(&revocations(&ROOT, 1, &[1])), Ok(1));
        assert_eq!(
            check(&s, "hello-app", &SIGNER),
            Err(TrustError::RevokedSigner)
        );
        assert_eq!(s.revocations().revoked(), &[1]);
    }

    #[test]
    fn an_older_revocation_list_cannot_unrevoke() {
        let mut s = store();
        s.apply_revocations(&revocations(&ROOT, 5, &[1])).unwrap();
        assert_eq!(
            s.apply_revocations(&revocations(&ROOT, 4, &[])),
            Err(CertError::Rollback)
        );
        assert_eq!(
            check(&s, "hello-app", &SIGNER),
            Err(TrustError::RevokedSigner)
        );
        // A newer list can lift a revocation — that is the root's decision.
        s.apply_revocations(&revocations(&ROOT, 6, &[])).unwrap();
        assert_eq!(check(&s, "hello-app", &SIGNER), Ok(1));
    }

    #[test]
    fn a_revocation_list_the_root_did_not_sign_is_refused() {
        let mut s = store();
        assert_eq!(
            s.apply_revocations(&revocations(&OTHER_ROOT, 9, &[1])),
            Err(CertError::BadRootSignature)
        );
        let mut tampered = revocations(&ROOT, 2, &[1]);
        tampered[14] = 2; // revoke key 2 instead of key 1
        assert_eq!(
            s.apply_revocations(&tampered),
            Err(CertError::BadRootSignature)
        );
        assert_eq!(s.revocations().sequence, 0);
    }

    #[test]
    fn signature_failures_are_reported_before_trust_failures() {
        let s = store();
        let mut bytes = package("hello-app", &SIGNER);
        bytes[80] ^= 1;
        let p = pkg::parse(&bytes).unwrap();
        assert_eq!(s.verify_package(&p), Err(TrustError::BadSignature));
        let unsigned = {
            let manifest = "name=hello-app\nversion=1\ncaps=fs_read\n";
            let digest = pkg::content_digest(manifest.as_bytes(), b"p");
            let mut v = pkg::header(manifest.len() as u32, 1, &digest).to_vec();
            v.extend_from_slice(manifest.as_bytes());
            v.push(b'p');
            v
        };
        let p = pkg::parse(&unsigned).unwrap();
        assert_eq!(s.verify_package(&p), Err(TrustError::Unsigned));
    }

    #[test]
    fn duplicates_and_capacity_are_bounded() {
        let mut s = store();
        assert_eq!(
            s.add_certificate(&cert(&ROOT, 1, &SIGNER_B, (9, 9), "")),
            Err(CertError::Duplicate)
        );
        assert_eq!(
            s.add_certificate(&cert(&ROOT, 2, &SIGNER, (9, 9), "")),
            Err(CertError::Duplicate)
        );
        for id in 2..=MAX_CERTS as u32 {
            let seed = [id as u8; 32];
            s.add_certificate(&cert(&ROOT, id, &seed, (9, 9), ""))
                .unwrap();
        }
        let seed = [0xEE; 32];
        assert_eq!(
            s.add_certificate(&cert(&ROOT, 99, &seed, (9, 9), "")),
            Err(CertError::StoreFull)
        );
    }

    #[test]
    fn hostile_encodings_are_refused_without_panicking() {
        let root = ed25519::public_key(&ROOT);
        let good = cert(&ROOT, 1, &SIGNER, (9, 10), "hello-");
        for len in 0..good.len() {
            assert!(verify_certificate(&good[..len], &root).is_err());
        }
        let mut long = good.clone();
        long.push(0);
        assert_eq!(verify_certificate(&long, &root), Err(CertError::BadLength));
        let mut huge_scope = good.clone();
        huge_scope[52] = 200;
        assert_eq!(
            verify_certificate(&huge_scope, &root),
            Err(CertError::BadText)
        );
        let good_rev = revocations(&ROOT, 1, &[1, 2, 3]);
        for len in 0..good_rev.len() {
            assert!(verify_revocations(&good_rev[..len], &root).is_err());
        }
        let mut many = good_rev.clone();
        many[12..14].copy_from_slice(&1000u16.to_le_bytes());
        assert_eq!(
            verify_revocations(&many, &root),
            Err(CertError::TooManyRevoked)
        );
        let mut body = [0u8; MAX_CERT_BODY];
        let pk = [0u8; 32];
        assert_eq!(
            encode_cert_body(&mut body, 1, &pk, 5, 4, "", ""),
            Err(CertError::BadWindow)
        );
        assert_eq!(
            encode_cert_body(&mut body, 1, &pk, 1, 2, "has space", ""),
            Err(CertError::BadText)
        );
    }
}
