# ADR-0011: Device model, PCI foundation, USB, audio, and input unification

**Status:** Accepted · 2026-09-02 (V0.6)

## Context

V0.1–V0.5 gave the OS storage (NVMe) and PS/2 input, but each driver was wired
by hand and there was no generic device architecture, no USB, and no audio.
V0.6 turns ITISYOU OS into a stronger hardware platform: a real device/driver
model, an enriched PCI foundation, verified USB and audio, and a unified input
subsystem — all inside QEMU, never touching the physical host.

## Decision — device/driver model

- **`Device` = identity + sized BARs + capability list.** Every PCI function is
  probed into a `Device`: the `PciId`, its six BARs sized by the standard
  write-all-ones/read-back-mask procedure (with I/O + memory decode disabled
  during the probe and the original values restored — non-destructive), and its
  config-space capability list walked with a bounded, loop-guarded walker.
- **`Driver` trait + explicit registry.** A driver declares `probe` (side-
  effect-free match) and `attach` (initialize once). Binding is deterministic:
  the model probes each registered driver against each device in order. There
  is no dynamic registration, so enumeration and binding are reproducible.
- **Queryable device table.** The result is surfaced to serial diagnostics
  (B190), the `lsdev` shell command, and a userspace `devinfo` syscall.

## Decision — PCI foundation

- BAR sizing (32- and 64-bit), capability-list decoding (MSI 0x05 / MSI-X 0x11 /
  PCIe / PM / vendor), multi-function enumeration, and structured diagnostics.
  The pure decoding + the loop-guarded capability walk live in host-tested
  `kernel_core::pci`, with adversarial tests for circular/self-referential/out-
  of-range capability chains and malformed BAR masks.

## Decision — USB (UHCI + HID)

- **UHCI, not xHCI, first.** UHCI is the smallest controller QEMU emulates fully
  with `usb-kbd`/`usb-mouse`, so a HID class driver can actually be *verified*
  end to end. xHCI is the recommended follow-up (V0.7). The driver resets the
  controller + root port, then enumerates the device over polled control
  transfers (SETUP/DATA/STATUS TDs behind a QH in the frame list):
  GET_DESCRIPTOR(device) → GET_DESCRIPTOR(config) → SET_ADDRESS →
  SET_CONFIGURATION → SET_PROTOCOL(boot). It then reads HID boot reports off the
  interrupt-IN endpoint. Descriptor parsing + HID decode are host-tested
  `kernel_core::usb`.

## Decision — audio (AC97)

- **AC97 output, proven by the captured samples.** The driver initializes the
  codec (reset, unmute, full volume), then plays a synthesized 16-bit stereo
  tone by describing DMA buffers to the PCM-Out bus master through a Buffer
  Descriptor List and confirming the controller consumes every buffer (CIV
  advances to LVI, DMA halts). The harness routes AC97 output to QEMU's `wav`
  backend and asserts the captured WAV is non-silent — the samples must
  actually traverse driver → controller → output, never "success from init".

## Decision — input unification

- PS/2 (IRQ-driven) and USB HID (polled) both decode into the **same**
  `InputEvent` queue; the graphical desktop consumes either identically and is
  source-agnostic (events are tagged `src=usb`/PS-2 for diagnostics only). This
  is "event delivery to GUI processes" behind one abstraction.

## Decision — interrupts stay polled (least risk)

- Every V0.6 driver (NVMe, AC97, UHCI) is **polled**. No device IRQ is wired, so
  the verified PIC timer + PS/2 IRQ path is untouched. APIC/IOAPIC/MSI is
  *researched* (MSI/MSI-X capabilities are detected and reported) but
  deliberately deferred — replacing a verified interrupt path without an
  equivalent test suite would be a regression risk, not progress.

## Decision — userspace hardware access

- Ring 3 gets device information only through the `devinfo` syscall, which
  copies a fixed record out of the kernel device table into a validated user
  buffer. Applications never touch config space, MMIO, or I/O ports — least
  authority, consistent with the memory/GUI isolation model.

## Alternatives

- **xHCI first:** the modern standard, but far more state (64-byte contexts,
  command/event/transfer rings, DCBAA, scratchpad) — a poor first target when
  the goal is *verified* HID input. Deferred with a clear rationale.
- **Interrupt-driven drivers now:** would need APIC/MSI plumbing and a rewrite
  of the timer/PS-2 path's tests; polling is correct and sufficient in QEMU.
- **Intel HD Audio instead of AC97:** heavier codec/verb model; AC97 is the
  smallest correct PCM-out path and the `wav` backend proves it.

## Consequences

- A generic device model binds real drivers over enriched PCI discovery; USB
  HID keyboard input and AC97 audio output are machine-verified in QEMU; input
  is unified; userspace reaches devices safely — all while V0.1–V0.5 stay green.
- Limitations (V0.6): single-device USB enumeration (first connected port), no
  USB mouse-on-a-shared-bus enumeration, no bulk/isochronous USB transfers,
  no audio input/capture, polled-only (no device IRQ/APIC/MSI), no multi-bus
  PCI bridge recursion (unnecessary on the QEMU `pc` machine). Tracked in
  KNOWN_LIMITATIONS and ROADMAP.
