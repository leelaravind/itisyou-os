# Package-signing trust material (public)

Everything in this directory is **public** data. No private key is here, and
none may ever be committed (`scripts/secret-scan.ps1` runs before every push).
The design is in `docs/adr/0021-signing-key-hierarchy.md`; the verifier is
`crates/kernel-core/src/trust.rs`; the offline tool is `tools/keytool`.

| File | What it is |
|---|---|
| `root.pub.hex` | Public key of the offline **root**. Compiled into the kernel (`kernel/build.rs` → `TRUST_ROOT`). The root's private key is kept outside the source tree by the owner and is used only offline, to issue certificates and revocation lists. |
| `certs/test-signer.cert` | Key id 1. The **published** test key (seed in `kernel/build.rs`), certified for package names starting `hello-`, epochs 9..=10. It signs the image's fixture packages, which is what keeps the image reproducible by anyone — and its scope is why publishing it is safe. |
| `certs/release-signer.cert` | Key id 10. The **release** key, any package name, epochs 9..=12. Its private key is outside the source tree; no package in the image is signed by it yet. |
| `certs/retired-signer.cert` | Key id 2. A test fixture: validly certified, then revoked in `revocations.bin`. |
| `certs/expired-signer.cert` | Key id 3. A test fixture: certified only for epochs 7..=8, so expired on a V0.9 kernel (epoch 9). |
| `certs/rogue-signer.cert` | Key id 4. A test fixture: a certificate signed by an **impostor** root. The kernel refuses it when loading `/etc/trust` (`bad_root_signature`). |
| `revocations.bin` | Root-signed revocation list, sequence 1, revoking key id 2. |

Check any file against the root:

```sh
cargo run -q -p keytool -- check "$(cat keys/root.pub.hex)" keys/certs/test-signer.cert
```

Rotating a signing key: issue a certificate for the new key (`keytool cert`),
then publish a revocation list with a higher sequence number naming the old
key id (`keytool revoke`). The kernel refuses a revocation list older than the
one it has applied. Validity windows are in kernel **release epochs**
(`TRUST_EPOCH` in `kernel/src/platform.rs`), not dates: the kernel has no
trusted clock.
