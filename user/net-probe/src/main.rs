//! `net-probe` — an ordinary Ring 3 program that actually uses the network.
//!
//! It exists to prove the whole authorized path end to end from userspace, not
//! from the kernel shell: read the interface, bind a port through the network
//! capability, send a datagram to the peer's echo service, read the reply back
//! and check it byte for byte, then resolve a name over DNS.
//!
//! Every step prints a distinct marker, so a partial failure says which step
//! failed instead of just "no network".

#![no_std]
#![no_main]

use ulib::{
    exit, net_info, net_resolve, udp_bind, udp_close, udp_recv, udp_send, write, write_hex8,
    write_ipv4, write_u64, yield_now, ERR_AGAIN,
};

/// The harness's wire peer answers on these; they mirror QEMU user-mode
/// networking so the same program works against either.
const PEER: [u8; 4] = [10, 0, 2, 2];
const ECHO_PORT: u16 = 7;
const LOCAL_PORT: u16 = 40100;
/// A name the harness's DNS responder is authoritative for.
const NAME: &str = "os.itisyou.app";
/// Bounded so a missing peer fails the probe instead of hanging it.
const RECV_BUDGET: u32 = 200_000;

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // 1. Interface facts. A zero MAC or a down link means there is no point
    //    continuing, and saying so is more useful than a send timeout.
    let mut info = [0u8; 20];
    if net_info(&mut info) != 20 {
        write("NETPROBE-INFO-FAILED\n");
        exit(1)
    }
    write("NETPROBE-IFACE mac=");
    for (i, octet) in info[..6].iter().enumerate() {
        write_hex8(*octet);
        if i < 5 {
            write(":");
        }
    }
    write(" ip=");
    write_ipv4([info[6], info[7], info[8], info[9]]);
    write(" link_up=");
    write_u64(info[19] as u64);
    write("\n");
    if info[19] == 0 {
        write("NETPROBE-LINK-DOWN\n");
        exit(2)
    }

    // 2. Bind. This is the capability check: a program without a network
    //    handle covering this port never gets past here.
    let sock = udp_bind(LOCAL_PORT);
    if sock > 64 {
        write("NETPROBE-BIND-DENIED\n");
        exit(3)
    }
    write("NETPROBE-BOUND port=");
    write_u64(LOCAL_PORT as u64);
    write("\n");

    // 3. Round trip against the peer's echo service.
    const PROBE: &[u8] = b"itisyou-os-v0.8-udp-probe";
    // The first send usually needs an ARP round trip. The kernel refuses to
    // block for it — that would spin with interrupts off and freeze the whole
    // machine — so the retry loop lives here, where yielding is free.
    let mut budget = RECV_BUDGET;
    loop {
        let sent = udp_send(sock, PEER, ECHO_PORT, PROBE);
        if sent == PROBE.len() as u64 {
            break;
        }
        if sent != ERR_AGAIN {
            write("NETPROBE-SEND-FAILED\n");
            exit(4)
        }
        yield_now();
        budget -= 1;
        if budget == 0 {
            write("NETPROBE-SEND-TIMEOUT\n");
            exit(4)
        }
    }
    let mut buf = [0u8; 256];
    budget = RECV_BUDGET;
    let n = loop {
        let n = udp_recv(sock, &mut buf);
        if n != ERR_AGAIN && n <= buf.len() as u64 {
            break n as usize;
        }
        yield_now();
        budget -= 1;
        if budget == 0 {
            write("NETPROBE-RECV-TIMEOUT\n");
            exit(5)
        }
    };
    // The reply must be the same bytes, from the host we sent to. Accepting a
    // reply from anywhere would make this a test of "something answered".
    if n != 6 + PROBE.len() || &buf[6..n] != PROBE {
        write("NETPROBE-BAD-ECHO\n");
        exit(6)
    }
    if buf[0..4] != PEER {
        write("NETPROBE-WRONG-SOURCE\n");
        exit(7)
    }
    write("NETPROBE-ECHO-OK bytes=");
    write_u64(PROBE.len() as u64);
    write(" from=");
    write_ipv4([buf[0], buf[1], buf[2], buf[3]]);
    write("\n");
    udp_close(sock);

    // 4. DNS. A resolver failure is reported, not papered over with a guess.
    let mut addr = [0u8; 4];
    if net_resolve(NAME, &mut addr) != 4 {
        write("NETPROBE-RESOLVE-FAILED\n");
        exit(8)
    }
    write("NETPROBE-RESOLVE-OK name=");
    write(NAME);
    write(" address=");
    write_ipv4(addr);
    write("\n");

    write("NETPROBE-OK\n");
    exit(0)
}
