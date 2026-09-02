# Session checkpoint — resumable state

**Timestamp:** 2026-09-02 ~12:40 Europe/London
**Repository:** `E:\Project\itisyou-os` · branch `main`
**Tags:** `v0.1.0`, `v0.2.0`, `v0.3.0`, `v0.4.0`, `v0.5.0` (on commit 5be3c0c)
**Remote:** https://github.com/leelaravind/itisyou-os (private)
**Milestone:** V0.5 Graphics + Input + Basic Desktop/Compositor — verified +
tagged (selftest pass=78 fail=0; CI run 33628363046 success; live at
os.itisyou.app). Next: V0.6 Hardware Expansion (device model, PCI, USB, audio).

## V0.5 verified additions

- **Graphics** (`kernel/src/gfx/mod.rs`): bootloader linear framebuffer (QEMU
  1280×720 BGR) + `u32` back buffer + 8×8 font (`kernel-core::font`) +
  fill/glyph/blit + CRC `hash_region` + one format-converting `present()`.
  Heap grown to 32 MiB.
- **Compositor** (`kernel/src/gfx/compositor.rs`): windows with per-window RGB
  backing stores, wallpaper + top bar + cursor; ownership + bounds enforced
  (a process cannot draw into another window or out of bounds); windows removed
  on owner exit.
- **GUI syscalls** (`syscall.rs` SYS_GUI_CREATE/FILL/TEXT/PRESENT): Ring 3
  renders a window from validated user-memory request structs, no direct FB.
  `user/gui-demo` prints `RING3-GUI-OK`.
- **Input** (`kernel/src/input/mod.rs`, `kernel-core::{scancode,mouse}`): PS/2
  keyboard (IRQ1) + mouse (IRQ12) via i8042; host-tested decoders; every event
  emits an `[ITISYOU:INPUT]` marker. Mouse IRQ accumulates deltas lock-free.
- **Desktop** (`kernel/src/desktop.rs`, `desktop` shell command): live window
  reacting to keyboard + mouse; `[ITISYOU:MODE] desktop` → `DESKTOP-READY` →
  `DESKTOP-INPUT-VERIFIED`.
- **Harness** (`tools/qemu-runner`): HMP **monitor** channel — `--monitor`,
  `--inject-after <substr>`, `--monitor-cmd <hmp>` — injects real PS/2 input
  (`sendkey`/`mouse_move`/`mouse_button`) and captures `screendump`. New
  `desktop-input-bios` leg proves the graphics + input path; `desktop.ppm` is
  the OS-rendered desktop.
- Stages **B170** (graphics) + **B180** (desktop) added.

## Gates / how to resume

- `scripts/test.ps1` runs the full matrix (host tests + boot/selftest/shell/
  **desktop-input**/panic/fs-persist). `scripts/verify.ps1` = full gate (fmt,
  clippy, doctor, tests, website, secret scan).
- Desktop input test needs the runner `--monitor`/`--monitor-cmd` flags;
  storage FS tests need `--nvme-persist <path>`.
- Environment note: BIOS boot in this QEMU/TCG is slow (~40 s to load the
  bootloader via ATA PIO; UEFI ~12 s). Timeouts are sized for it; no
  acceleration change (keeps validated V0.1–V0.4 behavior).
- Next milestone **V0.6** candidates: window focus/drag/z-order + a broader GUI
  toolkit; compositing a persistent Ring 3 window on the live desktop (needs
  async process spawning); OR networking (NIC/virtio-net). Also pending from
  earlier: second block driver (virtio-blk/AHCI), richer ITFS ops, stack guard
  pages, capability-handle IPC, scheduler priorities.

## Environment keys

nightly-2026-08-01 pin; toolchains on E:; QEMU 11.1.0 at E:\tools\qemu;
TEMP → G:\claude-tmp\tmp; wrangler OAuth on this machine; never mix
kernel/user + host packages in one cargo invocation; user programs build
static no-pie; user pointer validation uses the active CR3
(translate_active); preemption resume is iretq (full GPR restore); input IRQs
enabled with interrupts masked (PIC-lock deadlock avoidance); mouse IRQ must
not take the compositor/FB lock.

## Blockers

None.
