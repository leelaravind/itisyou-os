//! A Ring 3 hardware-listing tool. It enumerates devices purely through the
//! `devinfo` syscall — no port I/O, no MMIO, no direct hardware authority — and
//! prints each device's identity, proving userspace reaches device information
//! only through the kernel's mediated interface (least authority).

#![no_std]
#![no_main]

use ulib::{devinfo, exit, write, write_hex16, write_u64};

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut buf = [0u8; 16];
    let mut count = 0u64;
    let mut index = 0u64;
    loop {
        let r = devinfo(index, &mut buf);
        if r != 1 {
            break; // 0 = past the end; anything else = error.
        }
        count += 1;
        let vendor = u16::from_le_bytes([buf[6], buf[7]]);
        let device = u16::from_le_bytes([buf[8], buf[9]]);
        write("dev ");
        write_u64(buf[0] as u64); // bus
        write(":");
        write_u64(buf[1] as u64); // slot
        write(".");
        write_u64(buf[2] as u64); // func
        write(" ");
        write_hex16(vendor);
        write(":");
        write_hex16(device);
        write(" class=");
        write_hex16(buf[3] as u16);
        write("/");
        write_hex16(buf[4] as u16);
        if buf[5] == 1 {
            write(" [driver]");
        }
        write("\n");
        index += 1;
        if index > 64 {
            break; // defensive bound
        }
    }
    write("RING3-LSDEV-OK count=");
    write_u64(count);
    write("\n");
    exit(0)
}
