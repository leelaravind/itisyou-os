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
//!               [--artifacts artifacts/qemu] [--qemu <path>]

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
    /// Attach an emulated NVMe controller backed by a generated raw disk
    /// whose first sector carries a known magic (for storage read tests).
    nvme: bool,
    timeout: Duration,
    label: String,
    artifacts: PathBuf,
    qemu: String,
}

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
    let mut nvme = false;
    let mut timeout = Duration::from_secs(60);
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
            "--nvme" => nvme = true,
            "--timeout-secs" => {
                timeout = Duration::from_secs(
                    value("--timeout-secs")?
                        .parse()
                        .map_err(|e| format!("bad timeout: {e}"))?,
                )
            }
            "--label" => label = value("--label")?,
            "--artifacts" => artifacts = PathBuf::from(value("--artifacts")?),
            "--qemu" => qemu = value("--qemu")?,
            other => return Err(format!("unknown argument {other}")),
        }
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
        nvme,
        timeout,
        label,
        artifacts,
        qemu,
    })
}

/// Create (or overwrite) the NVMe backing disk: 1 MiB raw, LBA 0 begins with
/// NVME_DISK_MAGIC then a deterministic pattern. Returns its path.
fn make_nvme_disk(artifacts: &std::path::Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(artifacts).ok();
    let path = artifacts.join("nvme-disk.img");
    let mut buf = vec![0u8; 1024 * 1024];
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
        "qemu-runner: label={} outcome={:?} exit={:?} stages={} duration={}ms evidence={}",
        result.label,
        result.outcome,
        result.qemu_exit_code,
        result.stages_seen.len(),
        result.duration_ms,
        json_path.display(),
    );
    if !ok {
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
fn build_command(opts: &Options, serial_port: u16) -> Command {
    let mut cmd = Command::new(&opts.qemu);
    cmd.arg("-drive")
        .arg(format!("format=raw,file={}", opts.image.display()))
        .arg("-serial")
        .arg(format!("tcp:127.0.0.1:{serial_port},nodelay"))
        .args(["-display", "none", "-no-reboot", "-m", "256M"])
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
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

    let mut cmd = build_command(opts, serial_port);
    let command_line: Vec<String> = std::iter::once(opts.qemu.clone())
        .chain(cmd.get_args().map(|a| a.to_string_lossy().into_owned()))
        .collect();

    let started = Instant::now();
    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::write(&serial_log_path, format!("LAUNCH FAILURE: {e}\n"));
            return RunResult {
                outcome: Outcome::LaunchFailure,
                label: opts.label.clone(),
                image: opts.image.display().to_string(),
                firmware_mode: if opts.uefi {
                    "uefi".into()
                } else {
                    "bios".into()
                },
                qemu_exit_code: None,
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
            };
        }
    };

    // Accept QEMU's serial connection (it connects during device realize).
    let deadline = started + opts.timeout;
    let serial_stream: Option<TcpStream> = loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Windows: streams accepted from a nonblocking listener
                // inherit nonblocking mode — force blocking reads back on,
                // or the reader thread dies instantly on WouldBlock.
                stream
                    .set_nonblocking(false)
                    .expect("restoring blocking mode on serial stream");
                stream.set_nodelay(true).ok();
                break Some(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline || child.try_wait().ok().flatten().is_some() {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break None,
        }
    };
    let Some(serial_stream) = serial_stream else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::write(
            &serial_log_path,
            "LAUNCH FAILURE: QEMU never connected to the serial TCP port\n",
        );
        return RunResult {
            outcome: Outcome::LaunchFailure,
            label: opts.label.clone(),
            image: opts.image.display().to_string(),
            firmware_mode: if opts.uefi {
                "uefi".into()
            } else {
                "bios".into()
            },
            qemu_exit_code: None,
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
        };
    };
    let mut serial_writer = serial_stream.try_clone().expect("cloning serial stream");

    // Stream serial lines through a channel so the main loop owns the timeout.
    let (tx, rx) = mpsc::channel::<String>();
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
    let stderr = child.stderr.take().expect("stderr piped");
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
    let mut completed_by_markers = false;
    let mut sent_commands = false;

    loop {
        let now = Instant::now();
        if now >= deadline {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(line) => {
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
                serial_lines.push(line);
                if opts.expect_panic && panic_message.is_some() {
                    // Evidence captured; the panicked kernel halts forever,
                    // so end the run here.
                    let _ = child.kill();
                    break;
                }
                if opts.exit_after_markers
                    && !opts.expect.is_empty()
                    && panic_message.is_none()
                    && opts.expect.iter().all(|s| stages_seen.contains(s))
                {
                    completed_by_markers = true;
                    let _ = child.kill();
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                let _ = child.kill();
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
    while child.try_wait().ok().flatten().is_none() {
        if Instant::now() >= grace_deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = reader_handle.join();
    let exit_status = child.wait().ok();
    let qemu_exit_code = exit_status.and_then(|s| s.code());
    let stderr_text = stderr_handle.join().unwrap_or_default();

    let mut log_text = serial_lines.join("\n");
    log_text.push('\n');
    if !stderr_text.is_empty() {
        log_text.push_str("--- qemu stderr ---\n");
        log_text.push_str(&stderr_text);
    }
    let _ = std::fs::write(&serial_log_path, &log_text);

    let missing: Vec<String> = opts
        .expect
        .iter()
        .filter(|s| !stages_seen.contains(s))
        .map(|s| s.code().to_string())
        .collect();
    let missing_required: Vec<String> = opts
        .require
        .iter()
        .filter(|needle| !serial_lines.iter().any(|l| l.contains(needle.as_str())))
        .cloned()
        .collect();

    let outcome = if opts.expect_panic {
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
    }
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
