//! A host-side TCP echo endpoint for guest TCP tests (V0.9).
//!
//! QEMU's user-mode network maps the guest's `10.0.2.2` to the host's
//! loopback, so a guest connecting to `10.0.2.2:<port>` reaches this listener
//! through the host operating system's own TCP stack — an implementation that
//! shares no code with the guest's. The endpoint echoes every byte back and,
//! independently of anything the guest prints, checks the bytes against the
//! test pattern both sides agree on: `byte[i] = (i * 7 + 3) % 251`. Its
//! verdict lands in the serial log as `[HOST:TCP] …` lines, so a guest that
//! claimed success without delivering the right bytes fails the test.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// The byte the test pattern has at offset `i`.
pub fn pattern_byte(i: usize) -> u8 {
    ((i * 7 + 3) % 251) as u8
}

#[derive(Default)]
struct Tally {
    connections: u32,
    bytes_in: u64,
    bytes_out: u64,
    pattern_ok: bool,
    clean_close: bool,
}

pub struct TcpEcho {
    pub port: u16,
    tally: Arc<Mutex<Tally>>,
}

impl TcpEcho {
    /// Listen on a loopback port (0 = ephemeral) and serve connections on a
    /// background thread; `tx` carries `[HOST:TCP]` observations into the
    /// serial stream. A fixed port exists because Ring 3 test programs take no
    /// arguments and need a port they can know in advance.
    pub fn start_on(port: u16, tx: mpsc::Sender<String>) -> std::io::Result<TcpEcho> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let tally = Arc::new(Mutex::new(Tally::default()));
        let shared = Arc::clone(&tally);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &tx, &shared);
            }
        });
        Ok(TcpEcho { port, tally })
    }

    /// One line summarizing what the host saw, appended after the run.
    pub fn summary(&self) -> String {
        let t = self.tally.lock().unwrap();
        format!(
            "[HOST:TCP] summary connections={} bytes_in={} bytes_out={} pattern_ok={} clean_close={}",
            t.connections, t.bytes_in, t.bytes_out, t.pattern_ok, t.clean_close
        )
    }
}

fn serve(mut stream: TcpStream, tx: &mpsc::Sender<String>, tally: &Arc<Mutex<Tally>>) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    let _ = tx.send(format!("[HOST:TCP] accepted peer={peer}"));
    // A stuck guest must not wedge the harness thread forever.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let mut buf = [0u8; 4096];
    let mut offset = 0usize;
    let mut pattern_ok = true;
    let mut bytes_out = 0u64;
    let clean_close = loop {
        match stream.read(&mut buf) {
            Ok(0) => break true, // orderly FIN from the guest
            Ok(n) => {
                for (k, &b) in buf[..n].iter().enumerate() {
                    if b != pattern_byte(offset + k) {
                        pattern_ok = false;
                    }
                }
                offset += n;
                if stream.write_all(&buf[..n]).is_err() {
                    break false;
                }
                bytes_out += n as u64;
            }
            Err(_) => break false,
        }
    };
    // Close our half only after the guest closed its own, so the guest sees a
    // full passive-close sequence as well as an active one.
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let mut t = tally.lock().unwrap();
    t.connections += 1;
    t.bytes_in += offset as u64;
    t.bytes_out += bytes_out;
    // The summary speaks for EVERY connection: one bad stream among several
    // good ones must still fail it.
    let first = t.connections == 1;
    t.pattern_ok = (first || t.pattern_ok) && offset > 0 && pattern_ok;
    t.clean_close = (first || t.clean_close) && clean_close;
    let _ = tx.send(format!(
        "[HOST:TCP] closed peer={peer} bytes_in={offset} bytes_out={bytes_out} pattern_ok={} clean_close={clean_close}",
        offset > 0 && pattern_ok
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_and_checks_the_pattern_over_a_real_socket() {
        let (tx, rx) = mpsc::channel();
        let echo = TcpEcho::start_on(0, tx).unwrap();
        let mut c = TcpStream::connect(("127.0.0.1", echo.port)).unwrap();
        let data: Vec<u8> = (0..5000).map(pattern_byte).collect();
        c.write_all(&data).unwrap();
        c.shutdown(std::net::Shutdown::Write).unwrap();
        let mut back = Vec::new();
        c.read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
        let lines: Vec<String> = (0..2)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert!(lines[1].contains("bytes_in=5000 bytes_out=5000 pattern_ok=true clean_close=true"));
        assert!(echo.summary().contains("pattern_ok=true"));
    }
}
