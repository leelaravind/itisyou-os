//! Regression tests for the harness's startup/timeout state machine.
//!
//! Origin (V0.8): every BIOS leg of the QEMU matrix intermittently failed with
//! an EMPTY serial log. The harness reported `Timeout`/`MissingMarkers` after
//! burning the full leg budget, which read as "flaky harness" and hid the
//! actual fault — the bootloader was reading a 26.8 MiB unstripped kernel ELF
//! through real-mode INT 13h, so the guest's first byte arrived after the leg
//! deadline. These tests pin the behaviour that makes that class of failure
//! self-diagnosing instead of silent:
//!
//!   * a phase-attributed outcome for each way startup can fail;
//!   * fail-fast — the boot deadline must not be allowed to consume the whole
//!     run budget, which is what made the original failures so slow to
//!     reproduce;
//!   * `first_output_ms` recorded as evidence, so a boot path that is merely
//!     getting slower is visible long before it becomes a timeout.
//!
//! `stub-qemu` stands in for QEMU: the real emulator cannot be asked to
//! never connect, or to connect and stay mute, on demand.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const RUNNER: &str = env!("CARGO_BIN_EXE_qemu-runner");
const STUB: &str = env!("CARGO_BIN_EXE_stub-qemu");

/// Cargo-provided scratch dir for integration tests. Keeps artifacts inside
/// the workspace `target/` tree (never the system temp dir).
fn artifacts_dir(label: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(label);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating test artifacts dir");
    dir
}

struct Run {
    outcome: String,
    detail: String,
    first_output_ms: Option<u64>,
    connect_ms: Option<u64>,
    elapsed: Duration,
    serial_log: String,
}

/// Drive the real runner binary against the stub, returning parsed evidence.
fn run_harness(label: &str, mode: &str, extra: &[&str]) -> Run {
    let dir = artifacts_dir(label);
    let mut cmd = Command::new(RUNNER);
    cmd.args(["--image", "test-fixture-no-image.img"])
        .args(["--qemu", STUB])
        .args(["--label", label])
        .arg("--artifacts")
        .arg(&dir)
        .args(extra)
        .env("STUB_QEMU_MODE", mode);
    let started = Instant::now();
    let output = cmd.output().expect("running qemu-runner");
    let elapsed = started.elapsed();

    let json_path = dir.join(format!("{label}.result.json"));
    let json = std::fs::read_to_string(&json_path).unwrap_or_else(|e| {
        panic!(
            "reading {}: {e}\nstdout: {}\nstderr: {}",
            json_path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let value: serde_json::Value = serde_json::from_str(&json).expect("parsing result json");
    let serial_log_path = value["serial_log"].as_str().unwrap_or_default().to_string();
    Run {
        outcome: value["outcome"].as_str().unwrap_or_default().to_string(),
        detail: value["detail"].as_str().unwrap_or_default().to_string(),
        first_output_ms: value["first_output_ms"].as_u64(),
        connect_ms: value["connect_ms"].as_u64(),
        elapsed,
        serial_log: std::fs::read_to_string(serial_log_path).unwrap_or_default(),
    }
}

/// QEMU dies before dialling back: a launch problem, and the exit code has to
/// survive into the evidence so a bad command line is obvious.
#[test]
fn qemu_exiting_before_connecting_is_a_launch_failure() {
    let run = run_harness(
        "startup-launch-failure",
        "exit-immediately",
        &["--timeout-secs", "30", "--connect-timeout-secs", "10"],
    );
    assert_eq!(run.outcome, "launch-failure", "detail: {}", run.detail);
    assert!(
        run.detail.contains("before opening the serial connection"),
        "detail should name the phase, got: {}",
        run.detail
    );
    // Must be attributed the moment the child dies, not at the deadline.
    assert!(
        run.elapsed < Duration::from_secs(8),
        "should fail as soon as QEMU exits, took {:?}",
        run.elapsed
    );
}

/// QEMU is alive but never connects. This is a transport failure and must NOT
/// be reported as a guest verdict — the guest cannot have run yet.
#[test]
fn qemu_never_connecting_is_a_serial_connect_failure() {
    let run = run_harness(
        "startup-connect-failure",
        "never-connect",
        &["--timeout-secs", "60", "--connect-timeout-secs", "3"],
    );
    assert_eq!(
        run.outcome, "serial-connect-failure",
        "detail: {}",
        run.detail
    );
    assert!(
        run.detail.contains("never connected"),
        "detail should name the phase, got: {}",
        run.detail
    );
    // Bounded by connect-timeout, not by the 60s run budget.
    assert!(
        run.elapsed < Duration::from_secs(20),
        "connect timeout must bound the run, took {:?}",
        run.elapsed
    );
}

/// The exact V0.8 failure shape: the serial connection is established and then
/// the guest emits nothing. Previously this consumed the whole leg budget and
/// surfaced as `Timeout`/`MissingMarkers` with an empty log.
#[test]
fn connected_but_mute_guest_is_no_serial_output_and_fails_fast() {
    let run = run_harness(
        "startup-no-serial-output",
        "silent",
        &[
            "--timeout-secs",
            "60",
            "--boot-timeout-secs",
            "3",
            "--expect",
            "B010",
        ],
    );
    assert_eq!(run.outcome, "no-serial-output", "detail: {}", run.detail);
    assert!(run.connect_ms.is_some(), "connection was established");
    assert_eq!(run.first_output_ms, None, "guest emitted nothing");
    // The boot deadline must bite long before the absolute run deadline.
    assert!(
        run.elapsed < Duration::from_secs(20),
        "boot timeout must bound the run, took {:?}",
        run.elapsed
    );
    // The artifact has to explain itself rather than being an empty file.
    assert!(
        run.serial_log.contains("emitted no bytes"),
        "serial log should carry the diagnosis, got: {:?}",
        run.serial_log
    );
}

/// Output starts and then stops. Distinct from both "never spoke" and "ran out
/// of time while working", so a wedged guest is not confused with a slow one.
#[test]
fn guest_going_silent_mid_run_is_stalled() {
    let run = run_harness(
        "startup-stalled",
        "stall",
        &[
            "--timeout-secs",
            "60",
            "--stall-timeout-secs",
            "3",
            "--expect",
            "B010",
            "--expect",
            "B030",
        ],
    );
    assert_eq!(run.outcome, "stalled", "detail: {}", run.detail);
    assert!(
        run.first_output_ms.is_some(),
        "the guest did speak before stalling"
    );
    assert!(
        run.elapsed < Duration::from_secs(25),
        "stall timeout must bound the run, took {:?}",
        run.elapsed
    );
}

/// The healthy path still works through the same state machine, and records
/// the boot-latency evidence that makes a creeping slowdown visible.
#[test]
fn healthy_guest_succeeds_and_records_boot_latency() {
    let run = run_harness(
        "startup-success",
        "markers",
        &[
            "--timeout-secs",
            "30",
            "--expect",
            "B010",
            "--expect",
            "B020",
            "--expect",
            "B030",
        ],
    );
    assert_eq!(run.outcome, "success", "detail: {}", run.detail);
    assert!(
        run.first_output_ms.is_some(),
        "first-output latency must be recorded as evidence"
    );
    assert!(run.connect_ms.is_some(), "connect latency must be recorded");
    assert!(
        run.serial_log.contains("B030"),
        "serial log should hold the captured markers"
    );
}

/// Sub-deadlines may only narrow the run: a leg that asks for a 5 s budget
/// must not be handed the 75 s default boot budget, or "deterministic
/// timeout semantics" would depend on flag ordering.
#[test]
fn sub_deadlines_never_exceed_the_absolute_timeout() {
    let run = run_harness(
        "startup-clamped-deadline",
        "silent",
        &["--timeout-secs", "4", "--expect", "B010"],
    );
    // Default boot timeout is 75s; clamped to the 4s run budget it must still
    // classify as a boot failure rather than a generic timeout.
    assert_eq!(run.outcome, "no-serial-output", "detail: {}", run.detail);
    assert!(
        run.elapsed < Duration::from_secs(20),
        "run budget must bound everything, took {:?}",
        run.elapsed
    );
}
