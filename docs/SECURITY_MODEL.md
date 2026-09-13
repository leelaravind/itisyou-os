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

Nothing in the current runtime contains an AI component; the principle exists
now because it constrains interface design (kernel APIs must remain callable
only through narrow, deterministic, auditable paths).

## Trust boundaries

1. **Firmware/bootloader boundary** — SeaBIOS/OVMF + the rust-osdev
   bootloader run before the kernel; boot-provided data (memory map,
   framebuffer, RSDP) is validated before broad use. Boot-time-only code is
   not part of the OS runtime.
2. **Kernel trusted boundary** — all Rust `no_std` code in `kernel/`;
   the trusted computing base.
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
7. **External network boundary** *(V0.8–V0.9, verified)*: IPv4 (ARP, ICMP,
   UDP, DNS, TCP), a DHCP client and IPv6 foundations over one e1000 NIC; every
   received byte is parsed as hostile, and Ring 3 network access is a
   capability scoped to a port, re-checked on every call (NET08-*, NET09-*).

## Implemented security controls (V0.7)

- `unsafe` minimized, localized, documented with invariants; inventory
  tracked in `docs/UNSAFE_INVENTORY.md` (SEC-001).
- NX / page-permission separation where the boot path permits; no
  writable+executable mappings by design once paging is owned by the kernel.
- Panic on violated kernel invariants rather than continuing corrupted.
- No network service listens unless a program asks: the stack answers only
  ARP for its own address, ICMP/ICMPv6 echo and neighbour discovery, and TCP
  connections a program opened or is listening for (v0.9.0). No host
  filesystem sharing into the guest. QEMU launches attach only
  project-generated disposable images (SEC-002).
- Dependency and secret scans before pushes; secrets never in source, logs,
  or history.
- Capabilities are explicit default-deny bits at the kernel syscall boundary;
  spawn delegation intersects with the parent's authority and cannot amplify.
- Platform services are ordinary Ring 3 processes with declared capabilities,
  deterministic dependency ordering, bounded restart, and audited transitions.
  This holds for the V0.8 persistent services too: living for the life of the
  system buys a daemon no authority (`tickd` runs IPC-only, `flapd` with
  none), and a daemon that stops serving — even by exiting cleanly — is
  restarted under the same bounded policy and then left Failed rather than
  restarted forever.
- A syscall is ungated only when it conveys no authority: `write`, `exit`,
  `yield`, `getpid`, (V0.8) `uptime`, a free-running tick counter with no
  wall clock and nothing about any other process, and (V0.10) `args`, which
  returns only the caller's own argument block — data its launcher chose to
  hand it. Everything else is default-deny behind a capability handle;
  `spawn_args` keeps `spawn_caps`'s Process gate and delegation rule, and its
  argument block is validated (count, size, printable ASCII without spaces)
  before anything is loaded.
- Applications launch only from validated manifests and packages; filesystem
  visibility is normalized and prefix-confined.
- Updates use integrity-verified packages and crash-atomic staging/rollback;
  interrupted updates are detected and recovered after reboot.
- Packages are **authenticated as well as integrity-checked** (V0.8): an
  Ed25519 signature over a context string, the declared lengths and the content
  digest, verified against a compiled-in trust root. Integrity and authenticity
  are separate steps so a corrupt download, a package from a stranger and a
  forged signature are three distinguishable refusals. An empty trust store
  trusts nothing.
- **CPU-enforced kernel/user separation** (V0.8): SMEP, SMAP and UMIP. SMAP
  makes kernel access to user pages forbidden by default and permitted only in
  three declared windows, so a stray dereference of a user pointer faults
  instead of quietly working.
- The **audit trail is hash-chained and durable** (V0.8): each record's hash
  covers the previous one, the chain is extended before the bounded ring drops
  anything, and it continues across boots from the head recovered at startup.
  It detects editing; it does not defend against an attacker who can rewrite
  the whole file including its head. Signing the head is the missing step.
- The network stack refuses more than it accepts: fragments, VLAN tags, ICMP
  types other than echo, and packets addressed elsewhere are counted and
  dropped. It generates no ICMP errors, so it cannot be used as a reflector.
  Network access is a capability scoped to a port.

## Capability model and remaining direction

`subject → capability → object → permitted operation → constraints →
provenance` is the V0.7 platform contract. The current implementation uses
static process bit capabilities and path prefixes. Per-resource handles,
revocation, enforcement at the syscall boundary, signed package authenticity
and a port-scoped network policy have since landed in V0.8
(forged, expired and revoked handles are each refused on the real syscall
path, with the reason audited). V0.9 added key management (ADR-0021): the
kernel trusts only an offline root whose private key never enters the source
tree; signing keys are trusted through root-signed certificates that limit
each key to a package-name scope and a window of release epochs, and a
root-signed revocation list retires keys (an older list can never replace a
newer one). The fixture packages in the image are signed by a published test
key whose certificate covers only `hello-*` names, which keeps the image
reproducible without letting that key vouch for anything real.
