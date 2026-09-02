# Unsafe inventory — kernel `unsafe` blocks and their contracts

Every `unsafe` in privileged code is listed here with its safety invariant
(requirement SEC-001). Update in the same change that adds/modifies the
block. Host tools using safe std are not tracked.

| # | Location | Operation | Safety contract |
|---|---|---|---|
| 1 | `kernel/src/serial.rs::init` | `SerialPort::new(0x3F8)` | 0x3F8 is COM1 on the targeted QEMU pc/q35 machine models; port I/O is side-effect-contained to the UART |
| 2 | `kernel/src/lib.rs::panic` | re-creating `SerialPort` in the panic path | bypasses the global lock deliberately so a panic can never deadlock on serial; output interleaving during panic is acceptable; `init()` re-runs full port setup |
| 3 | `kernel/src/qemu.rs::exit` | write to port `0xF4` | project-configured `isa-debug-exit` device; on hardware without it the write is inert and the function falls through to `hlt` |

Notes:
- `entry_point!` macros generate the actual `_start` symbols; their contract
  is upheld by the `bootloader_api` crate version pinned in `Cargo.lock`.
- New subsystems (paging, context switch, port-IO drivers) must extend this
  table in the same commit that introduces the `unsafe`.
