# ADR-0010: Framebuffer graphics, window compositor, and PS/2 input

**Status:** Accepted · 2026-09-02 (V0.5)

## Context

V0.4 delivered a headless multitasking kernel driven entirely over the serial
console. V0.5 gives ITISYOU OS a real graphical environment: the OS itself must
render a desktop into a framebuffer inside QEMU, accept keyboard and mouse
input, run a Ring 3 graphical program without breaking process isolation, and
prove all of it with automated verification — never a host-rendered mock-up.

## Decision — graphics

- **Bootloader-provided linear framebuffer.** The `bootloader` crate already
  hands the kernel a linear framebuffer (firmware-chosen mode; QEMU gives
  1280×720, `Bgr`, 3 bytes/pixel). The kernel queries its real dimensions and
  format at runtime and never hardcodes them. No GPU driver is written — a
  linear framebuffer is the smallest correct graphics architecture and is
  honest for V0.5 (hardware acceleration / Intel Iris Xe is explicitly out of
  scope).
- **`u32` back buffer + single `present()`.** All drawing targets an
  in-heap `Vec<u32>` (`0x00RRGGBB`) back buffer; one `present()` converts to
  the framebuffer's real pixel format (BGR/RGB, bpp) and copies. This avoids
  tearing and isolates every format concern to one place. The heap was grown
  to 32 MiB to hold the back buffer plus window backing stores.
- **Bitmap font + primitives.** An 8×8 bitmap font and `fill_rect` / `draw_char`
  / `draw_string` / `blit` primitives live in the kernel; the font table and
  the scancode/mouse decoders live in host-tested `kernel_core`. A CRC-32
  `hash_region` gives deterministic framebuffer assertions.

## Decision — compositor

- **Windows own their pixels.** Each `Window` has an `owner_pid` and its own
  RGB backing buffer. `composite()` renders wallpaper → top bar → windows
  (back-to-front, with border + title bar) → cursor, then presents.
- **Ownership + bounds enforced.** Every window operation validates the caller
  owns the window (pid 0 = kernel) and that the rectangle/text stays inside the
  window. A process therefore cannot draw into another process's window or
  outside its own — the same isolation guarantee as memory, extended to the
  screen. On process exit the kernel removes that pid's windows.

## Decision — GUI syscalls

- Ring 3 has **no direct framebuffer access**. Four syscalls
  (`gui_create` / `gui_fill` / `gui_text` / `gui_present`) take request structs
  copied out of validated user memory; the kernel applies them under the
  compositor's ownership/bounds checks. A userspace program (`user/gui-demo`)
  renders a window entirely through these syscalls.

## Decision — input

- **PS/2 keyboard (IRQ1) + mouse (IRQ12)** via the i8042 controller. The IRQ
  handlers feed host-tested decoders (scancode set 1; a 3-byte mouse packet
  state machine that resynchronises on the always-1 bit). Every event is also
  emitted as a machine-readable `[ITISYOU:INPUT]` serial marker.
- **ISR/lock discipline.** The mouse IRQ must never touch the compositor or
  framebuffer locks (a `composite()` on the main thread may hold them), so it
  only accumulates deltas atomically; the desktop loop applies them. Enabling
  the input IRQs masks interrupts around the PIC write to avoid a timer-ISR
  deadlock on the shared PIC lock.

## Decision — verification (no faked UI)

- The graphics/compositor/GUI path is asserted in the in-kernel selftest with
  pixel read-backs (including a Ring 3 process's window colour read back off
  the composited screen) and adversarial rejections (cross-owner, out-of-bounds,
  bad size).
- Real input is proven by a QEMU-**monitor**-driven test: the harness injects
  `sendkey` / `mouse_move` / `mouse_button` into the emulated PS/2 devices, the
  kernel's IRQ handlers emit `[ITISYOU:INPUT]` markers, and the compositor
  captures a framebuffer `screendump` — the desktop is produced by the OS
  inside QEMU, not by the host.

## Alternatives

- **virtio-gpu / a real GPU driver:** far more code; a linear framebuffer is
  sufficient and honest for a desktop foundation and can be replaced behind the
  same `gfx` interface later.
- **Reading PS/2 status bit 5 (aux-data) in a shared IRQ handler:** the split
  IRQ1/IRQ12 handlers are simpler and correct for QEMU's routing; the first
  mouse packet after enable can carry a benign sync artifact (see
  KNOWN_LIMITATIONS) — subsequent packets decode exactly.
- **Persistent Ring 3 windows on the live desktop:** would need asynchronous
  process spawning so a non-exiting GUI app and the desktop loop co-schedule;
  deferred. V0.5 proves Ring 3 rendering + isolation in the selftest and the
  live desktop with kernel-rendered windows + real input.

## Consequences

- ITISYOU OS boots into a real graphical desktop in QEMU, accepts real keyboard
  and mouse input through the PS/2 IRQ path, and runs a Ring 3 GUI program with
  full isolation — all machine-verified while V0.1–V0.4 stay green.
- Limitations (V0.5): single framebuffer mode (no mode-setting), no window
  drag/focus/z-order UI, no compositing of a persistent Ring 3 window on the
  live desktop, software rendering only, no GPU acceleration. Tracked in
  KNOWN_LIMITATIONS and ROADMAP.
