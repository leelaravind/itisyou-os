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
| Evidence tampering | editing the persisted trail | records hash-chained, chain extended before the bounded ring drops anything, verified on every boot. Does NOT cover an attacker who rewrites the file including its head |
| Kernel dereferencing a user pointer by accident | a logic bug | SMAP: forbidden by default, permitted only inside three declared windows |
| Ring 3 leaking kernel addresses | `sgdt`/`sidt`/`sldt`/`str`/`smsw` | UMIP; verified by a probe whose success marker is forbidden from the log |

### Explicit non-threats, still

The guest has never been attached to a real network. Its only peer is the test
harness on localhost, and there is no DHCP, no IPv6, and no TCP — so there is
no listening service, no connection state to exhaust, and nothing that
initiates traffic on its own.

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
  The trigger was removed; guard pages for kernel stacks remain future work.
