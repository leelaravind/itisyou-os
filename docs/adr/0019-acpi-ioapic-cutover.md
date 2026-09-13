# ADR-0019: ACPI discovery and the I/O APIC cutover

## Status

Accepted and verified for V0.9.

## Context

Until V0.9 the timer and PS/2 lines were delivered by the legacy 8259 PICs.
V0.8 (ADR-0017) enabled the local APIC and proved MSI-X, but only programmed and
read back one *masked* I/O APIC entry, at the architectural address, because the
kernel did not parse ACPI and so did not know which global system interrupt
(GSI) each ISA IRQ arrives on. On QEMU's `pc` machine the PIT's IRQ0 arrives on
GSI 2, not GSI 0 — routing by guesswork would have taken the timer away.

## Decision

1. **ACPI tables are parsed, not trusted.** `kernel_core::acpi` (host-tested, 10
   tests) validates the RSDP (v1 and v2 checksums), the RSDT/XSDT, and each
   table's signature, declared length and checksum; the MADT walker refuses
   zero-length and overrunning entries and skips unknown types by length; the
   `\_S5_` finder decodes only a literal package and refuses anything it would
   have to guess. The kernel reads firmware bytes through the physical-memory
   window only after confirming every page of the range is mapped.
2. **Cutover order.** With interrupts disabled: program the I/O APIC entries for
   ISA IRQ 0, 1 and 12 on the MADT's GSIs (keeping the PIC-era vectors 0x20,
   0x21, 0x2C so the IDT does not change, and each line's existing mask state),
   verify each by read-back, mask both 8259s *and* the local APIC's LINT0 (the
   virtual-wire input the PIC reached the CPU through), then switch the EOI
   target. A route that does not read back aborts the cutover and leaves the PIC
   in charge. No MADT means no cutover.
3. **EOI is lock-free.** The local APIC base is published in an atomic; ISRs
   write the EOI register directly and never take the APIC mutex, which the
   interrupted code may hold.
4. **The tick rate is measured across the cutover** (100 ms on the TSC before
   and after) and logged as `rate_preserved=true|false`.
5. **The PIT runs in mode 2 (rate generator)** instead of mode 3 (square wave).
6. **ACPI S5 power-off** writes `SLP_TYP|SLP_EN` to the FADT's PM1 control
   block(s), enabling ACPI mode through `SMI_CMD` first when firmware has not.

## Consequences

- The first cutover run measured `before=10 after=20` ticks per 100 ms: through
  QEMU's edge-triggered I/O APIC the PIT's mode-3 square wave was delivered
  twice per period, which would have halved every scheduler quantum and every
  deadline without failing any functional test. Mode 2 gives one edge per
  period; the rate check is now part of every boot and asserted by the `irq`
  leg.
- The whole matrix runs on the I/O APIC path: preemption, PS/2 keyboard and
  mouse, and every timeout. The `irq` leg additionally masks the timer's I/O APIC
  entry and requires the tick count to stop (`masked_ticks=0`), then resume —
  so nothing else can still be delivering the timer.
- Still single-CPU: no IPIs, no AP startup, no x2APIC, and the local APIC timer is
  not the scheduler tick. Only three ISA lines are routed; other devices remain
  polled or MSI-X.
- The `\_S5_` finder is a pattern match, not an AML interpreter. It works for the
  static packages QEMU's SeaBIOS and OVMF tables use and refuses anything else.

## Evidence

`irq-bios` (routes, `pic_retired masks=0xff/0xff lint0=masked`,
`rate_preserved=true`, `masked_ticks=0 … result=ioapic_only`, APIC timer and
MSI-X still delivered), `acpi-poweroff-bios` (ACPI 1.0, RSDT, PM1a 0x604) and
`acpi-poweroff-uefi` (ACPI 2.0, XSDT, PM1a 0xb004), plus every other leg of the
matrix running on the new path.
