//! An audit-anchor witness outside the guest's disk (V0.9).
//!
//! The guest's audit chain is unkeyed, so an attacker who can rewrite the
//! whole trail can write one that verifies. The defence is a copy of the head
//! held somewhere that attacker cannot reach; in the harness that is this
//! witness, reached by the guest as `10.0.2.2:<port>` over QEMU's user-mode
//! network. It persists anchors to a file so a later boot — a separate QEMU
//! process and a separate runner process — can be checked against an earlier
//! one.
//!
//! Protocol (one UDP datagram each way):
//!   `ITISYOU-ANCHOR v1 count=<n> head=<hex>` → store, reply `ANCHORED count=<n> head=<hex>`
//!   `ITISYOU-ANCHOR-QUERY v1`                → reply `LAST count=<n> head=<hex>` or `NONE`

use std::fs::OpenOptions;
use std::io::Write;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

pub struct Witness {
    pub port: u16,
}

/// The most recent anchor recorded in `store`, as `count=<n> head=<hex>`.
fn last_anchor(store: &Path) -> Option<String> {
    let text = std::fs::read_to_string(store).ok()?;
    text.lines()
        .rev()
        .find(|l| l.starts_with("count="))
        .map(str::to_string)
}

fn is_hex_head(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Handle one request; returns the reply to send, if any.
fn answer(request: &str, store: &Path, tx: &mpsc::Sender<String>) -> Option<String> {
    if let Some(rest) = request.strip_prefix("ITISYOU-ANCHOR v1 ") {
        let mut count = None;
        let mut head = None;
        for field in rest.split_whitespace() {
            if let Some(v) = field.strip_prefix("count=") {
                count = v.parse::<u64>().ok();
            } else if let Some(v) = field.strip_prefix("head=") {
                head = Some(v.to_string()).filter(|h| is_hex_head(h));
            }
        }
        let (Some(count), Some(head)) = (count, head) else {
            let _ = tx.send("[HOST:WITNESS] refused malformed_anchor".to_string());
            return None;
        };
        let entry = format!("count={count} head={head}");
        let stored = OpenOptions::new()
            .create(true)
            .append(true)
            .open(store)
            .and_then(|mut f| writeln!(f, "{entry}"));
        if stored.is_err() {
            let _ = tx.send("[HOST:WITNESS] store_failed".to_string());
            return None;
        }
        let _ = tx.send(format!("[HOST:WITNESS] anchored {entry}"));
        return Some(format!("ANCHORED {entry}"));
    }
    if request.trim() == "ITISYOU-ANCHOR-QUERY v1" {
        let reply = match last_anchor(store) {
            Some(entry) => format!("LAST {entry}"),
            None => "NONE".to_string(),
        };
        let _ = tx.send(format!("[HOST:WITNESS] query answered={reply}"));
        return Some(reply);
    }
    let _ = tx.send("[HOST:WITNESS] refused unknown_request".to_string());
    None
}

impl Witness {
    pub fn start(store: PathBuf, tx: mpsc::Sender<String>) -> std::io::Result<Witness> {
        let socket = UdpSocket::bind(("127.0.0.1", 0))?;
        let port = socket.local_addr()?.port();
        thread::spawn(move || {
            let mut buf = [0u8; 512];
            while let Ok((n, from)) = socket.recv_from(&mut buf) {
                let Ok(request) = std::str::from_utf8(&buf[..n]) else {
                    continue;
                };
                if let Some(reply) = answer(request, &store, &tx) {
                    let _ = socket.send_to(reply.as_bytes(), from);
                }
            }
        });
        Ok(Witness { port })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn anchors_persist_and_are_returned_across_restarts() {
        // Under the workspace's target dir, never the system temp dir (which
        // is on C: on the development host).
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("itisyou-witness-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let store = dir.join("anchors.txt");
        let _ = std::fs::remove_file(&store);
        let (tx, _rx) = mpsc::channel();
        let head = "ab".repeat(32);

        let w = Witness::start(store.clone(), tx.clone()).unwrap();
        let c = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = [0u8; 512];
        c.send_to(b"ITISYOU-ANCHOR-QUERY v1", ("127.0.0.1", w.port))
            .unwrap();
        let n = c.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"NONE");
        c.send_to(
            format!("ITISYOU-ANCHOR v1 count=7 head={head}").as_bytes(),
            ("127.0.0.1", w.port),
        )
        .unwrap();
        let n = c.recv(&mut buf).unwrap();
        assert_eq!(
            std::str::from_utf8(&buf[..n]).unwrap(),
            format!("ANCHORED count=7 head={head}")
        );

        // A second witness process on the same store (the next boot's runner).
        let w2 = Witness::start(store.clone(), tx).unwrap();
        c.send_to(b"ITISYOU-ANCHOR-QUERY v1", ("127.0.0.1", w2.port))
            .unwrap();
        let n = c.recv(&mut buf).unwrap();
        assert_eq!(
            std::str::from_utf8(&buf[..n]).unwrap(),
            format!("LAST count=7 head={head}")
        );

        // Malformed anchors are refused and store nothing.
        c.send_to(b"ITISYOU-ANCHOR v1 count=1 head=zz", ("127.0.0.1", w2.port))
            .unwrap();
        assert!(c.recv(&mut buf).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
