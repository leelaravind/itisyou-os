# ADR-0025: IPC capabilities name their channels

## Status

Accepted and implemented for V1.0 (V1-SEC-006 in `docs/V1_ACCEPTANCE.md`,
row V1-SEC-006 in `docs/REQUIREMENTS.md`).

## Context

IPC (V0.3, ADR-0007) is a small fixed set of kernel message channels
addressed by number. Since V0.8 every IPC syscall checks the caller's
Service handle with the channel as the scoped resource
(`require_scoped(Service, USE, channel)`), so a handle scoped to a channel
range could never reach another channel — but nothing ever minted such a
handle. Every grant was `ResourceScope::ANY`: any process holding the IPC
capability reached every channel.

V0.11 made that matter. The inference service `inferd` answers on channels
6 and 7 (`kernel_core::infer`). Every program the console starts without a
caps list holds IPC, so any of them could take a diagnosis's request off
channel 6 — reading the 16 features derived from a view it was never
granted — or answer on channel 7 in inferd's place. The kernel re-checks
every proposal, so a spoofed answer could not get an unjustified proposal
filed, but it could mislead a diagnosis (`docs/KNOWN_LIMITATIONS.md`, V0.11).

## Decision

1. **The channels are capability bits.** Bits 16–23 of the capability word
   are one bit per channel (`caps::cap_ipc_channel`). `CAP_IPC` reaches only
   the channels whose bits are set with it, and they must form one
   contiguous range: that range is the scope the kernel mints the Service
   handle with (`capability::scope_from_bits`). A channel set that is not one
   range grants no IPC at all — rounding it up to one range would widen it.
2. **Delegation needs nothing new.** The channel bits are part of the same
   word, so `caps::delegate` intersects them like every other capability,
   and the capability table refuses a child scope outside its parent's. A
   child reaches at most its parent's channels; no ABI changes.
3. **Grants name channels.** In a caps list (console `run`/`bg`/`rsh`,
   `/etc/init.conf`, package manifests) `ipc` grants the default channels
   0–5, and `ipc:<a>` or `ipc:<a>-<b>` grants exactly `a..=b`. Malformed,
   reversed, out-of-range or non-contiguous lists are refused
   (`CapParseError::BadChannels`; `init.conf` reason `ipc_channels`).
4. **The inference channels are reserved.** Channels 6 and 7 are in no
   default set: not in plain `ipc`, not in the console's default set
   (`CAP_LEGACY_FULL`). Only a grant naming them reaches inferd — the
   operator's (`run /bin/agent sys_view,propose,ipc:6-7,fs_read /etc/ai --
   propose`) or init's config (`service inferd ... caps=ipc:6-7,fs_read`).
5. **`service` leaves the default set.** The supervisor's ADMIN right is
   checked for the whole class (`svc_report`, `init_ctl` use
   `ResourceScope::ANY`), so a Service handle carrying it cannot be scoped —
   a default set with `service` in it would reach every channel. It is
   granted only by naming it (init holds it; the legs that exercise
   `svc_report` already named it).
6. **Every service names its channels.** init holds every channel bit and
   gives each service exactly its own: tickd `ipc:2-3`, inferd `ipc:6-7`;
   the kernel's V0.7 echo service and its client get channels 0–1.

## Consequences

- An ordinary program — any console `run` without a caps list, any
  default-set process, any service or package that does not name them —
  can no longer read inferd's requests or answer in its place
  (`ipc-scope-bios`: refused `kind=service reason=scope_denied`).
- IPC is still unauthenticated among the holders of a channel: the agent
  and inferd both hold 6–7, and a second program the operator grants
  `ipc:6-7` could still interfere. The nonce and the kernel's recomputation
  remain the defence there.
- The capability values printed for processes now include their channel
  bits (`caps=0xc0002` for tickd, `0xc00012` for inferd, `0xff0413` for
  init); legs that pinned the old values were updated.
- A grant of more than one disjoint channel range is not expressible: one
  handle per kind, one scope per handle. No program needs one today.
