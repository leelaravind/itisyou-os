//! Program-arguments probe (V0.10, PROC10-001).
//!
//! Prints exactly what the kernel says this process's arguments are —
//! `ARGS-COUNT n=<n>`, then `ARGS-ITEM i=<i> value=<arg>` for each — checks
//! the `args` syscall's edge cases, and finishes with `ARGS-OK`. The first
//! argument can select a second role:
//!
//! * `spawn-child` — PARENT. Spawns `/bin/args-probe child delta echo-7`
//!   through `spawn_args`, waits, and requires the child's distinctive exit
//!   status (which the child only produces after checking its arguments).
//!   Then it hands the kernel six hostile argument blocks; each must be
//!   refused with the documented error and no child may come into being.
//!   Launched WITHOUT the Process capability it must instead be refused at
//!   the capability gate, before its arguments are even looked at.
//! * `child` — CHILD. Requires exactly `child delta echo-7` and exits 42.
//!
//! The child is spawned with NO capabilities, so reading its arguments also
//! shows that `args` needs none.

#![no_std]
#![no_main]

use ulib::{
    args, exit, raw_syscall, spawn_args, split_args, wait, write, write_raw, write_u64,
    ARGS_BLOCK_MAX, ARGS_MAX, ERR_2BIG, ERR_FAULT, ERR_INVAL, ERR_NOENT, ERR_PERM, SYS_ARGS,
    SYS_SPAWN_ARGS,
};

const SELF: &str = "/bin/args-probe";
const CHILD_ARGS: &[u8] = b"child\0delta\0echo-7\0";
const CHILD_EXPECTED: [&[u8]; 3] = [b"child", b"delta", b"echo-7"];
/// Only a child that verified its arguments exits with this.
const CHILD_EXIT: u64 = 42;

fn is_err(v: u64) -> bool {
    v > u64::MAX - 16
}

fn write_bytes(b: &[u8]) {
    write_raw(b.as_ptr() as u64, b.len() as u64);
}

fn write_err(v: u64) {
    // ERR_* codes are u64::MAX - k; print k, which is what the kernel names.
    write_u64(u64::MAX - v);
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut buf = [0u8; ARGS_BLOCK_MAX];
    let len = args(&mut buf);
    if is_err(len) || len as usize > ARGS_BLOCK_MAX {
        write("ARGS-FAILED step=args err=");
        write_err(len);
        write("\n");
        exit(1);
    }
    let block = &buf[..len as usize];

    write("ARGS-COUNT n=");
    write_u64(split_args(block).count() as u64);
    write("\n");
    for (i, arg) in split_args(block).enumerate() {
        write("ARGS-ITEM i=");
        write_u64(i as u64);
        write(" value=");
        write_bytes(arg);
        write("\n");
    }

    if !check_args_syscall(block) {
        exit(1);
    }

    match split_args(block).next() {
        Some(b"spawn-child") => {
            if !parent() {
                exit(1);
            }
        }
        Some(b"child") => {
            if check_child(block) {
                write("ARGS-CHILD-OK n=3\n");
                exit(CHILD_EXIT);
            }
            write("ARGS-CHILD-MISMATCH\n");
            exit(1);
        }
        _ => {}
    }
    write("ARGS-OK\n");
    exit(0)
}

/// The `args` syscall's own contract, beyond the happy path.
fn check_args_syscall(block: &[u8]) -> bool {
    let len = block.len();
    if len == 0 {
        // Nothing to truncate and nothing to copy — so a NULL, zero-length
        // buffer is a valid request that must return 0. The kernel may not
        // touch the pointer at all (forming even an empty slice from NULL is
        // undefined behaviour, which a debug kernel turns into a panic).
        let r = raw_syscall(SYS_ARGS, 0, 0, 0);
        if r != 0 {
            write("ARGS-FAILED step=null_empty err=");
            write_err(r);
            write("\n");
            return false;
        }
        write("ARGS-NULL-EMPTY-OK\n");
        return true;
    }
    // One byte short: refused whole, nothing written.
    let mut short = [0xAAu8; ARGS_BLOCK_MAX];
    let r = args(&mut short[..len - 1]);
    if r != ERR_2BIG || short.iter().any(|&b| b != 0xAA) {
        write("ARGS-FAILED step=short_buffer err=");
        write_err(r);
        write("\n");
        return false;
    }
    write("ARGS-2BIG-OK\n");
    // Exactly the right size: the same block again.
    let mut exact = [0u8; ARGS_BLOCK_MAX];
    if args(&mut exact[..len]) != len as u64 || &exact[..len] != block {
        write("ARGS-FAILED step=exact_buffer\n");
        return false;
    }
    // A kernel address is refused by validation, never written.
    let r = raw_syscall(SYS_ARGS, 0xffff_8000_0000_0000, ARGS_BLOCK_MAX as u64, 0);
    if r != ERR_FAULT {
        write("ARGS-FAILED step=kernel_pointer err=");
        write_err(r);
        write("\n");
        return false;
    }
    write("ARGS-FAULT-OK\n");
    true
}

fn check_child(block: &[u8]) -> bool {
    let mut n = 0usize;
    for (i, arg) in split_args(block).enumerate() {
        if i >= CHILD_EXPECTED.len() || arg != CHILD_EXPECTED[i] {
            return false;
        }
        n += 1;
    }
    n == CHILD_EXPECTED.len()
}

/// Parent role: one legitimate spawn with arguments, then hostile blocks.
fn parent() -> bool {
    let pid = spawn_args(SELF, 0, CHILD_ARGS);
    if pid == ERR_PERM {
        // Launched without the Process capability: the gate refused the call
        // before the arguments were considered, exactly as for spawn_caps.
        write("ARGS-SPAWN-DENIED call=spawn_args\n");
        return true;
    }
    if is_err(pid) {
        write("ARGS-SPAWN-FAILED err=");
        write_err(pid);
        write("\n");
        return false;
    }
    let status = wait(pid);
    if status != CHILD_EXIT {
        write("ARGS-SPAWN-FAILED status=");
        write_u64(status);
        write("\n");
        return false;
    }
    write("ARGS-SPAWN-OK child_status=");
    write_u64(status);
    write("\n");

    // 17 one-byte arguments: one past the count limit, well under the size.
    let mut many = [0u8; 2 * (ARGS_MAX + 1)];
    for (i, byte) in many.iter_mut().enumerate() {
        if i % 2 == 0 {
            *byte = b'a';
        }
    }
    // One 600-byte argument: past the 512-byte block limit.
    let mut long = [b'x'; 601];
    long[600] = 0;
    let cases: [(&str, &[u8], u64); 6] = [
        ("too_many", &many, ERR_2BIG),
        ("too_long", &long, ERR_2BIG),
        ("non_printable", b"bell\x07\0", ERR_INVAL),
        ("space", b"two words\0", ERR_INVAL),
        ("empty", b"ok\0\0", ERR_INVAL),
        ("unterminated", b"abc", ERR_INVAL),
    ];
    let mut refused = 0u64;
    for (name, block, expected) in cases {
        let r = spawn_args(SELF, 0, block);
        if r == expected {
            write("ARGS-HOSTILE-REFUSED case=");
            write(name);
            write("\n");
            refused += 1;
        } else if !is_err(r) {
            // A child exists that should never have been created.
            write("ARGS-HOSTILE-ACCEPTED case=");
            write(name);
            write("\n");
            wait(r);
        } else {
            write("ARGS-HOSTILE-WRONG-ERROR case=");
            write(name);
            write(" err=");
            write_err(r);
            write("\n");
        }
    }
    if refused != cases.len() as u64 {
        return false;
    }
    write("ARGS-HOSTILE-OK refused=");
    write_u64(refused);
    write("\n");

    // A NULL, zero-length block is a valid EMPTY argument list; the kernel
    // must accept it without touching the pointer. Aimed at a path that does
    // not exist, so the call gets past the block and fails at the load —
    // ERR_NOENT proves the block was accepted and no child was created.
    let missing = "/bin/no-such-program";
    let req: [u64; 3] = [0, 0, 0];
    let r = raw_syscall(
        SYS_SPAWN_ARGS,
        missing.as_ptr() as u64,
        missing.len() as u64,
        req.as_ptr() as u64,
    );
    if r != ERR_NOENT {
        write("ARGS-FAILED step=null_block err=");
        write_err(r);
        write("\n");
        return false;
    }
    write("ARGS-NULL-BLOCK-OK\n");
    true
}
