# ADR-0021: Package-signing key hierarchy

## Status

Accepted and verified for V0.9 (KEY09-001).

## Context

V0.8 authenticated packages with Ed25519 against one compiled-in key whose seed
was published in `kernel/build.rs`. The mechanism was real; the key anchored
nothing, because anyone could sign with it. There was no way to retire a key
or limit what a key may sign, and changing the key meant a new kernel.

The open design question recorded against KEY09-001: the image's fixture
packages are signed at build time. A private release key kept outside the
tree would therefore have to be a CI secret — and then nobody without that
secret could rebuild the image bit for bit, undoing the v0.8.1
reproducibility work.

## Decision

1. **Offline root.** The kernel compiles in only the root's public key
   (`keys/root.pub.hex`). The root's private key is generated with the OS
   CSPRNG and kept outside the source tree; it is used offline
   (`tools/keytool`) to issue certificates and sign revocation lists, never by
   the build or CI.
2. **Certified signing keys.** A certificate (`ITCERT01`, domain-separated
   root signature) binds a key id and public key to a **scope** — a
   package-name prefix — and a **validity window in kernel release epochs**.
   Epochs, not dates, because the kernel has no trusted clock; a date checked
   against an attacker-settable clock would be decoration.
3. **Signed revocation** (`ITREVL01`): a root-signed list of revoked key ids
   with a sequence number; a list older than the applied one is refused, so a
   replayed list cannot un-revoke a key.
4. **Package format unchanged.** Packages still embed signer key and
   signature (`ITPKG002`); the kernel finds the signer's certificate among
   those loaded from `/etc/trust/certs`, each verified against the root at
   load. A certificate file added to the image confers nothing unless the
   root signed it.
5. **The published test key is scope-limited.** Fixture packages are signed by
   a published key certified only for names starting `hello-`, so the image
   stays reproducible by anyone and the published key cannot vouch for
   anything else. Real packages are to be signed by the release key (id 10),
   whose private key is off-tree. That resolves the open question without a
   CI secret.
6. **Refusal order**, most serious first: unsigned → bad signature → no
   certificate (untrusted signer) → revoked → outside the validity window →
   out of scope. Each is audited with its own reason.

## Consequences

- Verified in QEMU (`trust-bios`): one install and launch accepted through
  the test key's certificate, and five distinct refusals — revoked, expired,
  impostor-certified (refused at load), out of scope, uncertified.
- Rotation is an offline operation plus a new image: a new certificate and a
  higher-sequence revocation list, shipped in `/etc/trust`. There is no
  over-the-network update of trust material.
- The boot image itself is not authenticated (no Secure Boot, no measured
  boot), so an attacker who can rewrite the image can replace the kernel and
  its root along with it. The hierarchy protects package installation on a
  trusted kernel; it does not make the boot chain trustworthy.
- There is no persistent anti-rollback floor across boots for revocation
  lists: the kernel applies the list shipped in its own image.
