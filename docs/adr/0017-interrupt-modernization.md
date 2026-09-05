# ADR-0017: Interrupt modernization — APIC alongside the PIC, and what "MSI works" means

## Status

Accepted and verified for V0.8 (`irq-bios`).

## Context

Everything V0.1–V0.7 depends on — the preemption timer, PS/2 keyboard and
mouse — is delivered by the legacy 8259 PIC, and all of it is verified. The
roadmap called for APIC/IOAPIC and MSI/MSI-X. The tempting reading of that is
"replace the PIC", and it is the wrong one: swapping a verified interrupt path
for a new one, in the same milestone that first brings the new one up, risks
every earlier guarantee to gain nothing this milestone needs.

## Decision

**The APIC comes up alongside the PIC, not instead of it.** The local APIC is
enabled with its own spurious vector and handler; the PIC keeps serving IRQ0,
IRQ1 and IRQ12. APIC vectors live at 0x41–0x43 and 0xFF, well clear of the
PIC's 32–47 window, so the two controllers cannot collide.

**Delivery is proved, not configured.** Two of the three claims are an
interrupt actually arriving:

* a one-shot local-APIC timer interrupt on vector 0x41, counted by its handler;
* an MSI-X message on vector 0x42, raised by the completion of a **real NVMe
  block read**. Using genuine I/O rather than a synthetic poke matters: it is
  the same path a driver would depend on, and it exercises the whole chain —
  local APIC enabled, message address encoding the right destination, the
  MSI-X table entry unmasked, bus mastering live.

**The I/O APIC is programmed and read back, and left masked.** Its identity and
redirection-entry register window are exercised (a written entry is read back
and compared), but the line it describes stays with the PIC. Two controllers
delivering the same IRQ would be worse than one. The evidence line says
`masked=true` precisely so it cannot be mistaken for line-based delivery.

**The NIC does not carry the MSI proof.** QEMU's 82540EM exposes no MSI
capability at all, so `nic_msi=false` at boot is the expected state rather than
a failure, and the boot line says which devices *are* MSI-X capable. Naming the
device that carries the proof beats a bare "MSI supported".

**Handlers touch no device state.** The MSI handler counts and acknowledges;
the NVMe and network data paths stay polled (ADR-0015). Moving a data path into
an interrupt handler would import a whole class of interrupt-context locking
rules, and this milestone needs the *delivery* proof, not the throughput.

## Consequences

The interrupt cause register still has to be retired somewhere, so the polling
side does it. That is stated in the code rather than left as a trap: without
it, a device raises one message and then goes quiet, which looks exactly like
MSI not working.

## Limitations

Line-based IRQs are not routed through the I/O APIC, so this is not yet a
system that could run without the PIC. There is no ACPI MADT parsing — the I/O
APIC's address is the architectural one, and that assumption is written down in
the code. Single CPU only: no IPIs, no AP startup, no per-CPU APIC state. No
APIC timer as the scheduler tick.

## Evidence

`artifacts/qemu/irq-bios.result.json`: `lapic_ready id=0`, an I/O APIC
redirection entry with `readback=ok masked=true`, `apic_timer delivered=true
count=1 vector=0x41`, `msix armed=true block_read=true delivered=true`, and
`spurious=0`, alongside the PIC's own tick and preemption counters proving the
legacy path is still the one doing the work.
