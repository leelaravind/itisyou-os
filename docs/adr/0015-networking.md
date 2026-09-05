# ADR-0015: Networking — polled NIC, strict parsers, capability-scoped sockets

## Status

Accepted and verified for V0.8 (`net-bios`).

## Context

A network stack is the largest attack surface an OS exposes: every byte comes
from someone else, and most of it arrives before any authentication exists to
judge it by. It is also the subsystem where "it works" is easiest to fake — a
ping that succeeds proves the happy path and nothing about what happens when
the peer is hostile, or about whether the kernel put correct bytes on the wire
or merely bytes the same kernel was willing to read back.

## Decision

**Parsers live in `kernel-core`, host-tested, and refuse rather than guess.**
`net::{checksum, eth, arp, ipv4, icmp, udp, dns}` are allocation-free, borrow
from the caller's DMA buffer, and return a distinct error for every rejection
so the driver can count and log the reason. Fragments are refused rather than
reassembled (a reassembly engine is attacker-driven state, and the historical
bug list is long); VLAN tags are refused rather than skipped; ICMP types other
than echo are refused rather than acted on; DNS compression pointers cost from
a fixed jump budget so a decompression bomb terminates.

**The driver is polled.** The receive path is drained from a scheduling slice,
not an ISR, continuing the pattern the storage, audio and USB drivers already
follow. A polled driver has no interrupt-context locking rules to get wrong and
leaves the verified V0.7 PIC path untouched. MSI delivery is proved separately
(ADR-0017) without moving the data path into an interrupt handler.

**Datagram syscalls never block.** Inside a syscall `SFMASK` has cleared IF, so
no interrupt can arrive: a bounded wait there freezes the machine for its
duration and cannot be preempted. `udp_send` returns `ERR_AGAIN` when ARP has
not resolved, having kicked off a (rate-limited) request, and the caller
retries on its own scheduling slice where yielding is free. The one exception
is `net_resolve`, which is documented as blocking and bounded short.

**Authority is a capability, scoped to a port.** `udp_bind` is checked against
the Network handle with the port as the resource, so a handle narrowed to a
range cannot listen outside it. Every later datagram call re-checks the
socket's own port, so a handle revoked or narrowed *after* the bind stops
working at the next use — which is the whole point of handles over static bits.

**The test harness brings its own network.** `tools/qemu-runner/src/wire.rs`
answers ARP, ICMP, UDP echo and DNS over a `dgram` netdev. It is deterministic
and offline (no slirp, no host resolver, no DHCP), and it is an *independent*
byte-level implementation — deliberately not `kernel_core::net` — because a
test where both ends share a checksum routine proves the two agree, not that
either is right.

## Consequences

The guest is verified in both directions. Its client paths (ARP resolution,
ICMP echo, a Ring 3 UDP round trip, DNS) and its responder paths (answering the
peer's ARP request and ping) are separately proved; a stack that only ever
initiates is not a host on a network.

Refusal is proved as explicitly as function. The peer sends five frames a
correct stack must reject — a corrupt IP checksum, a ping addressed to another
host but delivered to our MAC, UDP to an unbound port, an 802.1Q tag, and an
ARP whose hardware type contradicts its address lengths — and the assertions
are that the guest counts them as refused *and answers none of them*.

Three bugs came out of running it rather than reading it, and each is a class
worth naming: a `match` on a `Mutex` guard that re-locked in one of its arms
(self-deadlock with interrupts off); a bounded wait built on the timer tick,
which does not advance inside a syscall; and a retry loop that emitted one ARP
broadcast per attempt.

## Limitations

No TCP: the stack is datagram-only. No IPv6, no DHCP (the address plan is
static), no routing beyond a single gateway, no fragmentation in either
direction, and no ICMP error generation — an unreachable port is dropped
silently rather than answered, which is also what makes "answers nothing
hostile" a checkable property.

## Evidence

`artifacts/qemu/net-bios.result.json`, and the host peer's own tally in the
same serial log: `guest_arp_replies=1 guest_icmp_replies=1 hostile_sent=true
replies_to_hostile=0 bad_ip_csum=0 bad_udp_csum=0 bad_icmp_csum=0`.
