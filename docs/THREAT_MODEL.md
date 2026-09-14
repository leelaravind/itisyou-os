# Threat model — ITISYOU OS

ITISYOU OS is a pre-alpha research OS that runs **only inside QEMU**. The
threat model is scoped to that reality and grows with each milestone (plan
§11): the V0.1 baseline below still holds, and each later section adds what a
milestone changed — read the document top to bottom, newest additions last.

## Assets

- The developer's host machine (Windows laptop): its disks, boot chain,
  firmware, and data. **Highest-value asset; the OS must never touch it.**
- The repository: source integrity, commit history, requirement evidence.
- Credentials used by tooling (GitHub, Cloudflare) — never enter the repo.
- The public website's truthfulness (a false capability claim is an
  integrity failure).

## Adversary/failure assumptions — V0.1 baseline

There is no multi-user or network exposure yet; realistic risks are:

| Risk | Vector | Mitigation |
|---|---|---|
| Host damage from OS testing | misconfigured VM (disk passthrough, raw device writes) | QEMU launches only project-generated images; no passthrough flags anywhere in scripts/harness; physical-hardware boot is out of scope (plan §27) |
| Kernel bug corrupting its own state silently | logic errors, bad `unsafe` | panic-on-invariant-violation; boot-stage markers; selftests; unsafe inventory |
| Malformed boot data | firmware/bootloader handoff anomalies | validate memory map/pointers before use; reject overlapping regions (MEM-001 tests) |
| Malformed initramfs | corrupt/hostile archive bytes | strict bounds-checked parsing with negative-case tests (planned with FS-001) |
| Supply-chain drift | dependencies with unexpected code | small pinned dependency set, `Cargo.lock` committed, rationale per kernel dependency |
| Secret leakage | tokens in commits/logs/CI | `scripts/secret-scan.ps1` before pushes; env-var/platform secret stores only |
| Fake completion | claims without evidence | requirement matrix + machine-readable QEMU evidence; website generated from `status/current.json` |

## Explicit non-threats — V0.1 baseline (network input superseded by V0.8 below)

Remote attackers (no network stack), malicious local users (single-developer
VM), physical attacks, and side channels are out of scope until the relevant
subsystems exist. Each later milestone (userspace, storage, networking) must
extend this document before shipping the feature.


## V0.8 additions — the network changes the shape of this document

Until V0.8 every input came from the developer's own machine: an image the
build produced, a disk the harness generated, a keypress the harness injected.
A network stack is the first subsystem that processes bytes chosen by someone
else, arriving before any authentication exists to judge them by. That is a
different class of exposure, and it is worth stating plainly even though the
only network this OS has ever been attached to is a test harness on localhost.

### New assets

- The guest's own integrity while parsing untrusted frames. A parser bug here
  is reachable by anything that can put a frame on the wire.
- The package trust root: the one key that decides what may be installed.
- The audit trail's truthfulness: evidence that can be edited is not evidence.

### New adversary assumptions

| Risk | Vector | Mitigation |
|---|---|---|
| Malformed frame corrupts or hangs the stack | any peer on the link | allocation-free parsers with a distinct error per rejection; fragments, VLAN tags and non-echo ICMP refused rather than handled; DNS compression bounded by a jump budget; verified by a hostile-frame sequence the guest must refuse and answer nothing |
| The guest used as a reflector or amplifier | spoofed source, packets addressed elsewhere | the IP layer re-checks the destination even though the NIC filters; no ICMP errors are ever generated; a datagram to an unbound port is dropped silently |
| Unbounded kernel memory from remote traffic | flooding | ARP cache, socket table and per-socket queues are fixed-size arrays; a peer can cause entries to be *replaced*, never allocated |
| A program reaching the network without authority | any Ring 3 process | network access is a capability scoped to a port, re-checked on every datagram call against the socket's own port, so a handle narrowed or revoked after the bind stops working at the next use |
| A hostile package installed | a package from any source | Ed25519 signature over context + lengths + digest, verified against a compiled-in trust root; unsigned, foreign and forged packages refused as three distinct outcomes; re-verified at launch as well as install |
| A stolen or misused signing key | the development key is published | V0.8: ACCEPTED for a pre-alpha build. Superseded in V0.9 (ADR-0021): the published key is now a scope-limited test key (`hello-*` only); trust flows from an offline root through certificates with scopes and epoch windows, and a compromised key is retired by a root-signed revocation list |
| Evidence tampering | editing the persisted trail | records hash-chained, chain extended before the bounded ring drops anything, verified on every boot. Does NOT cover an attacker who rewrites the file including its head. Residual in v0.8.0–v0.10.0, found during V0.11: the verifier cries wolf — a trail saved by a later boot or after the ring dropped a record reads as TAMPERED although untouched (AUDIT11-001) — and any program holding `fs_write` without a sandbox can rewrite or delete the trail through the filesystem syscalls (SEC11-001); both fixed on the V0.11 branch |
| Kernel dereferencing a user pointer by accident | a logic bug | SMAP: forbidden by default, permitted only inside three declared windows. Residual in v0.9.0, found after the release: inside a window the kernel writes to any MAPPED user page, including the program's own read-only code, and the resulting Ring 0 fault panics the kernel — a denial of service any program can trigger through `cap_list` (SEC10-001, fixed in V0.10) |
| Ring 3 leaking kernel addresses | `sgdt`/`sidt`/`sldt`/`str`/`smsw` | UMIP; verified by a probe whose success marker is forbidden from the log |
| A program crashing the kernel through a syscall buffer | a pointer to the program's own read-only memory | V0.10 (SEC10-001): kernel writes require the page to be writable by the program at every level of the walk; before, a write-protect fault in Ring 0 panicked the kernel (v0.9.0 and earlier) |
| A process collecting or killing another's children, or claiming its services | `wait`/`wait_nohang` on a foreign pid; `svc_report` for a foreign pid or name | parent-only collection (audited `wait_foreign`); `svc_report` refuses a pid that is not the reporter's child, a reserved name and a row another live supervisor owns (audited); only the console can `kill` |
| A program starving the system | a CPU-bound program | preemption (V0.4) plus bounded background slices at every console wait (V0.10); residual: inside a non-schedulable region (a syscall, `irq`, `xhciwait`, storage commands) the background waits, measured up to 5 s for `xhciwait` |
| A program stealing input or another program's window | reading the console or another window's events; presenting another's window | console input has one owner, granted by the kernel; `gui_event` and `gui_present` are owner-only (V0.9's `gui_present` checked only existence); keys go to the focused window's owner only |
| Forged kernel evidence lines | a program printing `[ITISYOU:…]` | Ring 3 output is line-atomic and the marker prefix is rewritten (`[RING3-U:…]`) before it reaches the log (OUT10-002). Residual in v0.8.0–v0.10.0, found during V0.11: the KERNEL echoes file names and file contents a program chose without escaping them — a name with a line break in it becomes a line of its own that reads as a kernel marker (AUDIT11-002, fixed on the V0.11 branch); and a program's ESC, CR and C1 bytes reach the terminal, so it can move the cursor and redraw a line the kernel printed (OUT11-001, fixed on the V0.11 branch). Two more in v0.10.0, found by the V0.11 adversarial review: the prefix rewrite ran one output chunk at a time, so a marker split across the 256-byte buffer seam — or across two writes once all 16 line buffers were taken — reached the port whole (OUT11-002: the buffer now keeps a tail that could start a marker for the next chunk, and an unbuffered write is closed with a newline); and `ps`/`kill` printed the path string a program passed to `spawn`, whose `..`-popped component could hold a line break and a marker (SEC11-002: the kernel records the canonical path and escapes it). Both fixed on the V0.11 branch |
| A program rolling back an application | deleting `<app>.<v>.ok` through `fs_delete` | NOT mitigated in v0.10.0 and earlier, found during V0.11: the package store shares the filesystem namespace programs write (SEC11-001, fixed on the V0.11 branch: kernel-owned names are refused to the `fs_*` syscalls) |

### Explicit non-threats — V0.8 (superseded by the V0.9 additions below)

In V0.8 the guest had never been attached to a real network: its only peer
was the test harness on localhost, and there was no DHCP, no IPv6 and no TCP.
V0.9 added all three (see below); the guest is still exercised only against
the harness and QEMU's user-mode network, never a real LAN.

## V0.9 additions

- **Whole-trail rewrite of the audit log** (an attacker with disk access writes
  a new, internally consistent trail — in the limit, an empty one): the unkeyed
  hash chain cannot detect this by construction. Mitigation: anchoring the
  saved head with a witness off the disk (`audit anchor` / `audit
  check-anchor`); verified by a four-boot test in which the forgery passes the
  chain check and fails the anchor check. Residual risk: anchoring is explicit,
  and an attacker who also controls the witness is out of scope.
- **Firmware tables as input**: ACPI tables are parsed with every length and
  checksum validated before use and every page confirmed mapped before it is
  read; a malformed table is refused and the kernel keeps its previous
  configuration (no cutover without a valid MADT).
- **DHCP as input**: replies are accepted only for this client's transaction
  and hardware address, from the server port, with options bounds-checked; a
  lease missing an address or server identity is refused rather than applied.
- **TCP segments as input** (ADR-0020): every segment is checksummed against
  the pseudo-header of the addresses it arrived between; options are
  length-checked; a RST resets only at exactly `rcv_nxt` and a SYN or
  in-window RST elsewhere draws a challenge ACK (RFC 5961), so an off-path
  attacker must guess the sequence number exactly. Segments for no connection
  get a RST, never to broadcast. Resources are bounded (8 connections, fixed
  buffers, a fixed outbox). Residual risks, stated: initial sequence numbers
  are TSC-mixed rather than keyed (RFC 6528), and challenge ACKs are not rate
  limited.
- **Signing-key compromise and misuse** (ADR-0021): a stolen signing key is
  bounded by its certificate's scope and epoch window and retired by a
  root-signed revocation list; a certificate or list the root did not sign is
  refused at load; replaying an older revocation list is refused. Residual
  risks, stated: a stolen ROOT key defeats the hierarchy (it is kept offline
  for that reason); the boot image is unauthenticated, so an attacker who can
  rewrite it replaces the root with the kernel; no cross-boot anti-rollback
  floor for revocation lists.
- **Kernel stack exhaustion**: V0.9 demonstrated that an overflow of a kernel
  stack corrupts adjacent kernel data silently (it hit the capability table).
  The trigger was removed, and the RSP0 and double-fault stacks now have
  unmapped guard pages (the syscall stack — the one that actually overflowed — does NOT, in v0.9.0; corrected after release), verified by a deliberate overflow that ends in a
  reported double fault (HARD09-001). Residual risk: heap-allocated kernel
  task stacks have no guard page.

## V0.11 additions — the evidence itself

V0.11 puts a proposing agent on top of the audit trail, so the trail has to
hold up against the programs it records. Reviewing it for that found three
defects in every release since V0.8, fixed first:

| Risk | Vector | Mitigation |
|---|---|---|
| A program erasing or rewriting the evidence of what it did, or rolling back an application | `fs_write`/`fs_delete` on `/data/audit.log` or on a package's `.pkg`/`.ok` — v0.10.0 and earlier allowed it to any holder of `fs_write` without a sandbox | the audit trail and every name the package resolver reads are kernel-owned: refused to the `fs_*` syscalls (`ERR_PERM`, audited `reason=kernel_owned`) and not listed (SEC11-001) |
| A program forging kernel evidence lines through the kernel's own output | a file name with a line break in it, echoed in an audit detail, by `store ls`, `pkg list` or recovery; file contents echoed by `store cat` | ITFS refuses names with control characters; every kernel echo of program- or disk-chosen text escapes control characters and rewrites `[ITISYOU:` to `[RING3-U:` (AUDIT11-002) |
| A verifier that cries wolf | before V0.11 an untouched trail failed verification after a second boot's save, after the ring dropped a record, or after a mid-boot `audit verify` — so TAMPERED stopped meaning anything | the stored trail carries the head its first record extends and keeps the recovered records; `audit verify` no longer re-runs recovery (AUDIT11-001) |
| A trail replaced while the system runs | a valid (for example empty) trail written under a running kernel | `audit verify` compares the file with the exact bytes this boot saved or recovered (`reason=replaced`); across a reboot, only the witness anchor catches it |

Residual risks, stated: a trail from v0.8.0–v0.10.0 hit by AUDIT11-001 cannot
be told apart from a tampered one; the disk remains writable by anyone with
access to it offline, so everything the kernel reads back from it is treated
as untrusted text; and the serial console is the evidence channel — a program
can still print text that resembles a kernel marker without its prefix, which
is why kernel verdicts are printed by the kernel itself.

## V0.11 additions — the AI-native layer (in development)

V0.11 adds the first component whose output is a statistical guess — a
diagnostic model — and a program that acts on it by proposing system
actions (ADR-0024). The design assumes the agent, inferd and anything else
on IPC may be hostile and asks what each can make the kernel do. Everything
below is on the V0.11 branch; no release contains it (the latest is
v0.10.0).

### New assets

- The operator's approval: what `proposals` shows must be what the kernel
  checked, and nothing a program prints may pass for it or redraw it.
- The state an approved action changes: whether the scheduler is paused,
  and a supervised service's row.
- The model's provenance: claims are checked against the model the build
  trained and pinned.

### New adversary assumptions

| Risk | Vector | Mitigation |
|---|---|---|
| A malicious or compromised agent changing the system | code running as `/bin/agent`, or any program the console starts with `propose` | it can only file proposals: no syscall resumes the scheduler or retries a service, and with the agent's authority `svc_report`, `fs_write` and `spawn` are refused (`AIPROBE-DIRECT-ALL-DENIED`); a proposal is filed only after every kernel check in the rows below passes — ten adversarial runs, each refused for exactly its own reason — and nothing filed acts until the console approves it (AGENT11-001, PROP11-001, ACT11-001). Residual: a compromised agent can still file a proposal that passes the checks, print a misleading diagnosis (Ring 3 text, unprefixed and escaped), and — holding `ipc`, whose handle is not scoped to a channel — read or write any IPC channel, as every IPC holder can |
| A spoofed inferd or a forged reply steering the diagnosis | any IPC holder takes the request off channel 6 or writes a reply on channel 7; IPC carries no sender, and the agent's nonce (uptime and pid) is predictable | the kernel never uses inferd's answer: it recomputes the conditions from the exact view bytes it served to the submitter, with its own copy of the shipped model, and refuses a proposal whose condition is not among them (`diagnosis_mismatch`; control: without the recomputation the false diagnosis is refused only as `not_applicable`) (PROP11-001, INFER11-001). Residual: a forged or withheld reply can still mislead the agent into a wrong printed diagnosis, a proposal the kernel refuses, or none at all — a program that takes the replies makes it time out (`AGENT-INFER-TIMEOUT`); and the 16 features a request carries, scaled counts from the view, are readable by any IPC holder, which needs no `sys_view` for them |
| The operator misled at approval: a redrawn preview or lookalike console text | a program writes ESC, CR or C1 sequences to move the cursor over a kernel line, or prints `[ITISYOU:` or a line shaped like a preview; a file name or content the kernel echoes; a hostile target name in a proposal | Ring 3 control characters, and the invisible Unicode format characters that reorder or hide text (bidirectional marks, embeddings, overrides and isolates, zero-width characters, the byte-order mark), are shown escaped, never performed (OUT11-001: `AIPROBE-ANSI \x1b[2A\x1b[2K\x0dforged\u{202e}:UOYSITI]` stays one line, and the right-to-left override that would display the reversed text after it as a kernel marker is shown as `\u{202e}`); the marker prefix is rewritten (OUT10-002); the kernel escapes untrusted text it echoes (AUDIT11-002); a proposal's target is validated as `[a-z0-9-]{1,15}` before it reaches any line (`bad_field` for `[ITISYOU:AI]`), and the preview is built only from validated fields (PROP11-001). Residual: printable lookalike characters (homoglyphs) and other Unicode format characters outside the escaped set pass; a program can print a line that reads like a preview without the kernel prefix; the agent's `AGENT-PROPOSAL` lines are its own claims — what `approve <id>` acts on is the kernel's entry for that id, as `proposals` lists it |
| Filling the proposal table | one submitter filing repeatedly; several processes holding `propose` each filing | one pending proposal per submitter (`already_pending`, `ERR_AGAIN`; QEMU leg, and the control allowing two lets `flood` in), an 8-slot table (`table_full`, `ERR_AGAIN`; host-tested), finished entries evicted oldest first, a 6000-tick (60 s) TTL (PROP11-001). Residual: eight submitters — each a process the console started with `propose`, each with a proposal that passes every check — fill it for up to 60 s: a pending proposal outlives the process that filed it, and entries past their TTL leave only when a proposal is filed or the console runs `proposals`, `approve` or `deny` (an earlier version expired them only on the console commands, so a full table stayed full until the operator acted; found in the V0.11 review and fixed; expiry is host-tested, no QEMU leg waits out the TTL) |
| Replaying a stale or borrowed view | citing an older view, one served to another process, or one never served | the view digest must be that of the last view served to the submitting process (`snapshot_mismatch`), served at most 500 ticks (5 s) earlier (`snapshot_stale`); a process's view is forgotten when it ends (PROP11-001, VIEW11-001; control: without the binding the `forged-view` run is accepted). Residual: inside those 5 s the cited view can be out of date — applicability is checked against the kernel's own facts when the proposal is filed and again at `approve`, so an out-of-date view cannot make an action apply that does not apply now; the kernel remembers views for at most 16 processes at once, the oldest evicted first, so many processes asking can evict a submitter's view (it fails closed, `snapshot_mismatch`) |
| Posing as init on the mailbox | a process other than the live init calling `init_ctl` to read a command or acknowledge one | syscall 44 needs Service ADMIN and the caller must be the live init (`initd::is_init`); anyone else is refused `not_init`, printed and audited; an acknowledgement must name the command init fetched (ACT11-002; control: without the check a non-init process reaches the mailbox). On a rollback the kernel kills the pid init's `stop` acknowledgement names only if it is a live child of the live init running the program `/etc/init.conf` gives that service; anything else is refused (`rollback_kill_refused ... reason=not_an_instance`, audited) — found in the V0.11 review: the kernel had killed whatever pid init named, which gave init kill authority it does not hold (ACT11-002). Residual: init is still trusted to name the right instance of that program, and an init that ignores `stop` leaves the service restarting (control: `Failed { restarts: 3 }`) |
| A command reaching a restarted init | init dies with a command outstanding and its successor fetches it | the mailbox is cleared when init dies, before the next init starts (`initd::supervise`); the waiter times out after 3 s and fails safe — an unacknowledged `retry` is withdrawn and reported `init_unresponsive` (the dead init's services end with it); if the live init had fetched it before going quiet, the kernel does not assume nothing started and rolls the retry back (`stop`, then the instance init names); an unacknowledged `stop` kills nothing (`stop_ack=false killed_pid=0`) (ACT11-002). Residual: this path is code only — no QEMU leg kills init with a command outstanding |
| A poisoned training set | editing `ai/scenarios.txt` so the model fires, or stays silent, when it should not | the model is trained only at build time, from repository files, by host-tested code, and its bytes are pinned: moving one scenario bound by 1 changes the digest and fails both the host pin test and the kernel build (MODEL11-001). The model is also outside the authority path: a condition lets a proposal be filed only for that condition's one action, only when the action applies by the kernel's own facts, and only the console approves. Residual: whoever can change the repository can change the scenarios and the pin together — the pin catches drift, not a malicious commit, which only review catches; a poisoned model can suppress a diagnosis, or produce proposals for actions that do apply |
| Tampering with the model file | replacing `/etc/ai/diag.model` with a malformed or a different model | the model lives in the initramfs embedded in the kernel image, which the `fs_*` syscalls never write; at boot the kernel compares its SHA-256 with the digest compiled in and keeps a copy for recomputation only when they match — without one no proposal passes (the recomputation step refuses it `unknown_model`), and a proposal citing any other model is refused `unknown_model` first; inferd refuses a malformed file by name, each of four hostile fixtures for its own reason (MODEL11-001, INFER11-001; control: the decoder without its magic check accepts the bad-magic fixture). Residual: the model and the digest it is checked against ship in the same unauthenticated boot image, so an attacker who can rewrite the image replaces both (the V0.9 boot-image residual); inferd does not check the pin itself; the mismatch branch at boot has no QEMU leg |
| An approved action that does harm or does not hold | an action approved after the system changed, or one whose effect does not take | two actions only, both reversible and both kernel code; `approve` re-checks the TTL and applicability from fresh facts (`precondition_changed`); each is verified against a post-condition — other processes progress during a 100-tick busy-point window; no restart or failure of the service for 300 ticks and its row Running with the acknowledged instance — and rolled back on failure (ACT11-001, ACT11-002; controls: an executor that does nothing fails verification and is rolled back; a verification that always passes lets the flapd retry pass). Residual: verification checks one post-condition over a short window, not the system's health afterwards; the rollback of `retry-service` depends on init (above) |
| The view or `propose` reaching a program the operator did not name | `run` or `rsh` without a caps list, desktop apps, `pkg launch`, a child through `spawn`, a service in `/etc/init.conf` | both bits are outside `CAP_LEGACY_FULL`, dropped by `caps::delegate`, refused in `/etc/init.conf` (`console_only_capability`), and not implied by ADMIN (rights are exact); the view is refused with no capabilities (`no_handle`) and to a caps-less `run` (`scope_denied`) (VIEW11-001, PROP11-001; controls: the gate removed, or the legacy set carrying the view, prints the forbidden `AIPROBE-VIEW-LEAK`). Residual: the console grants them to whatever program the operator names |

Residual risks, stated: the model's measured accuracy — exact-match 10000
bp on 150 held-out synthetic examples — equals a one-rule baseline for every
condition, because the scenarios define each condition by essentially one
feature: it shows that the designed distributions are separable, not that
the model diagnoses real systems (MODEL11-001), and the QEMU legs show three
constructed situations diagnosed exactly, nothing more general
(AGENT11-001). The kernel's checks establish that a proposal is consistent
with the shipped model and applies now, not that acting is wise; the
operator's approval is that judgement, and whoever can type at the serial
console is the operator.
