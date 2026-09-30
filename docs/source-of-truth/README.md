# W0-01 Source-of-Truth Freeze

Packet: **W0-01 — Freeze normative inputs**

This directory records the normative authority boundary for the v0.3.1 architecture chain. It does not reinterpret the architecture and it does not promote implementation status.

## Authority chain

1. **SPEC v0.3.1 FINAL/FROZEN** — architecture source of truth.
2. **Architecture Baseline v1.0** — accepted architecture derivation from the frozen SPEC.
3. **Implementation Plan v1.1** — execution plan derived from Baseline v1.0 only.

The exact accepted DOCX artifacts are pinned by SHA-256 in `W0-01.json` and `hashes.sha256`.

## Historical boundary

Repository architecture/implementation material that predates this freeze is historical and non-authoritative unless it is explicitly revalidated against Baseline v1.0. It must not be copied forward as an unstated default, used to fill gaps, or patched into the current architecture chain.

## Packet acceptance

W0-01 is **not self-approving**. The packet remains started until both accepted DOCX artifacts are committed at the target paths with exact hash matches and the owner accepts the resulting evidence. W0-02 is not authorized by this file alone.
