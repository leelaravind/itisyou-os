# Known limitations — honest current state

Updated continuously; last full revision: 2026-09-13 (v0.9.0 release; earlier: V0.8.1 release-integrity
closeout); V0.11 items (in development, not released) added 2026-09-14.
Each item says what is true of the current build, not what was true
when the subsystem first landed. Items that later milestones address name the
milestone; the sequence lives in `docs/ROADMAP.md`.

## Execution environment

- **QEMU is the only supported and tested execution environment.** Physical
  hardware boot is intentionally out of scope and untested; nothing here should
  be written to a real disk.
- QEMU machine model: `pc` (i440FX), 256 MiB RAM, CPU `qemu64,+smep,+smap,+umip`.
  Other machine models are untested.
- Single CPU only; no SMP. Synchronization assumes one core; spinlocks are
  interrupt-safe by masking, not SMP-safe.
- Host tooling scripts are Windows PowerShell (`scripts/*.ps1`); CI provides the
  Linux path.
- The toolchain is pinned to nightly-2026-08-01 because newer nightlies break
  the bootloader crate's UEFI stage (rust-osdev/bootloader#579).

## Boot

- The `itisyou-fs-persist` binary's **BIOS** disk image does not boot (a
  bootloader BIOS-stage quirk specific to that binary; its ELF is valid and its
  UEFI image boots). Its two-boot persistence test runs on UEFI. Root cause not
  isolated. Every other image boots on both BIOS and UEFI.
- Boot under emulation is slow and depends on host load. In the V0.11 BIOS
  runs on the development laptop the first serial line (the bootloader's)
  arrived between about 5 and 14 s after QEMU started; `/sbin/init` then
  starts four services (tickd, flapd, inferd, flakyd) before the console is
  usable. The harness allows 75 s from QEMU's serial connection to the first
  byte (`qemu-runner`'s default `--boot-timeout-secs`, which no leg
  overrides), inside each leg's own overall timeout.

## Kernel, processes, scheduling

- Scheduling is **preemptive round-robin** with a fixed 20 ms quantum and no
  priorities or CPU accounting. Registers and address-space isolation are
  preserved across preemption (machine-verified).
- Scheduling is **always-on but cooperative** (V0.10, ADR-0022): background
  processes get bounded slices (1 tick, 32 quanta or 20 ms) at audited safe
  points — the idle prompt, `bg` job waits, every network poll, the desktop
  and `usbwait` loops, and between quanta of a foreground `run` — never from
  the timer interrupt; the kernel itself is not preemptible. Inside these
  **non-schedulable regions** the background waits for the region to end:
  every syscall (`net_resolve` can take 1.5 s), `irq` (about 200 ms with the
  timer masked), `xhciwait` (up to 5 s), `beep`, and every command that holds
  the NVMe store (`store`, `pkg install/stage/rollback/recover`,
  `audit save/verify`). Measured with `sched last` (the whole command window,
  QEMU on the development laptop): `store put` 37 ms, `pkg install` 1547 ms,
  `irq` 932 ms, `xhciwait` with no input 5002 ms — against 49 ms for a busy
  `net poll`. A background process's effective quantum is 10–20 ms of Ring 3
  time; syscall time is not charged to it. The NIC is polled only by syscalls
  and console network commands (a slice never polls it), and `tickd` still
  spins on `yield`, so an idle system never halts the CPU.
- **Userspace init** (V0.10, ADR-0023): `/sbin/init` (pid 1) starts and
  supervises the services in `/etc/init.conf`, which lives in the initramfs —
  there is no persistent configuration and no reload. Its authority is fixed
  (spawn, IPC, `fs_read` under `/etc`, service reporting; since V0.11 also
  the kernel's one-slot `retry`/`stop` mailbox, `init_ctl`, which only the
  live init may read), every service inherits its `/etc` filesystem
  sandbox, and only init and its configured services have deterministic
  pids (since V0.11 four services: tickd, flapd, inferd and the test fixture
  flakyd, pids 2–5). If init dies the kernel ends its whole
  tree and restarts it at most 3 times. `svc_report` trust: the kernel
  verifies pid, parentage, caps and row ownership; a service's name and
  policy are init's claim, attributed with `supervisor=`. Likewise, in a
  `retry-service` rollback the kernel kills the pid init names in its `stop`
  acknowledgement — but only a live child of init running the program
  `/etc/init.conf` gives that service (anything else is refused
  `not_an_instance`, ACT11-002), so init gains no kill authority over the
  rest of the system; which instance of that program it names is still its
  claim.
- **Ring 3 shell** (V0.10): `/bin/sh` runs only when the kernel console's
  `rsh` lends it the console's input, and the kernel console waits until it
  exits; the kernel console is still where the system is administered
  (`store`, `pkg`, `net`, `audit`, … exist only there). The shell has
  builtins and `run` — no pipes, redirection, job control, quoting or
  environment. Console input is polled (the UART interrupt is not routed),
  and the kernel reads input for the shell only while the shell is waiting
  for a line.
- Process model (V0.10): spawn, wait and a non-blocking `wait_nohang`, only by
  the parent; `sleep`; orphans reaped automatically (no adopter until
  `/sbin/init` runs); the console's `ps` and `kill`. No fork/exec, no process
  groups, no signals — `kill` is the console's alone and cannot be caught.
  Program arguments (V0.10) are
  bounded and deliberately narrow: at most 16 arguments and 512 bytes, each
  argument printable ASCII without spaces (no quoting, no UTF-8, no empty
  arguments); no environment variables. From the console a line is still
  capped at 256 bytes and 24 tokens, so the 512-byte limit is reachable only
  through `spawn_args`. Since V0.10 the console's `run` runs a program as a
  process in the table, so its `wait` blocks and its `sleep` sleeps (before,
  a foreground program ran alone and both returned at once); `run` has no
  timeout — the console waits until the program ends — while `bg` gives up
  after 30 s.
- IPC: bounded kernel message channels (8 channels since V0.11 - 6 and 7
  are `/bin/inferd`'s - ≤256 B, ≤8 queued), non-blocking, addressed by
  integer id. Since V1.0 an IPC capability names the channels it reaches
  (ADR-0025): plain `ipc` and every default set reach channels 0–5, and the
  inference channels 6–7 only by a grant naming them (`ipc:6-7`). v0.11.0
  and earlier scoped nothing: any holder of the IPC capability reached every
  channel. Channels still carry no sender identity, so among the processes
  that hold a channel a message is a claim (inferd's clients check a nonce;
  the kernel recomputes what it relies on). A message outlives its sender:
  what a program leaves queued stays until someone takes it, so the next
  client of that channel can read it as an answer (`tick-client` rejects
  what it cannot read as a reply - `TICKC-BAD-REPLY` - but an 8-byte message
  would pass; the V1.0 fuzz legs drain the channels before their checks). A
  grant is one contiguous range of channels. No shared-memory or synchronous
  rendezvous IPC.
- **`wait(0)` never returned while the caller's children were running** in
  v0.10.0 and v0.11.0 (found by V1.0's syscall fuzzer, fixed in V1.0 -
  PROC1-001): the wake-up on a child's exit matched the waiter's pid alone,
  so a parent blocked for "any child" slept forever, and a console `run` of
  it never returned. It needs the Process capability and hangs only the
  caller and whoever waits for it; `wait(<pid>)` and `wait_nohang` were
  unaffected.
- **A page-table frame leaked with store operations** in every release from
  V0.4 to v0.11.0 (found by V1.0's soak leg, fixed in V1.0): each operation on
  the persistent store re-opens the NVMe controller, and every open mapped its
  registers at a fresh virtual address, so about one physical frame was lost
  per 256 store operations - about 16 MiB per million - until reboot.
  Since V1.0 the same registers keep one mapping (V1-REL-003).
- The kernel heap is a fixed **32 MiB** range; no growth. Physical memory above
  4 GiB is ignored by the frame allocator (`ignored_high_frames`).
- Syscalls run with interrupts masked (`SFMASK` clears IF); blocking waits inside
  a syscall are therefore forbidden and the network/storage paths are
  non-blocking there. CR3 switches do a full TLB flush (no PCID).

## Security model

- Capabilities are **resource-scoped handles** with owner binding, bounded
  delegation, revocation and expiry, enforced on the syscall path (V0.8): every
  gated call revalidates the caller's handle in the kernel table. The V0.7
  bitmask survives only as the *request* vocabulary (manifests, `spawn_caps`).
  The capability kinds are fixed at build time and there is no user-facing
  policy editor.
- The filesystem sandbox is a per-process set of path prefixes, checked on the
  normalized path for reads and, separately from the write right, for writes.
- **Package trust is a key hierarchy** (V0.9): the kernel trusts only an
  offline root whose private key is outside the source tree, and signing keys
  only through root-signed certificates with a name scope and a validity
  window in release epochs, subject to a root-signed revocation list. The
  image's fixture packages are signed by a PUBLISHED test key — deliberately,
  so anyone can reproduce the image — whose certificate covers only names
  starting `hello-`. Limits: rotation is an offline operation plus a new
  image (no over-the-network trust updates); validity is by release epoch, not
  date, because there is no trusted clock; the boot image itself is not
  authenticated, so whoever can rewrite it can replace the root with the
  kernel; and there is no persistent anti-rollback floor for revocation lists
  across boots. No real package is signed by the release key yet.
- The audit trail is hash-chained and persistent: it detects a record being
  altered, deleted, reordered or inserted. On its own it does **not** detect an
  attacker who rewrites the whole file — the chain is unkeyed, and a valid empty
  trail verifies. Since V0.9 the saved head can be **anchored** with a witness
  off the disk and checked against it, which does detect that rewrite; but
  anchoring and checking are explicit commands (`audit anchor`, `audit
  check-anchor`), the witness must be reachable, and in the harness the witness
  is a test peer, not a hardened service. Persisting is explicit (`audit
  save`), not automatic on every record. Since V0.11 the stored trail is a
  **window of the newest 128 records** whose header names the head its first
  record extends (`base`): records before the window are gone from the disk
  and the chain vouches only for those still present (dropping them changes
  the stored count, which the anchor compares). `audit verify` is read-only
  and compares the file with the trail this boot last saved or recovered, so
  a replacement is caught while the system runs even when its chain is valid;
  across a reboot only the anchor catches a valid replacement. Trails written
  by v0.8.0–v0.10.0 after the ring had dropped a record, or by a boot other
  than the first, are reported `TAMPERED` by every version, V0.11 included:
  that is AUDIT11-001, and such a trail cannot be told apart from a tampered
  one. A trail that does not verify STAYS reported: recovery continues its
  chain on purpose, so every later save carries the break and every later
  boot reports `TAMPERED` (AUDIT11-003; an earlier version of this page said
  the next save replaced it with a verifying one — it does not). The way out
  is the operator's: `store rm audit.log`, then a reboot, which starts a
  fresh trail and loses the old records (they could not be vouched for
  anyway). An unreadable trail — including one over 256 KiB or over 128
  records, which is not read at all — is still overwritten by the next save.
- The persistent store is shared: programs holding `fs_read`/`fs_write` reach
  every name in it except the files the kernel owns — the audit trail and
  the package store (`<app>.<v>.pkg`/`.ok`), refused since V0.11 (SEC11-001;
  v0.10.0 and earlier let such a program replace the audit trail, or delete
  a package's commit marker to roll it back). There is no per-program
  directory beyond the path-prefix sandbox. Names with control characters
  — or, since the V0.11 review, the invisible characters that reorder or
  hide text — are refused, and every kernel echo of a name or of file
  contents is one escaped line (AUDIT11-002; v0.10.0 and earlier echoed them
  raw, so a program's file name could print a line that read as a kernel
  marker). Printable lookalike characters are not escaped.
- **Console evidence in v0.10.0** (both fixed in V0.11, found by its
  adversarial review): a program could still print a genuine-looking
  `[ITISYOU:` line by splitting the prefix across the kernel's 256-byte
  output chunks, or across two writes when all 16 line buffers were in use
  (OUT11-002); and `ps`/`kill` printed the path string a program passed to
  `spawn`, so a component later popped by `..` could carry a line break and
  a forged marker into the kernel's own output (SEC11-002). v0.10.0 stays as
  released.
- SMEP, SMAP and UMIP are enabled when the CPU advertises them (the harness's
  CPU model does); W^X for user segments, a guard page below the user stack, and
  user-pointer validation are always on — since V0.10 the validation also
  checks that the program itself may write a buffer the kernel writes (v0.9.0
  and earlier checked only that it was mapped, so any program could crash the
  kernel through `cap_list`: SEC10-001). Four kinds of kernel stack are on
  unmapped guard pages, so an overflow stops the machine with a double fault
  naming the stack instead of silently corrupting kernel data: since V0.9 the
  RSP0 stack (interrupts and exceptions arriving from Ring 3) and the
  double-fault IST stack; since V0.10 the syscall stack — a third static stack,
  32 KiB, the one the first TCP integration overflowed into the capability
  table, which v0.9.0 left unguarded although its documentation said otherwise
  (corrected after the release) — and the kernel TASK stacks (32 KiB each, in
  fixed slots of a dedicated window whose first page is never mapped). Whether
  the bootloader-provided boot/console stack has a guard page
  has not been verified.
- No hardware root of trust, no Secure Boot chain, no measured boot.

## AI layer (V0.11 — in development, not released)

- **The model is a linear classifier trained on synthetic data**
  (MODEL11-001, ADR-0024): three integer yes/no detectors —
  `service_failed`, `scheduler_paused`, `denial_burst` — each a bias and 16
  weights over the view's 16 features, trained at build time by an averaged
  perceptron on examples generated from the hand-written ranges in
  `ai/scenarios.txt`, never on telemetry observed from a running system. It
  does not learn at run time. Its measured accuracy — exact match 10000 bp on
  150 held-out synthetic examples — is also reached by a one-rule baseline
  for every condition, because the scenarios define each condition by
  essentially one feature: the figure says the designed distributions are
  separable, not that the model diagnoses real systems. Inference runs in
  Ring 3 (`/bin/inferd`), but the kernel evaluates the same model too — only
  to check the condition a proposal cites, never to decide anything
  (PROP11-001).
- **Three conditions, two actions.** `service_failed` may propose only
  `retry-service` and `scheduler_paused` only `resume-scheduler` (a closed
  table in `kernel_core::policy`); `denial_burst` is diagnosed but has no
  action (`AGENT-ACTION id=denial_burst action=none`), and a proposal citing
  it is refused `action_not_allowed`. There is no runaway-process condition
  and no action that stops an arbitrary process (no per-process CPU
  accounting exists to detect one, and there is no Ring 3 kill to refuse;
  ADR-0024) — the only kill is `retry-service`'s rollback of the instance it
  asked init to start. Knowledge is
  a fixed map — one runbook per condition under `/etc/ai/kb/` — not
  retrieval, and the agent does not plan, generate text or learn. It files
  one proposal per run and names as its retry target only the first failed
  service in the view (on the shipped image, flapd), so `ai-retry-bios` files
  the proposal for flakyd through its test probe, not through the agent.
- **What inferd answers is an unauthenticated claim.** IPC carries no sender
  identity (see *Kernel, processes, scheduling*). In v0.11.0 every program
  `run` from the console without a caps list held the IPC capability for
  every channel, so any such process could take inferd's requests off
  channel 6 — reading the 16 features the requester derived from its view —
  or answer on channel 7. Since V1.0 (ADR-0025) only a process granted
  `ipc:6-7` by name reaches those channels — inferd, and whatever the
  operator starts with that grant — so the residual is limited to processes
  holding that grant. The nonce a client sends only lets an honest
  client discard stale and foreign replies; it authenticates nothing. A
  reply read by the wrong client is discarded, not put back, so clients
  asking at the same time can lose each other's answers and time out
  (`AGENT-INFER-TIMEOUT`, after 3 s). A spoofed reply can therefore mislead a
  diagnosis, but it cannot get an unjustified proposal filed: the kernel
  recomputes the cited condition from the view it last served that process,
  with the model compiled into it, and refuses a mismatch
  (`diagnosis_mismatch`).
- **Proposals live only in kernel memory.** The table holds 8 and is lost at
  reboot (ids start again at 1 on every boot); what survives is the audit
  records, and only if the trail is saved (`audit save`). New proposals are
  refused `table_full` while all 8 are pending. One pending proposal per
  submitter means per process: separate processes the console granted
  `propose` can each hold one. A proposal must cite a view served to its
  submitter at most 500 ticks (5 s) earlier, and a pending one expires 6000
  ticks (60 s) after it was filed — checked whenever a proposal is filed and
  when the console runs `proposals`, `approve` or `deny`, not on the clock
  (expiry is host-tested; no QEMU leg waits out the 60 s). Only the filing and
  execution audit records (`proposal_submitted`, `action_executed`) cite the
  model and the view, by a 16-hex-digit digest prefix, and the kernel
  forgets the view's bytes when the process it served ends, so a diagnosis
  cannot be re-derived from the trail afterwards.
- **The healthy case is not observable in the shipped image.** flapd fails
  by design on every boot (and flakyd until a retry is approved), so the
  running system always shows at least `service_failed`, and
  `ai-diagnose-bios` forbids `conditions=none`. The no-condition diagnosis is
  covered only by host tests: the held-out synthetic examples include the two
  `healthy` scenarios of `ai/scenarios.txt`, and a unit test checks that a
  detector scoring exactly zero does not fire.
- **`/bin/flakyd` is a test fixture shipped in the image**, as
  `/sbin/init`'s fourth service (no capabilities), so that the success path
  of `retry-service` can be exercised: it exits with failure whenever it is
  started before 300 ticks (3 s) of guest uptime and runs from then on, so at
  boot init restarts it three times and leaves its row Failed until a retry
  is approved.
- **Approval is the kernel console's alone.** `approve <id>` and
  `deny <id>` exist only in the kernel console, which reads no input while
  the Ring 3 shell holds it. There is no remote or multi-operator approval,
  and nothing records which person approved: the audit says `by=console` /
  `approved_by=console` and no more. Every proposal needs approval — the
  policy has no automatic path — and `approve` holds the console for the
  verification window (1 s for `resume-scheduler`, 3 s for `retry-service`,
  plus up to 3 s for each acknowledgement it waits for from init).
- **Guest time is not wall time.** The kernel's clocks are the 100 Hz PIT
  tick and the TSC calibrated against it at boot: the TTL and the view's age
  are counted in ticks, verification windows and `busy` in calibrated TSC
  time. Under QEMU's emulation (TCG: the runner passes no accelerator) guest
  time runs slower than the host's wall clock, so a harness pause in host
  time (`@pause`) ages the guest by less; a leg that needs guest time to pass
  waits with the console's `busy <ms>` (`ai-retry-bios` runs `busy 3000`
  before retrying flakyd).

## Storage

- NVMe **read + write + flush**, one I/O queue, one namespace; MSI-X delivery is
  proved on it, but the data path is still polled.
- ITFS is a minimal persistent filesystem: a fixed directory of **≤12 files**,
  no subdirectories, and **contiguous** files. Since V0.10 freed space is
  reused (FS10-001): free space is recomputed from the gaps between the live
  extents the superblock lists — no free list is stored and the on-disk format
  is unchanged — and allocation is first-fit over those gaps. Crash
  consistency is unchanged: the superblock is a double-buffered CRC commit, an
  overwrite is one crash-atomic commit, and new data only ever goes to blocks
  that NEITHER superblock slot references (the committed one or the one a
  mount would fall back to), so a torn data write is harmless and an old
  extent is reused only after two commits have stopped referencing it.
  Remaining limits: **no compaction or defragmentation** — files are
  contiguous, so a write larger than every gap is refused with `Fragmented`
  even when enough blocks are free in total (removing or overwriting a
  neighbour opens a gap); freed space becomes allocatable one commit late
  (the fallback slot pins it, reported as `pinned` by `store df`), so an
  overwrite needs a free run outside BOTH the committed and the fallback
  copies of the file — repeatedly overwriting a file larger than about a
  third of the free space can be refused until another commit releases the
  pin. That is deliberate: pinning only the committed slot would already be
  crash-safe, but a later unreadable newest superblock would then make mount
  fall back to a directory whose data may have been overwritten, and serve
  it silently; with both slots pinned the fallback is always intact. And a
  handle whose superblock commit failed re-reads both slots before its next
  transaction on a best-effort basis — if that re-read also fails, the next
  write on the same handle trusts memory, as V0.9 did (every console and
  syscall operation mounts afresh, so no long-lived handle crosses a failure).
- No second block driver (AHCI/virtio-blk); the block trait is ready for one.
- QEMU attaches only generated disposable disks; no host disk is ever touched.

## Networking

- IPv4 over one e1000 NIC: ARP, ICMP echo (client and responder), UDP with
  capability-scoped Ring 3 sockets, and a DNS A-record resolver.
- **IPv6** (V0.9) is foundations only, brought up on demand (`ipv6`): a
  link-local address, SLAAC from a router's advertised /64, neighbour
  discovery and ICMPv6 echo in both directions. No extension headers (a packet
  carrying one is refused), no duplicate address detection, no DHCPv6, and no
  IPv6 UDP/TCP or Ring 3 IPv6 sockets.
- **TCP** (V0.9) is IPv4 only. Ring 3 programs can open a connection
  (`tcp_connect`) or listen for one (`tcp_listen`), stream both ways and
  close. A listen serves exactly one connection — the listening descriptor
  becomes the stream — so there is no accept queue and no second client
  until the program listens again. At most 8 connections, 4 KB send and
  receive buffers each. Loss
  recovery is a single retransmission timer (300 ms, doubling, 6 retries, then
  the connection times out) — no RTT estimation, no congestion control beyond a
  cap of four segments in flight, no SACK, window scaling, timestamps, fast
  retransmit, delayed ACK, zero-window probe or FIN-WAIT-2 timeout. TIME-WAIT is
  1 s (MSL 500 ms), a shortcut for the QEMU link. Initial sequence numbers mix
  the TSC with the four-tuple but are not RFC 6528's keyed hash — there is no
  per-boot secret yet. Timers run only when the stack is polled, so a finished
  connection's TIME-WAIT slot is reclaimed at the next poll, not on the clock.
- **DHCP** (V0.9) runs on
  demand (`dhcp`) and applies address, mask, router and DNS; it is not run
  automatically at boot and there is no background renewal at T1 — the lease
  is simply used until the next boot. Without it the static plan (10.0.2.15/24
  via 10.0.2.2) applies. No routing beyond one gateway, no fragmentation, and
  no ICMP error generation — an unreachable port is dropped silently.
- DNS through QEMU's user-mode forwarder failed on the Windows test host (no
  reply at all); DNS is verified against the harness's own peer.
- One NIC, polled; the receive path is drained from a scheduling slice, so a
  program that never yields also never receives.
- QEMU's 82540EM exposes no MSI capability, so the NIC has no interrupt path at
  all; MSI-X is proved on the NVMe controller instead.

## Interrupts and platform

- Line-based IRQs (timer, PS/2 keyboard and mouse) are delivered by the
  **I/O APIC** on the routes the ACPI MADT declares, and the 8259 PICs are
  retired (fully masked, LAPIC LINT0 masked) — since V0.9. Only those three ISA
  lines are routed; other devices are polled or use MSI-X. The PIT (mode 2) is
  still the scheduler tick; the local APIC timer is only used for a delivery
  proof. Single CPU: no IPIs, no AP startup, no x2APIC.
- ACPI support is table discovery (RSDP, RSDT/XSDT, MADT, FADT) plus the DSDT's
  `\_S5_` found by pattern match — **not an AML interpreter**, so methods,
  devices and power resources described in AML are invisible to the kernel.
- `poweroff` performs a real ACPI S5 soft-off; `shutdown` still uses QEMU's
  `isa-debug-exit` test device (the harness relies on its exit code). No
  suspend/resume, no thermal or battery handling.
- PCI enumeration scans bus 0 only (complete on the `pc` machine; no bridge
  recursion).

## Graphics, desktop and input

- One **bootloader-chosen framebuffer mode** (QEMU: 1280×720 BGR); no
  mode-setting, no vsync, software rendering only, **no GPU acceleration**.
- One 8×8 bitmap font, integer scaling only, no Unicode.
- Compositor: fixed wallpaper and top bar; click-to-focus with raise (V0.10)
  but **no drag, resize, minimize or close buttons**. Ownership and bounds are
  enforced; a process holds at most 4 windows. One Ring 3 app can be started
  on the live desktop (`desktop <app>`); the console cannot start more while
  the desktop runs (it does not read the console), and ESC always leaves the
  desktop (it exits QEMU). Apps receive focus, key and click events; there is
  no pointer-motion or key-release event, and a full redraw happens on every
  change.
- PS/2 keyboard is scancode set 1, US layout, printable keys plus a few
  controls; no key repeat policy, no IME. Mouse is relative 3-byte packets, no
  wheel. Only the first USB HID device is enumerated.
- The console reads **polled serial**; the PS/2 keyboard drives the graphical
  desktop, not the console.

## USB and audio

- USB: UHCI (USB 1.1) and xHCI host controllers. Each enumerates **one device**
  on one root port: no hubs, no multi-device addressing, control and interrupt
  IN transfers only (no bulk or isochronous), no runtime attach/detach. HID boot
  protocol keyboard input is verified on both; USB mouse decode is host-tested
  only.
- Audio: AC97 **output only** — one PCM-out stream at 48 kHz, no capture, no
  mixing or resampling in the OS.

## Not present at all

- Wi-Fi, Bluetooth, webcam, GPU drivers, power management, suspend/resume, a
  browser, Linux binary compatibility, a production package ecosystem. These are
  research items that need physical hardware under an explicit, per-device
  authorization gate (`docs/IMPLEMENTATION_PLAN.md` §27).

## Project process

- Website deployment state is tracked in `docs/REQUIREMENTS.md` (CF-001 and the
  per-release WEB rows).
- The `v0.8.0` release reported `0.7.0-dev` from its own kernel; `v0.8.1` exists
  to correct that, and `website/scripts/check-consistency.mjs` now fails any
  build where the versions disagree.
