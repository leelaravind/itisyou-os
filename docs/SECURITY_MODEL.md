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
3. **Userspace boundary** *(stretch/planned)* — ring 3 with validated
   syscalls; user pointers checked before any kernel dereference; no user
   mapping of kernel pages.
4. **Privileged system-service boundary** *(future)*.
5. **Untrusted application boundary** *(future)*.
6. **AI/agent boundary** *(future — see principle above)*.
7. **External network boundary** *(future — no network stack in V0.1)*.

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
