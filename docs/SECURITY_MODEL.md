# Security model — ITISYOU OS

## Central principle

**AI has intelligence, not authority.** Any future AI/agent layer reasons
about the system and proposes actions; it never executes privileged kernel
operations directly. The intended pipeline:

```
agent intent → policy engine → risk/permission evaluation
  → user approval (where required) → deterministic privileged service
  → kernel operation → verification → audit → rollback/recovery
```

Nothing in the V0.1 runtime contains an AI component; the principle exists
now because it constrains interface design (kernel APIs must remain callable
only through narrow, deterministic, auditable paths).

## Trust boundaries

1. **Firmware/bootloader boundary** — SeaBIOS/OVMF + the rust-osdev
   bootloader run before the kernel; boot-provided data (memory map,
   framebuffer, RSDP) is validated before broad use. Boot-time-only code is
   not part of the OS runtime.
2. **Kernel trusted boundary** — all Rust `no_std` code in `kernel/`;
   the trusted computing base for V0.1.
3. **Userspace boundary** *(V0.2–V0.6, verified)* — ring 3 with validated
   syscalls; user pointers checked before any kernel dereference; no user
   mapping of kernel pages; per-process page tables; GUI window ownership;
   mediated device access.
4. **Privileged system-service boundary** *(V0.7, verified)* — services are
   ordinary Ring 3 processes supervised by the kernel with EXACTLY their
   declared capabilities; no hidden privileged daemons; every state
   transition diagnosed and the supervisor's actions audited.
5. **Untrusted application boundary** *(V0.7, verified)* — apps launch only
   through the platform with `manifest ∩ launcher` capabilities (default
   deny) under an FS sandbox (normalized-path prefixes; traversal-proof);
   denied access is machine-verified as denied, and every denial is audited.
6. **AI/agent boundary** *(future — see principle above)* — the V0.7
   request → capability check → deterministic service → action → audit path
   IS the pipeline an agent will use; agents get no other entry point.
7. **External network boundary** *(future — no network stack yet)*.

## V0.1 concrete requirements (plan §11.2)

- `unsafe` minimized, localized, documented with invariants; inventory
  tracked in `docs/UNSAFE_INVENTORY.md` (SEC-001).
- NX / page-permission separation where the boot path permits; no
  writable+executable mappings by design once paging is owned by the kernel.
- Panic on violated kernel invariants rather than continuing corrupted.
- No network services. No host filesystem sharing into the guest. QEMU
  launches attach only project-generated disposable images (SEC-002).
- Dependency and secret scans before pushes; secrets never in source, logs,
  or history.

## Future capability model (documented direction, not implemented)

`subject → capability → object → permitted operation → constraints →
provenance` — capabilities explicit, inspectable, revocable, and narrower
than blanket administrator privilege. Recorded here so V0.2+ interfaces
grow toward it instead of retrofitting.
