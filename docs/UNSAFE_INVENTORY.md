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

Notes:
- `entry_point!` macros generate the `_start` symbols; their contract is
  upheld by the pinned `bootloader_api` version.
- The `#[allow(clippy::vec_box)]` on the scheduler's task list is a
  documented aliasing-stability requirement, not style.
