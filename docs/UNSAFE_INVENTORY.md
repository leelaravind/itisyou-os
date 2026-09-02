# Unsafe inventory — kernel `unsafe` blocks and their contracts

Every `unsafe` in privileged code is listed here with its safety invariant
(requirement SEC-001). Update in the same change that adds/modifies a block.
Host tools using safe std are not tracked.

| # | Location | Operation | Safety contract |
|---|---|---|---|
| 1 | `serial.rs::init` | `SerialPort::new(0x3F8)` | 0x3F8 is COM1 on the targeted QEMU machine models; port I/O side-effects are contained to the UART |
| 2 | `serial.rs::try_read_byte` | LSR/RBR port reads | reads of COM1 status + receive registers; consuming the RX byte is the intended effect |
| 3 | `lib.rs::panic` | re-creating `SerialPort` in the panic path | bypasses the global lock deliberately so a panic can never deadlock on serial; interleaving during panic is acceptable; `init()` re-runs full port setup |
| 4 | `qemu.rs::exit` | write to port `0xF4` | project-configured `isa-debug-exit` device; inert on hardware without it, falls through to `hlt` |
| 5 | `memory/paging.rs::init` | deref of L4 table via physical-memory offset; `OffsetPageTable::new` | caller guarantees the bootloader mapped all physical memory at the offset and calls exactly once while that mapping is active |
| 6 | `memory/paging.rs::map_page` | `Mapper::map_to` | frames come exclusively from the PMM (exclusively owned); pages target unused kernel virtual space; W^X enforced before the call |
| 7 | `memory/heap.rs::init` | `ALLOCATOR.lock().init(...)` | `[HEAP_START, +HEAP_SIZE)` mapped immediately beforehand, exclusively owned by the allocator, initialized once |
| 8 | `gdt.rs` (TSS stack, segment loads) | address-of static IST stack; `CS::set_reg`/`SS::set_reg`/`load_tss` | IST stack is a dedicated static used only by the double-fault handler; selectors reference entries of the GDT loaded in the same function |
| 9 | `interrupts.rs` (IDT double-fault IST index; PIC/PIT port programming; EOI) | `set_stack_index`; `ChainedPics::new/initialize/write_masks`; PIT ports 0x43/0x40 | IST index installed by `gdt::init` before `IDT.load()`; architectural PIC/PIT locations on the QEMU pc machine; EOI acknowledges the in-service vector |
| 10 | `task/context.rs::switch_context` | naked fn: callee-saved register save/restore + stack switch | `old_rsp_slot` outlives the switch (boxed Task storage); `new_rsp` seeded by `spawn` or saved by a prior switch of a live task; SysV ABI on `x86_64-unknown-none` |
| 11 | `task/mod.rs::spawn` | seeding a fresh task stack through raw pointers | writes stay within the freshly allocated stack; layout matches exactly what `switch_context` pops (6 regs + return address) |
| 12 | `shell.rs::cmd_reboot` | write `0xFE` to port 0x64 | architectural 8042 CPU-reset pulse, invoked only by the explicit `reboot` command |
| 13 | `syscall.rs::init` | EFER/STAR/LSTAR/SFMASK writes; syscall stack top computed from a static | standard syscall-MSR arming; GDT layout asserted first; stack is a dedicated static, top written once before any Ring 3 code exists |
| 14 | `syscall.rs::syscall_entry` (naked) | stack switch + register save/restore around the dispatcher | IF masked by SFMASK; single CPU, one process → no nesting; user RSP is preserved verbatim and restored before sysretq; alignment maintained for the SysV call |
| 15 | `syscall.rs::sys_write` | `from_raw_parts` over a user buffer | every byte's page verified mapped and the range verified inside the user window BEFORE the slice is formed; the owning process is suspended in this syscall so the mapping cannot change |
| 16 | `user.rs` loader copies (`write_bytes`/`copy_nonoverlapping`) | writes through fresh user mappings | pages just mapped writable+zeroed, exclusively owned by the process being built; bounds checked against the validated ELF segments |
| 17 | `user.rs::transition::enter_user_raw` (naked) | iretq to CPL=3 with constructed frame | selector constants asserted against the GDT at boot; all GPRs zeroed so no kernel data reaches Ring 3; abort context armed first |
| 18 | `user.rs::transition::user_abort_raw` (naked) | setjmp-style long-jump back to the saved kernel frame | only reachable while the context is armed (exit syscall or verified-CPL3 fault); abandons the exception/syscall stack by design; IF re-enabled by the wrapper |
| 19 | `user/ulib::raw_syscall` (Ring 3 side) | `syscall` instruction | userspace code; declares every hardware/kernel-clobbered register to the compiler |
| 20 | `memory/aspace.rs` (table_mut, subtree walk, CR3 write) | building/switching/freeing per-process page tables via the physical alias | frames are owned by the space being built/torn down (or the boot table, read-shared); single CPU; every process L4 shares kernel mappings so CR3 switches don't disturb kernel execution; teardown only walks the process-exclusive L4 entry 0 |
| 21 | `memory/paging.rs::release_boot_identity_mappings` | unlink boot L4 entry 0, reload CR3 | runs after the kernel's own GDT/IDT are live; only stale low-address boot-identity TLB entries are dropped; frames are left reserved, not freed |
| 22 | `memory/paging.rs::map_mmio` / `translate_active` | map device MMIO uncacheable; walk the active CR3 | MMIO window is a fresh kernel-space range (not RAM); translate_active reads the live root read-only while the owning process is suspended in a syscall |
| 23 | `device/pci.rs` + `device/nvme.rs::write_config` | 0xCF8/0xCFC PCI config port I/O | architectural PCI config mechanism; reads are side-effect-free; the one write enables MMIO+bus-master on the discovered NVMe function |
| 24 | `device/nvme.rs` (MMIO reg access, DMA ring access, `submit`, write/flush) | BAR0 volatile MMIO; SQ/CQ/data DMA through the physical alias; `UnsafeCell` queue cursors | BAR mapped uncacheable; DMA frames are exclusively-owned PMM frames zeroed before use; `unsafe impl Sync` justified by single-threaded, single-CPU storage use with no aliasing; command/PRP fields bounds-checked; write stages one block into the owned DMA frame |
| 25 | `interrupts.rs::timer_isr` (naked) + `timer_handler_inner` | push/pop all GPRs around the handler; write the running process's UserContext from the trap frame | interrupt gate (IF masked, no nesting); TrapFrame `#[repr(C)]` matches the push order exactly; preemption only when the frame's CS RPL==3 and a quantum is armed; EOI before the long-jump; the abandoned RSP0 stack is reloaded fresh on the next user→kernel transition |
| 26 | `device/block.rs::RamDisk` (`UnsafeCell<Vec<u8>>`) | interior-mutable read/write backing store | single-threaded test use; `unsafe impl Sync`; bounds-checked per block |
| 27 | `gfx/mod.rs` (`unsafe impl Send for Gfx`; `from_raw_parts_mut(fb_ptr, fb_len)` in `present`) | reconstruct the linear-framebuffer slice to blit the back buffer | ptr+len captured from the bootloader's `FrameBuffer` at `init`; the region is the bootloader-owned linear framebuffer; the slice is formed only under the `GFX` mutex during `present`; single-CPU, and the `u32` back buffer is a separate allocation (no aliasing) |
| 28 | `input/mod.rs` (i8042 port I/O on 0x60 data, 0x64 status/command; `wait_read/wait_write/cmd/mouse_write/init` flush; IRQ-body data reads) | program the PS/2 controller and consume key/mouse bytes | architectural i8042 ports on the QEMU pc machine; status reads are side-effect-free; data-port reads consume the intended byte; command sequences follow the i8042 protocol; single-CPU |
| 29 | `interrupts.rs::keyboard_handler`/`mouse_handler` (`x86-interrupt`) | run the input IRQ body then EOI IRQ1/IRQ12 | interrupt gates; each reads the i8042 data port (entry 28) and posts to a bounded queue; the mouse handler only accumulates deltas atomically and never takes the compositor/framebuffer lock a `composite()` may hold; EOI acknowledges the in-service vector |
| 30 | `device/pci.rs::{write_config, probe_bars}` | 0xCF8/0xCFC config writes for BAR sizing + command register | writes restricted to the command register and to BAR sizing (all-ones probe with the original value always restored, I/O+mem decode disabled during the probe); architectural PCI config mechanism |
| 31 | `device/ac97.rs` (NAM/NABM port I/O; BDL + PCM DMA frames) | program the AC97 codec/bus master + describe PCM buffers | architectural AC97 I/O ports from the device's two I/O BARs; DMA frames are exclusively-owned PMM frames written through the physical alias and freed after playback; the bus master reads only the described BDL/buffer physical addresses; single-CPU, polled |
| 32 | `device/uhci.rs` (UHCI port I/O; frame list, QH/TD, transfer buffers) | init the controller, drive control/interrupt transfers | architectural UHCI I/O registers from the I/O BAR; frame-list/QH/TD/data live in exclusively-owned PMM DMA frames accessed via the physical alias; the controller reads only physical addresses the driver wrote; transfers are bounded/polled with terminate + loop guards; single-CPU |

Notes:
- `entry_point!` macros generate the `_start` symbols; their contract is
  upheld by the pinned `bootloader_api` version.
- The `#[allow(clippy::vec_box)]` on the scheduler's task list is a
  documented aliasing-stability requirement, not style.
