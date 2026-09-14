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

Released versions up to v0.10.0 contain no AI component; the principle
constrained their interface design (kernel APIs callable only through
narrow, deterministic, auditable paths). V0.11 (in development, ADR-0024)
implements the pipeline for two actions: the agent's intent is a proposal
(`propose`, syscall 43); the policy is `kernel_core::policy` plus checks the
kernel makes from its own knowledge; permission is the console-only
`propose` capability, and risk and reversibility are fixed per action and
shown in the `proposals` listing; approval is always required and only the
console gives it; the privileged service is kernel code (for
`retry-service`, `/sbin/init` asked through a kernel mailbox); verification
is a post-condition checked over a fixed window; filing, refusal, approval,
execution, verification and rollback are audited; a failed check is rolled
back. See boundary 6.

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
   Since V0.11 (in development) init also reads the kernel's one-slot
   mailbox (`init_ctl`, syscall 44) with the Service ADMIN right it already
   holds; every other caller is refused (`not_init`). `/bin/inferd` is one
   of its services, with IPC and `fs_read` under `/etc` and nothing else.
5. **Untrusted application boundary** *(V0.7, verified)* — apps launch only
   through the platform with `manifest ∩ launcher` capabilities (default
   deny) under an FS sandbox (normalized-path prefixes; traversal-proof);
   denied access is machine-verified as denied, and every denial is audited.
6. **AI/agent boundary** *(V0.11, in development; ADR-0024)* — the agent
   (`/bin/agent`) and the inference service (`/bin/inferd`) are ordinary
   Ring 3 programs with intelligence and no authority. The agent holds the
   view, `propose`, IPC and reads under `/etc/ai`; no syscall resumes the
   scheduler or retries a service, and its direct attempts at
   `svc_report`, `fs_write` and `spawn` are refused. Its one effect is a
   proposal, which the kernel checks against
   what it knows itself and which does nothing until the console approves
   it. inferd's answer is a claim the kernel does not rely on. The controls
   are listed below.
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
  application names, a program's path) is echoed as one line with control
  characters and the invisible reordering characters escaped and the marker
  prefix neutralized, names with either are refused (AUDIT11-002), and a
  process is recorded by the canonical path of the file it runs, never the
  string a program passed to `spawn` (SEC11-002), so it cannot pose as
  kernel evidence. Process output is rewritten across the pieces it is
  written in, so no split of the prefix survives (OUT11-002). A stored trail
  over 256 KiB or 128 records is not read, and a tampered one stays reported
  until the operator removes it (AUDIT11-003).
- **An agent has intelligence, not authority** (V0.11, in development;
  AGENT11-001, PROP11-001). `/bin/agent` runs with the view, `propose`,
  IPC and reads under `/etc/ai` only — no CONTROL, ADMIN or
  filesystem-write right — and neither action it may propose has a
  syscall. With its authority `svc_report`, `fs_write` and `spawn` are
  refused (`AIPROBE-DIRECT-ALL-DENIED`). It can file a proposal; nothing it
  files acts.
- **Console-only capabilities** (VIEW11-001, PROP11-001): `sys_view` (bit
  11 → SystemAdministration READ) and `propose` (bit 12 → USE) are granted
  only when a kernel console command (`run`, `bg`, `rsh`) names them. They
  are outside `CAP_LEGACY_FULL`, so a launch without a caps list, desktop
  apps and `pkg launch` never get them; `caps::delegate` drops them, so
  neither a child (`spawn`, `spawn_caps`, `spawn_args`) nor a package
  manifest obtains them; `/etc/init.conf` naming one is refused
  (`console_only_capability`); and rights are exact, so the ADMIN right
  `sys_admin` carries implies neither.
- **The kernel checks every claim itself** (PROP11-001). A proposal is
  filed only if its model digest is the one compiled into the kernel, its
  view digest is that of the last view served to the submitting process and
  at most 5 s old, recomputing from those exact bytes with the kernel's own
  copy of the shipped model fires the cited condition, and the action
  applies now by facts the kernel gathers itself. inferd's answer and the
  agent's diagnosis are claims; IPC carries no sender, and the kernel never
  relies on either. The model's output can cause a refusal, never an
  action. Agent-supplied names are validated (`[a-z0-9-]{1,15}`) before
  they reach any line.
- **Two actions, both reversible, verified and rolled back** (ACT11-001,
  ACT11-002): `resume-scheduler` and `retry-service`, each executed by
  kernel code only after the console's `approve <id>` — there is no
  automatic path — and only after the kernel re-checks the TTL and that
  the action still applies (`precondition_changed`). Each is verified
  against a post-condition over a fixed window and rolled back when the
  check fails; the lifecycle is one-way, and a denied or expired proposal
  can never run.
- **The init mailbox** (ACT11-002): the kernel executes `retry-service`
  through the service's supervisor, never behind its back. One command at
  a time, posted only by the kernel's `approve` path, read and
  acknowledged only by the live init (`init_ctl`, syscall 44; anyone else
  `not_init`, audited); an acknowledgement must name the command init
  fetched; the mailbox is cleared when init dies, so a command never
  reaches its successor. Init gets no new capability bit and still cannot
  kill: on a rollback the kernel kills the instance init's `stop`
  acknowledgement names, and only if it is a live child of init running the
  program `/etc/init.conf` gives that service (anything else
  `rollback_kill_refused ... reason=not_an_instance`, audited), so the
  mailbox gives init no kill authority over anything else. Which instance
  of that program it names is still init's claim.
- **Process output cannot drive the operator's terminal** (V0.11, in
  development; OUT11-001): every control character a Ring 3 process writes,
  except the line feed and the tab, is shown escaped (`\x1b`, `\x0d`,
  `\u{9b}`) rather than performed, on top of the marker rewrite
  (OUT10-002), so a program cannot move the cursor to redraw a line the
  kernel printed — such as a proposal's preview before the operator
  approves it. The invisible Unicode format characters that reorder or
  hide text are escaped the same way (`\u{202e}` for a right-to-left
  override, which could otherwise make reversed text display as a kernel
  marker; the bidirectional marks, embeddings and isolates, zero-width
  characters and the byte-order mark likewise). Everything else still
  passes: printable text, including lookalike glyphs.
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
reproducible without letting that key vouch for anything real. V0.11 (in
development, ADR-0024) adds two console-only bits, `sys_view` and
`propose`: no default set, delegation, service definition or package
manifest confers them, so the program holding them is always one the
operator named.
