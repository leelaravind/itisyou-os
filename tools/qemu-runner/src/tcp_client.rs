//! A host-side TCP CLIENT for the guest's passive open (V0.9).
//!
//! QEMU's user-mode network forwards a host loopback port to a guest port
//! (`hostfwd`), so a connection made here is carried to the guest by slirp
//! and accepted by the guest's own listener — the reverse direction of
//! `tcp_echo`. The client waits until the guest says it is listening (a
//! serial marker the runner relays), sends the shared test pattern, half-
//! closes, and reads until the guest closes too; the echo is checked here,
//! byte for byte, independently of anything the guest prints.

use crate::tcp_echo::pattern_byte;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Bytes the client sends: more than one maximum-size segment.
const LEN: usize = 2500;
const ATTEMPTS: u32 = 5;

/// An unused loopback port for the forward (bound and released at once).
pub fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

#[derive(Default)]
struct Tally {
    attempts: u32,
    echo_ok: bool,
    bytes: usize,
}

pub struct TcpClient {
    tally: Arc<Mutex<Tally>>,
}

impl TcpClient {
    /// Connect to `127.0.0.1:host_port` once `go` fires, retrying a few
    /// times (slirp accepts on the host side before the guest has answered,
    /// so an early attempt can be refused by the guest after the fact).
    pub fn start(host_port: u16, go: mpsc::Receiver<()>, tx: mpsc::Sender<String>) -> TcpClient {
        let tally = Arc::new(Mutex::new(Tally::default()));
        let shared = Arc::clone(&tally);
        thread::spawn(move || {
            if go.recv_timeout(Duration::from_secs(600)).is_err() {
                return;
            }
            for attempt in 1..=ATTEMPTS {
                shared.lock().unwrap().attempts = attempt;
                match exchange(host_port) {
                    Ok(n) => {
                        let mut t = shared.lock().unwrap();
                        t.echo_ok = true;
                        t.bytes = n;
                        let _ = tx.send(format!(
                            "[HOST:TCPC] echo_ok attempt={attempt} bytes={n} pattern_ok=true"
                        ));
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(format!("[HOST:TCPC] attempt={attempt} failed={e}"));
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        });
        TcpClient { tally }
    }

    /// One line summarizing what the host saw, appended after the run.
    pub fn summary(&self) -> String {
        let t = self.tally.lock().unwrap();
        format!(
            "[HOST:TCPC] summary attempts={} echo_ok={} bytes={}",
            t.attempts, t.echo_ok, t.bytes
        )
    }
}

fn exchange(port: u16) -> Result<usize, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    let data: Vec<u8> = (0..LEN).map(pattern_byte).collect();
    stream.write_all(&data).map_err(|e| e.to_string())?;
    // Half-close: the guest sees our FIN once it has read everything, echoes
    // what is left, and closes its own side — a full passive close.
    stream
        .shutdown(Shutdown::Write)
        .map_err(|e| e.to_string())?;
    let mut back = Vec::new();
    stream.read_to_end(&mut back).map_err(|e| e.to_string())?;
    if back == data {
        Ok(back.len())
    } else {
        Err(format!("echo_mismatch got={} want={LEN}", back.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchanges_with_a_real_echo_server_after_the_trigger() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            s.write_all(&buf).unwrap();
        });
        let (go_tx, go_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let client = TcpClient::start(port, go_rx, tx);
        go_tx.send(()).unwrap();
        let line = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            line.contains("echo_ok attempt=1 bytes=2500 pattern_ok=true"),
            "{line}"
        );
        assert!(client.summary().contains("echo_ok=true bytes=2500"));
    }
}
