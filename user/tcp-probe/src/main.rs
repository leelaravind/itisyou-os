//! `tcp-probe` — a Ring 3 program that uses a TCP stream end to end (V0.9).
//!
//! It connects to the harness's TCP echo endpoint (`10.0.2.2:47123`, which
//! QEMU's user-mode network maps to the host's loopback, so the peer is the
//! host operating system's own TCP stack), sends 3000 bytes of a known pattern
//! — more than two maximum-size segments — reads the echo back, checks it byte
//! for byte, and closes. The host checks the same pattern independently.
//!
//! Run without the `network` capability, the connect must be refused; the
//! probe reports that as `TCPPROBE-DENIED` and exits 3, so one binary proves
//! both the stream and the authority gate.

#![no_std]
#![no_main]

use ulib::{
    exit, tcp_close, tcp_connect, tcp_recv, tcp_send, tcp_state, uptime_ticks, write, write_u64,
    yield_now, ERR_AGAIN, ERR_PERM, TCP_CLOSED, TCP_CLOSING, TCP_CONNECTING, TCP_ESTABLISHED,
};

const PEER: [u8; 4] = [10, 0, 2, 2];
const PORT: u16 = 47123;
const LEN: usize = 3000;
/// Real-time limits (100 ticks per second), so a missing peer fails the probe
/// instead of hanging it — and so the limit does not depend on how fast the
/// scheduler happens to be spinning. Generous enough for several
/// retransmission timeouts when the loss-injection test drops segments.
const HANDSHAKE_TICKS: u64 = 20 * 100;
const TRANSFER_TICKS: u64 = 60 * 100;
const CLOSE_TICKS: u64 = 20 * 100;

fn pattern(i: usize) -> u8 {
    ((i * 7 + 3) % 251) as u8
}

fn fail(step: &str) -> ! {
    write("TCPPROBE-FAILED step=");
    write(step);
    write("\n");
    exit(1)
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let sock = tcp_connect(PEER, PORT);
    if sock == ERR_PERM {
        write("TCPPROBE-DENIED call=tcp_connect\n");
        exit(3)
    }
    if sock > 64 {
        fail("connect")
    }
    let deadline = uptime_ticks() + HANDSHAKE_TICKS;
    loop {
        match tcp_state(sock) {
            TCP_ESTABLISHED => break,
            TCP_CONNECTING => {}
            _ => fail("handshake"),
        }
        if uptime_ticks() > deadline {
            fail("handshake-timeout")
        }
        yield_now();
    }
    write("TCPPROBE-CONNECTED\n");
    // A zero-length send with a NULL pointer is a valid request for nothing;
    // it must be answered (nothing accepted) without the kernel ever forming
    // a slice from that pointer.
    if ulib::raw_syscall(ulib::SYS_TCP_SEND, sock, 0, 0) != ERR_AGAIN {
        fail("null-send")
    }
    write("TCPPROBE-NULL-SEND-OK\n");

    let mut data = [0u8; LEN];
    for (i, b) in data.iter_mut().enumerate() {
        *b = pattern(i);
    }
    let mut sent = 0usize;
    let mut got = 0usize;
    let mut buf = [0u8; 512];
    let deadline = uptime_ticks() + TRANSFER_TICKS;
    while got < LEN {
        if sent < LEN {
            let n = tcp_send(sock, &data[sent..]);
            if n != ERR_AGAIN {
                if n as usize > LEN - sent {
                    fail("send")
                }
                sent += n as usize;
            }
        }
        let n = tcp_recv(sock, &mut buf);
        if n == 0 {
            fail("early-eof")
        } else if n != ERR_AGAIN {
            if n as usize > buf.len() {
                fail("recv")
            }
            for &b in &buf[..n as usize] {
                if b != pattern(got) {
                    fail("mismatch")
                }
                got += 1;
            }
        }
        if uptime_ticks() > deadline {
            fail("transfer-timeout")
        }
        yield_now();
    }
    write("TCPPROBE-ECHO-OK bytes=");
    write_u64(got as u64);
    write("\n");

    if tcp_close(sock) != 0 {
        fail("close")
    }
    let deadline = uptime_ticks() + CLOSE_TICKS;
    loop {
        match tcp_state(sock) {
            TCP_CLOSED => break,
            TCP_CLOSING | TCP_ESTABLISHED => {}
            _ => fail("close-state"),
        }
        if uptime_ticks() > deadline {
            fail("close-timeout")
        }
        // Keep reading: the peer's FIN is what completes our close.
        let _ = tcp_recv(sock, &mut buf);
        yield_now();
    }
    write("TCPPROBE-CLOSED\n");
    tcp_close(sock);
    write("TCPPROBE-OK\n");
    exit(0)
}
