# Historical Architecture Boundary

This directory marks the authority boundary introduced by **W0-01**.

The repository state at and before base commit `84d6b24b11036297173004ea972a1a05ba24da88` contains pre-Baseline-v1.0 architecture and implementation material. That material is retained as historical evidence only.

## Read-only rule

Historical material is **non-authoritative** for the Private AI-Native Phone OS architecture now governed by SPEC v0.3.1 FINAL/FROZEN → Architecture Baseline v1.0 → Implementation Plan v1.1.

Do not:
- patch an old architecture/plan into the current chain;
- use old documents to fill a gap in Baseline v1.0 or Plan v1.1;
- treat prior implementation status as proof that a new work packet has passed;
- silently import old assumptions about kernel ownership, policy authority, backup semantics, networking, inference ordering, or licensing.

If old material is consulted for historical comparison, any reused fact must be revalidated against the current Baseline and the real target/source tree before it can influence implementation.

This marker does not delete or rewrite historical files. Their preservation is intentional.
