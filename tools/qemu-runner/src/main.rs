//! Deterministic QEMU boot/test harness.
//!
//! Launches a disk image, captures serial output, asserts required boot-stage
//! markers, classifies the outcome, and writes machine-readable evidence.
//! Absence of output is NEVER success (implementation plan §12.2).
//!
//! Usage:
//!   qemu-runner --image <path> [--uefi --firmware <code.fd>]
//!               [--expect B010]... [--expect-selftest]
//!               [--timeout-secs 60] [--label boot-smoke]
//!               [--connect-timeout-secs 30] [--boot-timeout-secs 75]
//!               [--stall-timeout-secs N]
//!               [--artifacts artifacts/qemu] [--qemu <path>]
//!
//! Timeout model (deterministic, phase-attributed):
//!   spawn ─connect-timeout→ serial connected ─boot-timeout→ first guest byte
//!   ─stall-timeout (per line, optional)→ … ─timeout (absolute)→ end.
//! Each phase has its own outcome (`LaunchFailure`, `SerialConnectFailure`,
//! `NoSerialOutput`, `Stalled`, `Timeout`) so a failure names the phase that
//! broke instead of collapsing into a generic timeout with an empty log.

use kernel_core::marker::{parse_line, Marker};
use kernel_core::stage::Stage;
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod wire;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Outcome {
    Success,
    SelftestFailed,
    Panic,
    Timeout,
    MissingMarkers,
    LaunchFailure,
    UnexpectedExit,
    /// QEMU stayed alive but never opened the serial connection within
    /// `--connect-timeout-secs` (a transport/launch problem, never a guest
    /// problem — the guest cannot have run yet).
    SerialConnectFailure,
    /// The serial connection was established but the guest emitted no bytes
    /// at all before `--boot-timeout-secs` elapsed: firmware/bootloader never
    /// reached kernel entry. Distinct from `Timeout`, which means the guest
    /// did run and then failed to finish.
    NoSerialOutput,
    /// The guest produced output, then went silent for longer than
    /// `--stall-timeout-secs` with required evidence still missing.
    Stalled,
}

#[derive(Debug, Serialize)]
struct TestEvent {
    name: String,
    pass: bool,
}

#[derive(Debug, Serialize)]
struct RunResult {
    outcome: Outcome,
    label: String,
    image: String,
    firmware_mode: String,
    qemu_exit_code: Option<i32>,
    duration_ms: u128,
    stages_seen: Vec<String>,
    missing_stages: Vec<String>,
    missing_required: Vec<String>,
    tests: Vec<TestEvent>,
    selftest_pass: Option<u32>,
    selftest_fail: Option<u32>,
    panic_message: Option<String>,
    serial_log: String,
    command_line: Vec<String>,
    /// Milliseconds from spawn until QEMU's serial connection was accepted.
    connect_ms: Option<u128>,
    /// Milliseconds from spawn until the guest's FIRST serial byte. The single
    /// most useful number for spotting boot-path slowdowns before they become
    /// timeouts (V0.8: an unstripped 26.8 MiB kernel ELF pushed this past the
    /// leg timeouts and produced empty-serial failures).
    first_output_ms: Option<u128>,
    /// Human-readable explanation for non-success outcomes.
    detail: Option<String>,
}

struct Options {
    image: PathBuf,
    uefi: bool,
    firmware: Option<PathBuf>,
    expect: Vec<Stage>,
    expect_selftest: bool,
    /// Kill QEMU and report success as soon as every expected stage marker
    /// has been observed — for interactive images that (correctly) never
    /// exit on their own.
    exit_after_markers: bool,
    /// The run is EXPECTED to panic (negative-path test): success iff the
    /// panic marker appears.
    expect_panic: bool,
    /// Lines written to the guest serial (stdin) once B130 (shell running)
    /// is observed.
    send: Vec<String>,
    /// Substrings that must appear somewhere in the serial log.
    require: Vec<String>,
    /// Substrings that must NOT appear anywhere in the serial log.
    ///
    /// Adversarial tests need this: "the guest refused" is only provable by
    /// the absence of the marker it would have printed had it not refused.
    /// Expressing that as a positive assertion is impossible — there is no
    /// line to match — so the harness has to be able to fail on presence.
    forbid: Vec<String>,
    /// Attach a QEMU HMP monitor over TCP (the runner listens; QEMU connects)
    /// so keyboard/mouse input can be injected into the guest's PS/2 devices.
    monitor: bool,
    /// Once this serial substring appears, send the `--monitor-cmd` sequence
    /// through the monitor (drives real input for the graphics/input test).
    inject_after: Option<String>,
    /// HMP commands sent in order after the injection gate is reached
    /// (e.g. `sendkey h`, `mouse_move 40 25`, `screendump <path>`).
    monitor_cmds: Vec<String>,
    /// Milliseconds to wait between injected monitor commands so each PS/2 IRQ
    /// is processed before the next event arrives.
    inject_delay_ms: u64,
    /// Attach an emulated AC97 audio controller whose output is captured to a
    /// WAV file via QEMU's `wav` audio backend, so a test can prove the OS's
    /// generated PCM samples traversed the full driver → controller → output
    /// path (not just that init succeeded).
    audio: bool,
    /// WAV output path for `--audio` (default `<artifacts>/<label>.wav`).
    audio_out: Option<PathBuf>,
    /// Attach a UHCI USB controller with a USB HID keyboard + mouse, so the OS
    /// can enumerate real USB devices and read HID input.
    usb: bool,
    /// Attach an xHCI controller with a USB HID keyboard instead of UHCI, so
    /// the newer host-controller path can be exercised on its own. Kept
    /// separate from `--usb` because a test that could not say which
    /// controller delivered a keystroke would prove nothing about either.
    xhci: bool,
    /// Attach an e1000 NIC wired to the runner's own host-side Ethernet peer
    /// (`wire.rs`) over a `dgram` netdev, so the guest's stack is exercised
    /// against a real, independent implementation of ARP/ICMP/UDP/DNS with no
    /// external network and nothing non-deterministic in the loop.
    net: bool,
    /// Attach an e1000 NIC on QEMU's user-mode network (slirp): an
    /// independent IPv4 implementation with its own gateway (10.0.2.2), DHCP
    /// server and DNS forwarder. Mutually exclusive with `--net`.
    net_user: bool,
    /// Attach an emulated NVMe controller backed by a generated raw disk
    /// whose first sector carries a known magic (for storage read tests).
    nvme: bool,
    /// Attach an emulated NVMe controller backed by a PERSISTENT raw disk at
    /// this path: created blank only if missing, never regenerated — so
    /// writes survive across separate QEMU runs (reboot-persistence tests).
    nvme_persist: Option<PathBuf>,
    /// Hard ceiling on the whole run, measured from spawn.
    timeout: Duration,
    /// How long QEMU may take to open the serial connection after spawn.
    /// Exceeding it is a transport failure (`SerialConnectFailure`), never a
    /// guest verdict, because the guest cannot have produced output yet.
    connect_timeout: Duration,
    /// How long the guest may take, measured from the moment the serial
    /// connection is accepted, to emit its FIRST byte. Exceeding it is
    /// `NoSerialOutput` — reported immediately instead of burning the full
    /// `timeout`, so a broken boot path is diagnosable rather than silent.
    boot_timeout: Duration,
    /// Maximum silence between serial lines once output has started. `None`
    /// disables the stall watchdog (long CPU-bound guest phases are legal).
    stall_timeout: Option<Duration>,
    label: String,
    artifacts: PathBuf,
    qemu: String,
}

/// Default ceiling for QEMU opening the serial socket after spawn. QEMU
/// connects during device realize (sub-second in practice); 30 s is generous
/// enough for a loaded CI host while still failing fast.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default ceiling from serial-connect to the guest's first byte. A healthy
/// BIOS boot of a stripped kernel reaches `B010` in <10 s; UEFI in <5 s.
const DEFAULT_BOOT_TIMEOUT: Duration = Duration::from_secs(75);

/// 16-byte magic written at LBA 0 of the generated NVMe disk; the kernel
/// asserts these exact bytes after reading block 0 back over NVMe.
const NVME_DISK_MAGIC: &[u8; 16] = b"ITISYOU-OS-DISK1";

fn parse_args() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let mut image = None;
    let mut uefi = false;
    let mut firmware = None;
    let mut expect = Vec::new();
    let mut expect_selftest = false;
    let mut exit_after_markers = false;
    let mut expect_panic = false;
    let mut send = Vec::new();
    let mut require = Vec::new();
    let mut forbid: Vec<String> = Vec::new();
    let mut monitor = false;
    let mut inject_after = None;
    let mut monitor_cmds = Vec::new();
    let mut inject_delay_ms = 250u64;
    let mut audio = false;
    let mut audio_out = None;
    let mut usb = false;
    let mut xhci = false;
    let mut net = false;
    let mut net_user = false;
    let mut nvme = false;
    let mut nvme_persist = None;
    let mut timeout = Duration::from_secs(60);
    let mut connect_timeout = DEFAULT_CONNECT_TIMEOUT;
    let mut boot_timeout = DEFAULT_BOOT_TIMEOUT;
    let mut stall_timeout: Option<Duration> = None;
    let mut label = "run".to_string();
    let mut artifacts = PathBuf::from("artifacts/qemu");
    let mut qemu =
        std::env::var("QEMU_SYSTEM_X86_64").unwrap_or_else(|_| "qemu-system-x86_64".into());

    while let Some(arg) = args.next() {
        let mut value = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--image" => image = Some(PathBuf::from(value("--image")?)),
            "--uefi" => uefi = true,
            "--firmware" => firmware = Some(PathBuf::from(value("--firmware")?)),
            "--expect" => {
                let code = value("--expect")?;
                let stage =
                    Stage::from_code(&code).ok_or_else(|| format!("unknown stage code {code}"))?;
                expect.push(stage);
            }
            "--expect-selftest" => expect_selftest = true,
            "--exit-after-markers" => exit_after_markers = true,
            "--expect-panic" => expect_panic = true,
            "--send" => send.push(value("--send")?),
            "--require" => require.push(value("--require")?),
            "--forbid" => forbid.push(value("--forbid")?),
            "--monitor" => monitor = true,
            "--inject-after" => {
                inject_after = Some(value("--inject-after")?);
                monitor = true;
            }
            "--monitor-cmd" => {
                monitor_cmds.push(value("--monitor-cmd")?);
                monitor = true;
            }
            "--inject-delay-ms" => {
                inject_delay_ms = value("--inject-delay-ms")?
                    .parse()
                    .map_err(|e| format!("bad inject-delay-ms: {e}"))?;
            }
            "--audio" => audio = true,
            "--audio-out" => {
                audio_out = Some(PathBuf::from(value("--audio-out")?));
                audio = true;
            }
            "--usb" => usb = true,
            "--xhci" => xhci = true,
            "--net" => net = true,
            "--net-user" => net_user = true,
            "--nvme" => nvme = true,
            "--nvme-persist" => nvme_persist = Some(PathBuf::from(value("--nvme-persist")?)),
            "--timeout-secs" => {
                timeout = Duration::from_secs(
                    value("--timeout-secs")?
                        .parse()
                        .map_err(|e| format!("bad timeout: {e}"))?,
                )
            }
            "--connect-timeout-secs" => {
                connect_timeout = Duration::from_secs(
                    value("--connect-timeout-secs")?
                        .parse()
                        .map_err(|e| format!("bad connect-timeout: {e}"))?,
                )
            }
            "--boot-timeout-secs" => {
                boot_timeout = Duration::from_secs(
                    value("--boot-timeout-secs")?
                        .parse()
                        .map_err(|e| format!("bad boot-timeout: {e}"))?,
                )
            }
            "--stall-timeout-secs" => {
                stall_timeout = Some(Duration::from_secs(
                    value("--stall-timeout-secs")?
                        .parse()
                        .map_err(|e| format!("bad stall-timeout: {e}"))?,
                ))
            }
            "--label" => label = value("--label")?,
            "--artifacts" => artifacts = PathBuf::from(value("--artifacts")?),
            "--qemu" => qemu = value("--qemu")?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if net && net_user {
        // Both would claim the guest's single NIC slot and MAC.
        return Err("--net and --net-user are mutually exclusive".to_string());
    }
    Ok(Options {
        image: image.ok_or("--image is required")?,
        uefi,
        firmware,
        expect,
        expect_selftest,
        exit_after_markers,
        expect_panic,
        send,
        require,
        forbid,
        monitor,
        inject_after,
        monitor_cmds,
        inject_delay_ms,
        audio,
        audio_out,
        usb,
        xhci,
        net,
        net_user,
        nvme,
        nvme_persist,
        timeout,
        // Sub-deadlines are only ever narrowing: a leg that sets a short
        // --timeout-secs must not be given a longer boot/connect budget than
        // the whole run, or the semantics stop being deterministic.
        connect_timeout: connect_timeout.min(timeout),
        boot_timeout: boot_timeout.min(timeout),
        stall_timeout,
        label,
        artifacts,
        qemu,
    })
}

/// Create (or overwrite) the NVMe backing disk: 16 MiB raw (room for the
/// V0.7 package store's multi-version lifecycle), LBA 0 begins with
/// NVME_DISK_MAGIC then a deterministic pattern. Returns its path.
fn make_nvme_disk(artifacts: &std::path::Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(artifacts).ok();
    let path = artifacts.join("nvme-disk.img");
    let mut buf = vec![0u8; 16 * 1024 * 1024];
    buf[..16].copy_from_slice(NVME_DISK_MAGIC);
    // Fill the rest of sector 0 with a known pattern (byte i => i xor 0x5A).
    for (i, b) in buf[16..512].iter_mut().enumerate() {
        *b = ((i + 16) as u8) ^ 0x5A;
    }
    std::fs::write(&path, &buf)?;
    Ok(path)
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("qemu-runner: {e}");
            std::process::exit(2);
        }
    };
    let result = run(&opts);
    let ok = result.outcome == Outcome::Success;

    std::fs::create_dir_all(&opts.artifacts).ok();
    let json_path = opts.artifacts.join(format!("{}.result.json", opts.label));
    match serde_json::to_string_pretty(&result) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&json_path, json) {
                eprintln!("qemu-runner: writing {}: {e}", json_path.display());
            }
        }
        Err(e) => eprintln!("qemu-runner: serializing result: {e}"),
    }

    println!(
        "qemu-runner: label={} outcome={:?} exit={:?} stages={} duration={}ms connect={:?}ms first-output={:?}ms evidence={}",
        result.label,
        result.outcome,
        result.qemu_exit_code,
        result.stages_seen.len(),
        result.duration_ms,
        result.connect_ms,
        result.first_output_ms,
        json_path.display(),
    );
    if !ok {
        if let Some(detail) = &result.detail {
            eprintln!("qemu-runner: {detail}");
        }
        // Surface the serial tail so CI logs are diagnosable without artifacts.
        eprintln!("--- last serial lines ---");
        let log = std::fs::read_to_string(&result.serial_log).unwrap_or_default();
        for line in log
            .lines()
            .rev()
            .take(25)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            eprintln!("{line}");
        }
        std::process::exit(1);
    }
}

/// Serial transport: TCP, with the runner as listener and QEMU connecting as
/// a client. `-serial stdio` is NOT used because QEMU's Windows stdio chardev
/// stops feeding redirected stdin after the guest UART's 16-byte RX FIFO
/// fills once, which stalls interactive shell tests (observed 2026-09-02).
fn build_command(
    opts: &Options,
    serial_port: u16,
    monitor_port: Option<u16>,
    net_ports: Option<(u16, u16)>,
) -> Command {
    let mut cmd = Command::new(&opts.qemu);
    // `snapshot=on`: guest writes to the boot disk go to a temporary overlay.
    // OVMF, lacking a persistent variable store, writes an `NvVars` file into
    // the image's EFI partition on every boot — so without this a test run
    // silently changed the image it was testing (observed 2026-09-13: a
    // booted release image no longer matched its published SHA-256).
    // Persistent state belongs on the separate NVMe disks, never here.
    cmd.arg("-drive")
        .arg(format!("format=raw,file={},snapshot=on", opts.image.display()))
        .arg("-serial")
        .arg(format!("tcp:127.0.0.1:{serial_port},nodelay"))
        // The default `qemu64` model advertises neither SMEP nor SMAP, so the
        // guest's hardening would silently do nothing. Requesting them means
        // every leg in the matrix runs WITH supervisor-mode protection on,
        // which is a far stronger statement than one leg that enables it.
        .args(["-cpu", "qemu64,+smep,+smap,+umip"])
        .args(["-display", "none", "-no-reboot", "-m", "256M"])
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
    if let Some(mport) = monitor_port {
        // QEMU connects to the runner's monitor listener as a client (same
        // transport as the serial line), giving an HMP channel for injecting
        // PS/2 keyboard/mouse input and capturing framebuffer screendumps.
        cmd.arg("-monitor")
            .arg(format!("tcp:127.0.0.1:{mport},nodelay"));
    }
    if opts.audio {
        // AC97 output → WAV file, so the OS's generated samples are captured
        // for inspection. `wav` backend needs no host audio hardware.
        let wav = opts
            .audio_out
            .clone()
            .unwrap_or_else(|| opts.artifacts.join(format!("{}.wav", opts.label)));
        if let Some(parent) = wav.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::remove_file(&wav).ok();
        cmd.arg("-audiodev")
            .arg(format!("wav,id=snd0,path={}", wav.display()))
            .args(["-device", "AC97,audiodev=snd0"]);
    }
    if opts.usb {
        // A UHCI controller with a USB HID keyboard + mouse on its root ports.
        cmd.args([
            "-device",
            "piix3-usb-uhci,id=uhci",
            "-device",
            "usb-kbd,bus=uhci.0,port=1",
            "-device",
            "usb-mouse,bus=uhci.0,port=2",
        ]);
    }
    if let Some((host_port, guest_port)) = net_ports {
        // `dgram` carries one raw Ethernet frame per UDP datagram: QEMU binds
        // `local` and sends to `remote`, so the runner's peer is literally the
        // only thing on the wire. No slirp, no host networking, no DHCP — the
        // guest sees exactly the packets the peer chooses to send.
        cmd.arg("-netdev")
            .arg(format!(
                "dgram,id=net0,local.type=inet,local.host=127.0.0.1,local.port={guest_port},remote.type=inet,remote.host=127.0.0.1,remote.port={host_port}"
            ))
            .args([
                "-device",
                "e1000,netdev=net0,mac=52:54:00:12:34:56",
            ]);
    }
    if opts.net_user {
        cmd.args([
            "-netdev",
            "user,id=net0",
            "-device",
            "e1000,netdev=net0,mac=52:54:00:12:34:56",
        ]);
    }
    if opts.xhci {
        cmd.args([
            "-device",
            "qemu-xhci,id=xhci",
            "-device",
            "usb-kbd,bus=xhci.0,port=1",
        ]);
    }
    if opts.nvme {
        match make_nvme_disk(&opts.artifacts) {
            Ok(disk) => {
                cmd.arg("-drive")
                    .arg(format!(
                        "file={},if=none,format=raw,id=nvme0",
                        disk.display()
                    ))
                    .args(["-device", "nvme,serial=itisyou1,drive=nvme0"]);
            }
            Err(e) => eprintln!("qemu-runner: could not create NVMe disk: {e}"),
        }
    }
    if let Some(disk) = &opts.nvme_persist {
        // Create a blank persistent disk only if it doesn't already exist,
        // so writes from a previous run survive into this one.
        if !disk.exists() {
            if let Some(parent) = disk.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if let Err(e) = std::fs::write(disk, vec![0u8; 16 * 1024 * 1024]) {
                eprintln!("qemu-runner: could not create persistent NVMe disk: {e}");
            }
        }
        cmd.arg("-drive")
            .arg(format!(
                "file={},if=none,format=raw,id=nvme0",
                disk.display()
            ))
            .args(["-device", "nvme,serial=itisyoup,drive=nvme0"]);
    }
    if opts.uefi {
        let fw = opts
            .firmware
            .clone()
            .or_else(|| std::env::var("OVMF_CODE").ok().map(PathBuf::from))
            .expect("--uefi requires --firmware or OVMF_CODE env var");
        cmd.arg("-drive").arg(format!(
            "if=pflash,format=raw,readonly=on,file={}",
            fw.display()
        ));
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    cmd
}

/// Guarantees the QEMU process is killed and reaped on every exit path —
/// early return, error, or panic. A leaked QEMU keeps the disk image open on
/// Windows and poisons the next leg of the matrix, which is exactly the class
/// of cross-test contamination that makes a harness look "flaky".
struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn get(&mut self) -> &mut Child {
        self.0.as_mut().expect("child already taken from guard")
    }
    /// Hand ownership back once the run reaches its own orderly teardown.
    fn release(mut self) -> Child {
        self.0.take().expect("child already taken from guard")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Terminal result for a failure detected before the serial-reading loop can
/// produce any evidence. `detail` is written into the serial log so the
/// artifact is self-describing rather than an empty file.
#[allow(clippy::too_many_arguments)]
fn early_failure(
    opts: &Options,
    outcome: Outcome,
    detail: String,
    started: Instant,
    serial_log_path: &std::path::Path,
    command_line: Vec<String>,
    connect_ms: Option<u128>,
    qemu_exit_code: Option<i32>,
) -> RunResult {
    let _ = std::fs::write(
        serial_log_path,
        format!(
            "{outcome:?}: {detail}
"
        ),
    );
    RunResult {
        outcome,
        label: opts.label.clone(),
        image: opts.image.display().to_string(),
        firmware_mode: if opts.uefi {
            "uefi".into()
        } else {
            "bios".into()
        },
        qemu_exit_code,
        duration_ms: started.elapsed().as_millis(),
        stages_seen: vec![],
        missing_stages: opts.expect.iter().map(|s| s.code().to_string()).collect(),
        missing_required: opts.require.clone(),
        tests: vec![],
        selftest_pass: None,
        selftest_fail: None,
        panic_message: None,
        serial_log: serial_log_path.display().to_string(),
        command_line,
        connect_ms,
        first_output_ms: None,
        detail: Some(detail),
    }
}

fn run(opts: &Options) -> RunResult {
    std::fs::create_dir_all(&opts.artifacts).ok();
    let serial_log_path = opts.artifacts.join(format!("{}.serial.log", opts.label));

    // Reserve the serial TCP port before launching QEMU (listener owns it,
    // so there is no bind race).
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding serial TCP listener");
    let serial_port = listener.local_addr().unwrap().port();
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");

    // Optional monitor listener (same no-race ownership as the serial port).
    let monitor_listener = if opts.monitor {
        let l = TcpListener::bind("127.0.0.1:0").expect("binding monitor TCP listener");
        l.set_nonblocking(true)
            .expect("nonblocking monitor listener");
        Some(l)
    } else {
        None
    };
    let monitor_port = monitor_listener
        .as_ref()
        .map(|l| l.local_addr().unwrap().port());

    // The serial line channel is created before launch so the host-side
    // network peer can publish its observations into the SAME stream the guest
    // writes to: its `[HOST:NET]` lines land in the serial log next to the
    // guest's, and `--require` sees both without a second artifact to
    // correlate. Host lines are prefixed so nobody can mistake them for
    // something the guest claimed about itself.
    let (tx, rx) = mpsc::channel::<String>();
    // The peer gets a SEPARATE channel, drained into the same line stream by
    // the main loop. Sharing `tx` would keep the serial channel open after the
    // guest exited, so the loop would wait for the peer instead of noticing
    // that QEMU was gone — a harness that hangs on a healthy run.
    let (net_tx, net_rx) = mpsc::channel::<String>();
    let wire_peer = if opts.net {
        match wire::WirePeer::start(net_tx.clone()) {
            Ok(peer) => {
                let _ = net_tx.send(format!(
                    "[HOST:NET] peer_ready host_port={} guest_port={} peer_ip=10.0.2.2 dns_ip=10.0.2.3",
                    peer.host_port, peer.guest_port
                ));
                Some(peer)
            }
            Err(e) => {
                eprintln!("qemu-runner: could not start the network peer: {e}");
                None
            }
        }
    } else {
        None
    };
    let net_ports = wire_peer.as_ref().map(|p| (p.host_port, p.guest_port));

    let mut cmd = build_command(opts, serial_port, monitor_port, net_ports);
    let command_line: Vec<String> = std::iter::once(opts.qemu.clone())
        .chain(cmd.get_args().map(|a| a.to_string_lossy().into_owned()))
        .collect();

    let started = Instant::now();
    let child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return early_failure(
                opts,
                Outcome::LaunchFailure,
                format!("spawning {}: {e}", opts.qemu),
                started,
                &serial_log_path,
                command_line,
                None,
                None,
            );
        }
    };
    let mut guard = ChildGuard(Some(child));

    // Deterministic, non-overlapping deadlines. Each phase owns its own
    // budget so a failure names the phase that actually failed:
    //   spawn ──connect_timeout──▶ serial connected ──boot_timeout──▶ first
    //   byte ──stall_timeout (per line)──▶ … ──timeout (absolute)──▶ end.
    let overall_deadline = started + opts.timeout;
    let connect_deadline = (started + opts.connect_timeout).min(overall_deadline);

    // Accept QEMU's serial connection (it connects during device realize).
    let serial_stream = match accept_with_deadline(&listener, guard.get(), connect_deadline) {
        Some(stream) => {
            stream.set_nodelay(true).ok();
            stream
        }
        None => {
            // Distinguish a dead QEMU (launch rejected the command line) from
            // a live QEMU that never dialled back (transport/port problem).
            let exit_code = guard.get().try_wait().ok().flatten().and_then(|s| s.code());
            let stderr_text = drain_stderr(guard.get());
            let (outcome, why) = match exit_code {
                Some(code) => (
                    Outcome::LaunchFailure,
                    format!("QEMU exited with code {code} before opening the serial connection"),
                ),
                None => (
                    Outcome::SerialConnectFailure,
                    format!(
                        "QEMU stayed alive but never connected to 127.0.0.1:{serial_port}                          within {}s",
                        opts.connect_timeout.as_secs()
                    ),
                ),
            };
            let detail = if stderr_text.trim().is_empty() {
                why
            } else {
                format!(
                    "{why}
--- qemu stderr ---
{stderr_text}"
                )
            };
            return early_failure(
                opts,
                outcome,
                detail,
                started,
                &serial_log_path,
                command_line,
                None,
                exit_code,
            );
        }
    };
    let connected_at = Instant::now();
    let connect_ms = connected_at.duration_since(started).as_millis();
    // The serial line is established: nothing else may claim this port for the
    // rest of the run, so stop listening. (Binding :0 already prevents a bind
    // race; closing prevents a late stray connection being mistaken for QEMU.)
    drop(listener);

    // First guest byte must arrive within boot_timeout of the connection —
    // measured from connect, not from spawn, so a slow host spawn cannot eat
    // the guest's boot budget.
    let boot_deadline = (connected_at + opts.boot_timeout).min(overall_deadline);

    let mut serial_writer = serial_stream.try_clone().expect("cloning serial stream");

    // Accept QEMU's monitor connection and spawn the injection thread. It
    // blocks on a one-shot gate the main loop trips when `inject_after`
    // appears in the serial stream, then plays the `monitor_cmds` sequence.
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let mut inject_handle = None;
    if let Some(ml) = &monitor_listener {
        if let Some(monitor_stream) = accept_with_deadline(ml, guard.get(), connect_deadline) {
            monitor_stream.set_nodelay(true).ok();
            let cmds = opts.monitor_cmds.clone();
            let delay = Duration::from_millis(opts.inject_delay_ms);
            inject_handle = Some(std::thread::spawn(move || {
                run_injection(monitor_stream, gate_rx, cmds, delay);
            }));
        } else {
            eprintln!("qemu-runner: QEMU never connected to the monitor TCP port");
        }
    }

    // Stream serial lines through the channel created above so the main loop
    // owns the timeout.
    let reader_handle = std::thread::spawn(move || {
        let reader = BufReader::new(serial_stream);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let stderr = guard.get().stderr.take().expect("stderr piped");
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    let mut serial_lines: Vec<String> = Vec::new();
    let mut stages_seen: BTreeSet<Stage> = BTreeSet::new();
    let mut tests: Vec<TestEvent> = Vec::new();
    let mut selftest: Option<(u32, u32)> = None;
    let mut panic_message: Option<String> = None;
    let mut timed_out = false;
    let mut no_serial_output = false;
    let mut stalled = false;
    let mut completed_by_markers = false;
    let mut sent_commands = false;
    let mut tripped_gate = false;
    let mut first_output_at: Option<Instant> = None;
    let mut last_line_at = connected_at;
    loop {
        // Fold in whatever the host-side peer observed since the last pass, so
        // its lines interleave with the guest's in the order they happened.
        while let Ok(host_line) = net_rx.try_recv() {
            serial_lines.push(host_line);
        }
        let now = Instant::now();
        if now >= overall_deadline {
            timed_out = true;
            let _ = guard.get().kill();
            break;
        }
        // The next wake-up is whichever phase deadline bites first. Only one
        // of the two sub-deadlines is ever armed: the boot deadline until the
        // guest speaks, the stall deadline afterwards.
        let phase_deadline = if first_output_at.is_none() {
            boot_deadline
        } else {
            opts.stall_timeout
                .map(|st| last_line_at + st)
                .unwrap_or(overall_deadline)
        };
        let wake = phase_deadline.min(overall_deadline);
        match rx.recv_timeout(wake.saturating_duration_since(now)) {
            Ok(line) => {
                if first_output_at.is_none() {
                    first_output_at = Some(Instant::now());
                }
                last_line_at = Instant::now();
                if let Some(marker) = parse_line(&line) {
                    match marker {
                        Marker::Stage(stage, _) => {
                            stages_seen.insert(stage);
                            // Drive the shell once it is running. The OS pipe
                            // buffers the burst; QEMU feeds the guest UART
                            // with flow control as the shell drains it.
                            if stage == Stage::B130ShellRunning
                                && !sent_commands
                                && !opts.send.is_empty()
                            {
                                sent_commands = true;
                                for cmd in &opts.send {
                                    let _ = writeln!(serial_writer, "{cmd}");
                                }
                                let _ = serial_writer.flush();
                            }
                        }
                        Marker::Panic(msg) => {
                            panic_message = Some(msg.to_string());
                        }
                        Marker::Selftest { pass, fail } => selftest = Some((pass, fail)),
                        Marker::Test { name, pass } => tests.push(TestEvent {
                            name: name.to_string(),
                            pass,
                        }),
                        Marker::Info(_) | Marker::Mode(_) => {}
                    }
                }
                // Trip the injection gate the first time its substring shows.
                if !tripped_gate {
                    if let Some(needle) = &opts.inject_after {
                        if line.contains(needle.as_str()) {
                            tripped_gate = true;
                            let _ = gate_tx.send(());
                        }
                    }
                }
                serial_lines.push(line);
                if opts.expect_panic && panic_message.is_some() {
                    // Evidence captured; the panicked kernel halts forever,
                    // so end the run here.
                    let _ = guard.get().kill();
                    break;
                }
                if opts.exit_after_markers
                    && !opts.expect.is_empty()
                    && panic_message.is_none()
                    && opts.expect.iter().all(|s| stages_seen.contains(s))
                {
                    completed_by_markers = true;
                    let _ = guard.get().kill();
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Attribute the timeout to the phase that actually expired.
                if first_output_at.is_none() {
                    no_serial_output = true;
                } else if opts.stall_timeout.is_some() && Instant::now() < overall_deadline {
                    stalled = true;
                } else {
                    timed_out = true;
                }
                let _ = guard.get().kill();
                break;
            }
            // Reader thread finished: QEMU closed stdout (exited).
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // The serial stream closing does not imply QEMU already exited: grant a
    // short grace period for a natural exit (isa-debug-exit still carries
    // the real status), then kill — child.wait must never be unbounded.
    let grace_deadline = Instant::now() + Duration::from_secs(3);
    while guard.get().try_wait().ok().flatten().is_none() {
        if Instant::now() >= grace_deadline {
            let _ = guard.get().kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    while let Ok(host_line) = net_rx.try_recv() {
        serial_lines.push(host_line);
    }
    let mut child = guard.release();
    // Reap QEMU before joining the reader: that closes QEMU's TCP half even on
    // Windows, so `BufRead::lines` cannot retain a runner process after a
    // timed-out guest. Closing our own write half is also required: otherwise
    // a cloned socket can keep the peer connection alive across the join.
    let exit_status = child.wait().ok();
    drop(serial_writer);
    let _ = reader_handle.join();
    // Drop the gate sender so a never-tripped injection thread unblocks, then
    // join it (best-effort — it exits once its sequence or the channel ends).
    drop(gate_tx);
    if let Some(h) = inject_handle {
        let _ = h.join();
    }
    let qemu_exit_code = exit_status.and_then(|s| s.code());
    let stderr_text = stderr_handle.join().unwrap_or_default();

    let mut log_text = serial_lines.join("\n");
    log_text.push('\n');
    if !stderr_text.is_empty() {
        log_text.push_str("--- qemu stderr ---\n");
        log_text.push_str(&stderr_text);
    }

    // The peer's own tally, appended after the run so a test can assert on
    // what the HOST observed rather than only on what the guest reported about
    // itself. A guest that printed "NETPROBE-OK" without transmitting a frame
    // fails here.
    if let Some(peer) = &wire_peer {
        let summary = peer.summary();
        serial_lines.push(summary.clone());
        log_text.push_str(&summary);
        log_text.push('\n');
    }

    let missing: Vec<String> = opts
        .expect
        .iter()
        .filter(|s| !stages_seen.contains(s))
        .map(|s| s.code().to_string())
        .collect();
    let mut missing_required: Vec<String> = opts
        .require
        .iter()
        .filter(|needle| !serial_lines.iter().any(|l| l.contains(needle.as_str())))
        .cloned()
        .collect();
    // A forbidden marker that DID appear is folded into the same list, so it
    // fails the run the same way a missing one does and shows up in the same
    // place in the evidence — prefixed so the two are never confused.
    for needle in &opts.forbid {
        if serial_lines.iter().any(|l| l.contains(needle.as_str())) {
            missing_required.push(format!("forbidden-present:{needle}"));
        }
    }

    // Audio evidence: the captured WAV must contain non-silent PCM — proof the
    // OS's generated samples actually reached the output backend, not just that
    // the driver initialized. QEMU may not finalize the WAV header on an
    // isa-debug-exit kill, so the data chunk is scanned leniently.
    if opts.audio {
        let wav = opts
            .audio_out
            .clone()
            .unwrap_or_else(|| opts.artifacts.join(format!("{}.wav", opts.label)));
        if !wav_has_nonsilent_pcm(&wav) {
            missing_required.push(format!("audio-nonsilent-wav:{}", wav.display()));
        }
    }

    // Boot-phase verdicts outrank every content check: with no output at all
    // there is nothing to interpret, and reporting "missing markers" for a
    // guest that never spoke hides the real fault (V0.8 boot-race lesson).
    let outcome = if no_serial_output {
        Outcome::NoSerialOutput
    } else if stalled {
        Outcome::Stalled
    } else if opts.expect_panic {
        // Negative-path run: the panic IS the expected evidence.
        match (&panic_message, timed_out) {
            (Some(_), _) if missing.is_empty() && missing_required.is_empty() => Outcome::Success,
            (Some(_), _) => Outcome::MissingMarkers,
            (None, true) => Outcome::Timeout,
            (None, false) => Outcome::MissingMarkers,
        }
    } else if !missing_required.is_empty() {
        Outcome::MissingMarkers
    } else if completed_by_markers && panic_message.is_none() {
        Outcome::Success
    } else {
        classify(
            timed_out,
            &panic_message,
            qemu_exit_code,
            &missing,
            opts.expect_selftest,
            selftest,
        )
    };

    let detail = match outcome {
        Outcome::Success => None,
        Outcome::NoSerialOutput => Some(format!(
            "serial connected after {connect_ms}ms but the guest emitted no bytes within {}s              (firmware/bootloader never reached kernel entry)",
            opts.boot_timeout.as_secs()
        )),
        Outcome::Stalled => Some(format!(
            "guest went silent for more than {}s with evidence still missing",
            opts.stall_timeout.map(|d| d.as_secs()).unwrap_or_default()
        )),
        Outcome::Timeout => Some(format!(
            "guest ran but did not finish within {}s",
            opts.timeout.as_secs()
        )),
        other => Some(format!("{other:?}")),
    };

    // The serial log is the artifact a human opens first. A failing run must
    // never leave an empty file: append the harness verdict so the log always
    // explains itself, even when the guest said nothing at all.
    if let Some(detail) = &detail {
        log_text.push_str(
            "--- harness verdict ---
",
        );
        log_text.push_str(&format!(
            "{outcome:?}: {detail}
"
        ));
    }
    let _ = std::fs::write(&serial_log_path, &log_text);

    RunResult {
        outcome,
        label: opts.label.clone(),
        image: opts.image.display().to_string(),
        firmware_mode: if opts.uefi {
            "uefi".into()
        } else {
            "bios".into()
        },
        qemu_exit_code,
        duration_ms: started.elapsed().as_millis(),
        stages_seen: stages_seen.iter().map(|s| s.code().to_string()).collect(),
        missing_stages: missing,
        missing_required,
        tests,
        selftest_pass: selftest.map(|(p, _)| p),
        selftest_fail: selftest.map(|(_, f)| f),
        panic_message,
        serial_log: serial_log_path.display().to_string(),
        command_line,
        connect_ms: Some(connect_ms),
        first_output_ms: first_output_at.map(|t| t.duration_since(started).as_millis()),
        detail,
    }
}

/// Drain whatever QEMU has already written to stderr without blocking the
/// caller forever: the pipe is read on a worker thread and abandoned if QEMU
/// keeps it open. Used only on failure paths, where the text is diagnostic.
fn drain_stderr(child: &mut Child) -> String {
    let Some(stderr) = child.stderr.take() else {
        return String::new();
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            buf.push_str(&line);
            buf.push('\n');
        }
        let _ = tx.send(buf);
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
}

/// Accept one connection from a nonblocking listener, bounded by `deadline`
/// and aborting if QEMU exits first. Returns the blocking-mode stream.
fn accept_with_deadline(
    listener: &TcpListener,
    child: &mut Child,
    deadline: Instant,
) -> Option<TcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("restoring blocking mode on accepted stream");
                return Some(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline || child.try_wait().ok().flatten().is_some() {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Injection worker: wait for the gate, then play the HMP command sequence
/// into QEMU's monitor. Commands drive the guest's PS/2 keyboard/mouse (so the
/// OS emits `[ITISYOU:INPUT]` markers) and can capture a framebuffer
/// screendump. A `screendump` command is given extra settle time so the PPM is
/// fully flushed before a following command may exit the guest.
fn run_injection(
    mut stream: TcpStream,
    gate: mpsc::Receiver<()>,
    cmds: Vec<String>,
    delay: Duration,
) {
    // Block until the main loop trips the gate (or the channel closes because
    // the gate substring never appeared — then just return without injecting).
    if gate.recv().is_err() {
        return;
    }
    // Let the HMP banner/prompt arrive; we do not parse it (fire-and-forget).
    std::thread::sleep(Duration::from_millis(200));
    for cmd in &cmds {
        if writeln!(stream, "{cmd}").is_err() {
            return;
        }
        let _ = stream.flush();
        if cmd.trim_start().starts_with("screendump") {
            // A screendump writes a PPM synchronously in the monitor thread;
            // give the filesystem time to flush before the next command runs.
            std::thread::sleep(Duration::from_millis(800));
        } else {
            std::thread::sleep(delay);
        }
    }
}

/// Scan a captured WAV for non-silent 16-bit PCM. Tolerant of an unfinalized
/// header (QEMU may not rewrite the RIFF/`data` sizes on an isa-debug-exit
/// kill): locate the `data` chunk if present, else fall back to a 44-byte
/// header offset, and require a meaningful count of above-noise-floor samples.
fn wav_has_nonsilent_pcm(path: &std::path::Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    if bytes.len() < 64 {
        return false;
    }
    let data_off = bytes
        .windows(4)
        .position(|w| w == b"data")
        .map(|p| p + 8)
        .unwrap_or(44);
    if data_off + 4 > bytes.len() {
        return false;
    }
    let mut nonzero = 0usize;
    let (samples, _) = bytes[data_off..].as_chunks::<2>();
    for chunk in samples {
        let s = i16::from_le_bytes(*chunk);
        if s.unsigned_abs() > 64 {
            nonzero += 1;
        }
    }
    nonzero >= 256
}

/// Outcome classification. Precedence: panic > timeout > selftest verdict >
/// marker completeness > exit-code interpretation.
fn classify(
    timed_out: bool,
    panic_message: &Option<String>,
    qemu_exit_code: Option<i32>,
    missing: &[String],
    expect_selftest: bool,
    selftest: Option<(u32, u32)>,
) -> Outcome {
    const EXIT_SUCCESS: i32 = 33; // (0x10 << 1) | 1
    const EXIT_FAILED: i32 = 35; // (0x11 << 1) | 1

    if panic_message.is_some() {
        return Outcome::Panic;
    }
    if timed_out {
        return Outcome::Timeout;
    }
    if expect_selftest {
        return match (selftest, qemu_exit_code) {
            (Some((pass, 0)), Some(EXIT_SUCCESS)) if pass > 0 && missing.is_empty() => {
                Outcome::Success
            }
            (Some((_, fail)), _) if fail > 0 => Outcome::SelftestFailed,
            (Some(_), Some(EXIT_FAILED)) => Outcome::SelftestFailed,
            (Some(_), _) if !missing.is_empty() => Outcome::MissingMarkers,
            (None, _) => Outcome::MissingMarkers,
            _ => Outcome::UnexpectedExit,
        };
    }
    if !missing.is_empty() {
        return Outcome::MissingMarkers;
    }
    match qemu_exit_code {
        Some(EXIT_SUCCESS) => Outcome::Success,
        Some(EXIT_FAILED) => Outcome::SelftestFailed,
        // A guest-initiated reset with -no-reboot exits 0; with required
        // markers all present before that, treat a plain quit as success for
        // non-selftest smoke runs, anything else as unexpected.
        Some(0) => Outcome::Success,
        _ => Outcome::UnexpectedExit,
    }
}
