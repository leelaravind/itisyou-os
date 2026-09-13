//! `tcp-server-probe` — a Ring 3 TCP server (V0.9).
//!
//! Listens on port 7 (`tcp_listen`), waits for a peer, echoes everything it
//! sends and closes after the peer does — a passive open and a passive close,
//! driven entirely from Ring 3. The harness forwards a host port to guest
//! port 7 and connects from the HOST's own TCP stack once it sees
//! `TCPSERVE-LISTENING`, checking the echo itself.
//!
//! Without the `network` capability the listen must be refused; the probe
//! reports `TCPSERVER-DENIED` and exits 3.

#![no_std]
#![no_main]

use ulib::{
    exit, tcp_close, tcp_listen, tcp_recv, tcp_send, tcp_state, uptime_ticks, write, write_u64,
    yield_now, ERR_AGAIN, ERR_PERM, TCP_CLOSED, TCP_CLOSING, TCP_CONNECTING, TCP_ESTABLISHED,
};

const PORT: u16 = 7;
/// Real-time limits (100 ticks per second).
const ACCEPT_TICKS: u64 = 60 * 100;
const STREAM_TICKS: u64 = 60 * 100;
const CLOSE_TICKS: u64 = 20 * 100;

fn fail(step: &str) -> ! {
    write("TCPSERVER-FAILED step=");
    write(step);
    write("\n");
    exit(1)
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let sock = tcp_listen(PORT);
    if sock == ERR_PERM {
        write("TCPSERVER-DENIED call=tcp_listen\n");
        exit(3)
    }
    if sock > 64 {
        fail("listen")
    }
    write("TCPSERVE-LISTENING port=7 ring=3\n");
    let deadline = uptime_ticks() + ACCEPT_TICKS;
    loop {
        match tcp_state(sock) {
            TCP_ESTABLISHED => break,
            TCP_CONNECTING => {}
            _ => fail("accept"),
        }
        if uptime_ticks() > deadline {
            fail("accept-timeout")
        }
        yield_now();
    }
    write("TCPSERVER-ACCEPTED\n");

    let mut buf = [0u8; 512];
    let mut echoed = 0u64;
    let deadline = uptime_ticks() + STREAM_TICKS;
    loop {
        let n = tcp_recv(sock, &mut buf);
        if n == 0 {
            break; // the peer closed and everything it sent has been read
        }
        if n != ERR_AGAIN {
            if n as usize > buf.len() {
                fail("recv")
            }
            let data = &buf[..n as usize];
            let mut off = 0usize;
            while off < data.len() {
                let m = tcp_send(sock, &data[off..]);
                if m != ERR_AGAIN {
                    if m as usize > data.len() - off {
                        fail("send")
                    }
                    off += m as usize;
                }
                if uptime_ticks() > deadline {
                    fail("send-timeout")
                }
                yield_now();
            }
            echoed += n;
        }
        if uptime_ticks() > deadline {
            fail("stream-timeout")
        }
        yield_now();
    }
    write("TCPSERVER-ECHOED bytes=");
    write_u64(echoed);
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
        yield_now();
    }
    write("TCPSERVER-OK\n");
    exit(0)
}
