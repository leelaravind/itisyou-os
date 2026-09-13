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
   syscalls; user pointers checked before any kernel dereference (in
   v0.9.0 for being mapped only — see SEC10-001 in KNOWN_LIMITATIONS); no user
   mapping of kernel pages; per-process page tables; GUI window ownership;
   mediated device access.
4. **Privileged system-service boundary** *(V0.7, verified; V0.10 init)* —
   services are ordinary Ring 3 processes with EXACTLY their declared
   capabilities; no hidden privileged daemons. Since V0.10 the persistent ones
   are started by `/sbin/init` (pid 1) from `/etc/init.conf`: init's own
   authority is fixed by the kernel (spawn, IPC, `fs_read` under `/etc`,
   service reporting — 0x413), a service gets at most that, and the kernel
   checks every report init makes (the pid is init's own child, the caps
   printed are the kernel's record) and attributes it (`supervisor=`).
5. **Untrusted application boundary** *(V0.7, verified)* — apps launch only
   through the platform with `manifest ∩ launcher` capabilities (default
   deny) under an FS sandbox (normalized-path prefixes; traversal-proof);
   denied access is machine-verified as denied, and every denial is audited.
6. **AI/agent boundary** *(future — see principle above)* — the V0.7
   request → capability check → deterministic service → action → audit path
   IS the pipeline an agent will use; agents get no other entry point.
7. **Console and desktop input** *(V0.10, verified)*: one owner of the
   console's input at a time, granted only by the kernel (`rsh`), returned on
   every exit path; a Ring 3 app reads events only for windows it owns, and
   keys go only to the focused window's owner. A process holds at most four
   windows.
8. **External network boundary** *(V0.8–V0.9, verified)*: IPv4 (ARP, ICMP,
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
  the whole file including its head — the V0.9 witness anchor is what catches
  that. Since V0.11 the stored trail is the newest 128 records with the head
  its first record extends (`base`), and the recovered records stay in the
  next save — v0.8.0–v0.10.0 stored only the current boot's ring under a head
  covering everything, so an untouched trail read as tampered after a second
  boot or a busy one (AUDIT11-001). `audit verify` is read-only: the running
  kernel's copy of what it saved or recovered is the reference.
- **The evidence is kernel-owned** (V0.11, SEC11-001): the audit trail and the
  package store are refused to the `fs_*` syscalls, whatever capability the
  program holds — holding `fs_write` is authority over a program's files, not
  over the record of what programs did or over which version of an
  application runs. Text a program or a disk chose (file names, contents,
  application names) is echoed as one line with control characters escaped
  and the marker prefix neutralized, and names with control characters are
  refused (AUDIT11-002), so it cannot pose as kernel evidence.
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
