# ADR-0016: Package authenticity — Ed25519 signatures and a compiled-in trust root

## Status

Accepted and verified for V0.8 (`platform-bios`).

## Context

V0.7 packages were integrity-protected by a SHA-256 digest. That answers "did
these bytes arrive undamaged?" and says nothing about "who made them" — a
digest is computed by whoever produced the file, so an attacker who can supply
a package can supply a matching digest. ADR-0012 deferred authenticity because
it needs a key, and a key needs somewhere to live.

## Decision

**Ed25519, implemented in `kernel-core`.** No dependency: the crate already
compiles unchanged into the kernel and into host tools, allocates nothing, and
is host-tested — the same reasons SHA-256 lives there. The implementation is
validated against RFC 8032 §7.1 vectors (public keys, signatures and
verification), not only against itself, because a self-consistent
implementation of the wrong thing is exactly the failure mode to rule out.

Every curve constant is **derived rather than transcribed**: `d = -121665/121666`,
`sqrt(-1) = 2^((p-1)/4)`, and the base point from `y = 4/5`. A mistyped 32-byte
constant produces a working implementation of a different curve, which passes
every round-trip test and rejects every genuine signature. Deriving them makes
that impossible.

**Integrity and authenticity are separate steps.** `pkg::parse` answers "are
these bytes a well-formed, undamaged package?"; `pkg::verify_trust` answers
"did someone we trust vouch for them?". Fusing them would collapse a corrupt
download, a package from a stranger and a forged signature into one refusal,
and those call for three different responses. The audit trail names which:
`unsigned`, `untrusted_signer`, `bad_signature`.

**What the signature covers.** A context string, the declared manifest and
payload lengths, and the content digest. The context stops a signature over
some other 32-byte value being presented as a package signature; the lengths
stop one being lifted onto a package that claims a different shape.

**The trust root is compiled in.** A root stored in the mutable package store
could be replaced by the thing it is supposed to protect. The kernel's key is
`include_bytes!`d from a file `build.rs` derives from the signing seed, so the
trust root and the signer cannot drift apart — a hand-copied key that had
drifted would refuse every package as `untrusted_signer`, which looks exactly
like the check working correctly.

**An empty trust store trusts nothing.** A system that accepts anything when no
keys are configured fails silently in the worst direction.

## Consequences

Signatures are re-verified at launch, not only at install: the store is
mutable, so a package that was trustworthy when installed is not automatically
trustworthy when run.

`ITPKG001` (V0.7, unsigned) still parses. An old package therefore reports
`unsigned` rather than `bad magic` — "we know exactly what this is and we will
not install it" is more useful than "unrecognised bytes".

## The development key is published, deliberately

The current trust root is a development key whose seed is a literal in
`kernel/build.rs`. It is not a secret and is not presented as one: a build-time
key that *looked* secret would invite someone to trust it. The mechanism is
real and enforced; the key it anchors is not. Shipping to real users requires
provisioning a signing key that never enters the source tree, and that step is
recorded in `docs/KNOWN_LIMITATIONS.md` rather than implied to be done.

No key rotation, revocation list, or expiry exists yet. The trust store is a
static array of one key.

## Evidence

`artifacts/qemu/platform-bios.result.json`: a trusted package installs and
launches (`signature result=ok signer=…`), and three intact packages are
refused for three distinct reasons — `unsigned`, `untrusted_signer`,
`bad_signature` — while a corrupted one is still refused earlier, on integrity,
as `DigestMismatch`. Host tests: 24 for the curve, 14 for the package format.
