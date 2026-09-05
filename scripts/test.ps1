# Full local test suite: host unit tests + QEMU boot/selftest matrix.
. $PSScriptRoot\env.ps1
Set-Location (Split-Path $PSScriptRoot -Parent)

Write-Output '=== host unit tests ==='
cargo test -p kernel-core -p image-builder -p qemu-runner
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Output '=== build images ==='
cargo run -p image-builder -- target/images
if ($LASTEXITCODE -ne 0) { exit 1 }

$runner = { param($runnerArgs)
    cargo run -q -p qemu-runner -- @runnerArgs
    if ($LASTEXITCODE -ne 0) { $script:anyFailed = $true }
}
$anyFailed = $false

Write-Output '=== QEMU boot smoke (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B010', '--expect', 'B020', '--expect', 'B030',
    '--exit-after-markers',
    '--timeout-secs', '120', '--label', 'boot-smoke-bios')

Write-Output '=== QEMU boot smoke (UEFI) ==='
if (Test-Path 'target/images/itisyou-kernel-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-uefi.img', '--uefi',
        '--expect', 'B010', '--expect', 'B020', '--expect', 'B030',
        '--exit-after-markers',
        '--timeout-secs', '120', '--label', 'boot-smoke-uefi')
} else {
    Write-Output 'SKIPPED: UEFI image absent - bootloader UEFI stage blocked upstream (rust-osdev/bootloader#579)'
}

Write-Output '=== QEMU selftest (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-selftest-bios.img', '--nvme',
    '--expect', 'B010', '--expect', 'B020', '--expect', 'B030', '--expect', 'B140',
    '--expect', 'B160', '--expect-selftest',
    '--require', 'RING3-HELLO', '--require', 'RING3-DONE',
    '--require', 'user_exit pid=', '--require', 'contained=true',
    '--require', 'RING3-PARENT-DONE', '--require', 'RING3-PARENT-WAIT-OK',
    '--require', 'RING3-PARENT-IPC-OK', '--require', 'RING3-CHILD-EXIT',
    '--require', 'NVMe controller', '--require', 'nvme_ready blocks=',
    '--require', 'SPIN-FINITE-OK',
    '--require', 'gfx width=', '--require', 'RING3-GUI-OK',
    '--expect', 'B170', '--expect', 'B180',
    '--timeout-secs', '300', '--label', 'selftest-bios')

Write-Output '=== QEMU selftest (UEFI) ==='
if (Test-Path 'target/images/itisyou-kernel-selftest-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-selftest-uefi.img', '--uefi', '--nvme',
        '--expect', 'B010', '--expect', 'B020', '--expect', 'B030', '--expect', 'B140',
        '--expect', 'B160', '--expect-selftest',
        '--require', 'RING3-HELLO', '--require', 'RING3-DONE',
        '--require', 'RING3-PARENT-DONE', '--require', 'RING3-PARENT-IPC-OK',
        '--require', 'nvme_ready blocks=', '--require', 'SPIN-FINITE-OK',
        '--timeout-secs', '300', '--label', 'selftest-uefi')
} else {
    Write-Output 'SKIPPED: UEFI image absent - bootloader UEFI stage blocked upstream (rust-osdev/bootloader#579)'
}

Write-Output '=== QEMU shell interaction (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B040', '--expect', 'B050', '--expect', 'B060', '--expect', 'B070',
    '--expect', 'B080', '--expect', 'B090', '--expect', 'B100', '--expect', 'B110',
    '--expect', 'B120', '--expect', 'B130', '--expect', 'B140', '--expect', 'B150',
    '--expect', 'B160',
    '--send', 'help', '--send', 'version', '--send', 'system', '--send', 'memory',
    '--send', 'tasks', '--send', 'uptime', '--send', 'ls /', '--send', 'cat /etc/version',
    '--send', 'echo shell-echo-check', '--send', 'definitely-not-a-command',
    '--send', 'run /bin/init', '--send', 'run /bin/broken',
    '--send', 'panic-test', '--send', 'shutdown',
    '--require', 'itisyou-os 0.7.0-dev',
    '--require', 'task 0: kmain',
    '--require', 'RING3-DONE',
    '--require', 'run: /bin/init: Exit(0)',
    '--require', 'load failed: Elf(BadMagic)',
    '--require', 'physical: usable_frames=',
    '--require', 'heap: used=',
    '--require', 'etc/',
    '--require', 'shell-echo-check',
    '--require', 'unknown command: definitely-not-a-command',
    '--require', "panic-test: pass 'confirm'",
    '--require', 'shutting down (QEMU exit)',
    '--timeout-secs', '90', '--label', 'shell-test-bios')

Write-Output '=== QEMU desktop graphics + PS/2 input (BIOS) ==='
# Boot to the shell, enter the graphical desktop, then inject real keyboard and
# mouse events through the QEMU HMP monitor (into the emulated PS/2 devices).
# The kernel's IRQ1/IRQ12 handlers must observe them (INPUT markers) and the
# compositor captures a framebuffer screendump - the OS renders the desktop,
# no host UI. DESKTOP-INPUT-VERIFIED proves the full graphics+input path.
$deskShot = Join-Path (Resolve-Path 'artifacts/qemu') 'desktop.ppm'
Remove-Item $deskShot -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B170', '--expect', 'B180',
    '--send', 'desktop',
    '--inject-after', 'DESKTOP-READY',
    '--monitor-cmd', 'sendkey h', '--monitor-cmd', 'sendkey e', '--monitor-cmd', 'sendkey l',
    '--monitor-cmd', 'sendkey l', '--monitor-cmd', 'sendkey o',
    '--monitor-cmd', 'mouse_move 40 25', '--monitor-cmd', 'mouse_move -20 15',
    '--monitor-cmd', 'mouse_button 1', '--monitor-cmd', 'mouse_button 0',
    '--monitor-cmd', "screendump $deskShot",
    '--monitor-cmd', 'sendkey esc',
    '--require', '[ITISYOU:MODE] desktop',
    '--require', 'DESKTOP-READY',
    '--require', '[ITISYOU:INPUT] key=h',
    '--require', '[ITISYOU:INPUT] key=o',
    '--require', '[ITISYOU:INPUT] mouse dx=',
    '--require', 'DESKTOP-INPUT-VERIFIED',
    '--timeout-secs', '150', '--label', 'desktop-input-bios')

Write-Output '=== QEMU device model + AC97 audio (BIOS) ==='
# Attach an AC97 audio controller whose output is captured to a WAV via QEMU's
# `wav` backend. The OS binds the ac97 driver (B190 device model), the `beep`
# command synthesizes a tone and DMAs it through the PCM-Out bus master, and
# the runner asserts (a) the driver init + full buffer consumption markers and
# (b) that the captured WAV contains non-silent PCM — proving the generated
# samples traversed driver -> controller -> output, not just init.
$audioWav = Join-Path (Resolve-Path 'artifacts/qemu') 'audio.wav'
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B190',
    '--audio', '--audio-out', $audioWav,
    '--send', 'lsdev', '--send', 'beep', '--send', 'shutdown',
    '--require', 'devmodel devices=',
    '--require', 'driver_bound name=', '--require', 'ac97 init',
    '--require', 'ac97 play', '--require', 'halted=true',
    '--timeout-secs', '150', '--label', 'audio-bios')

Write-Output '=== QEMU USB (UHCI) enumeration + HID input into the desktop (BIOS) ==='
# Attach a UHCI controller + USB HID keyboard. The OS binds the uhci driver
# (B190), resets the root port, and enumerates over control transfers (device +
# configuration descriptors, SET_ADDRESS/CONFIG/PROTOCOL). Then the graphical
# desktop polls the HID interrupt endpoint through the *unified* input queue
# (same InputEvent stream as PS/2); an injected keypress reaches the desktop
# tagged src=usb - proving both real USB HID input and input-source unification.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B190', '--usb',
    '--send', 'desktop',
    '--inject-after', 'DESKTOP-READY',
    '--monitor-cmd', 'sendkey g', '--monitor-cmd', 'sendkey esc',
    '--require', 'uhci ready', '--require', 'usb device vendor=',
    '--require', 'usb hid iface=', '--require', 'key=g src=usb',
    '--require', 'DESKTOP-INPUT-VERIFIED',
    '--timeout-secs', '150', '--label', 'usb-hid-bios')

Write-Output '=== QEMU system platform: caps, sandbox, services, packages (BIOS) ==='
# The V0.7 platform driven through the shell over a persistent NVMe store:
# a zero-capability probe proves default deny; a sandboxed fs-probe proves the
# path sandbox; the service supervisor runs (echod serves a client, crashd is
# restarted 3x then Failed); packages install/launch/update/rollback with the
# hello-app running under manifest-only capabilities; the audit trail shows
# denials + privileged actions. All values are real kernel/platform state.
$platDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-platform-test.img'
Remove-Item $platDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $platDisk,
    '--expect', 'B190',
    '--send', 'run /bin/sandbox-probe -',
    '--send', 'run /bin/cap-handle-probe',
    '--send', 'run /bin/fs-probe fs_read /etc',
    '--send', 'svc',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'pkg install /pkgs/hello-app-bad.itpkg',
    '--send', 'pkg launch hello-app',
    '--send', 'pkg install /pkgs/hello-app-2.itpkg',
    '--send', 'pkg rollback hello-app',
    '--send', 'pkg launch hello-app',
    '--send', 'pkg list',
    '--send', 'audit',
    '--send', 'shutdown',
    '--require', 'SANDBOX-DENIED-OK',
    '--require', 'FS-SANDBOX-OK',
    # V0.8 capability-handle ENFORCEMENT (not just issuance): a forged
    # generation, an expired handle and a revoked handle are each refused on
    # the real syscall path, with the precise reason in the audit trail.
    '--require', 'CAPH-FORGED-DENIED',
    '--require', 'CAPH-EXPIRED-DENIED',
    '--require', 'CAPH-REVOKED-DENIED',
    '--require', 'CAPH-ENFORCEMENT-OK',
    '--require', 'reason=expired',
    '--require', 'reason=no_handle',
    '--require', 'ECHOD-SERVED-3', '--require', 'SVC-CLIENT-OK',
    '--require', 'name=crashd state=failed pid=', '--require', 'restarts=3',
    '--require', 'install name=hello-app v=1 result=ok',
    '--require', 'verify result=refused reason=DigestMismatch',
    '--require', 'HELLO-APP-OK',
    '--require', 'rollback name=hello-app from=v2 to=v1 result=ok',
    '--require', 'result=denied',
    '--timeout-secs', '240', '--label', 'platform-bios')

Write-Output '=== QEMU long-running background services (BIOS) ==='
# V0.8 persistent services: `tickd` and `flapd` are ordinary Ring 3 processes
# started at boot with exactly the capabilities they declare (tickd IPC-only,
# flapd none), co-scheduled with the interactive shell instead of run to
# completion. The assertions prove all four properties that distinguish a
# long-running service from V0.7's run-to-completion tasks:
#   * it is still ALIVE later in the boot - heartbeats are stamped with real
#     kernel ticks, not scheduling passes, so the cadence is load-independent;
#   * it is still SERVING, not merely resident - two `bg` clients complete an
#     IPC round trip against the SAME instance (tickd's own counter reaches
#     n=2, which a restarted service could never print);
#   * a daemon's clean exit is a FAULT - flapd returns immediately and is
#     restarted under the bounded policy, then marked Failed at the ceiling
#     instead of restarting forever;
#   * the system stays healthy and interactive throughout - the on-demand
#     supervisor still runs its own services correctly alongside them, the
#     `svc` table reports background and on-demand state together, and the
#     shell still accepts commands afterwards.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B190',
    '--send', 'bg /bin/tick-client',
    '--send', 'bg /bin/spin-finite',
    '--send', 'svc',
    '--send', 'bg /bin/tick-client',
    '--send', 'shutdown',
    '--require', 'bg_start name=tickd pid=1 caps=0x2 long_running=true',
    '--require', 'bg_start name=flapd pid=2 caps=0x0 long_running=true',
    '--require', 'TICKD-READY',
    '--require', 'TICKD-ALIVE tick=',
    '--require', 'TICKD-SERVED n=2',
    '--require', 'TICKC-OK passes=',
    '--require', 'bg_restart name=flapd pid=',
    '--require', 'clean_exit=true',
    '--require', 'bg_failed name=flapd restarts=3',
    '--require', 'name=flapd state=failed',
    '--require', 'tickd  Running',
    '--require', 'flapd  Failed { restarts: 3 }',
    '--require', 'SVC-CLIENT-OK',
    '--require', 'svc: started=3 done=2 failed=1 restarts=3',
    '--require', 'shutting down (QEMU exit)',
    '--timeout-secs', '240', '--label', 'services-bg-bios')

Write-Output '=== QEMU interrupted update -> recovery (BIOS, two boots) ==='
# Boot 1 installs v1 and STAGES v2 without committing (a simulated crash mid-
# update), then powers off. Boot 2 (fresh guest, same disk) must find v1
# still active, detect the orphaned staged v2, remove it via the recovery
# scan, and launch v1 - an interrupted update can never activate, and
# recovery works across a real reboot.
$updDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-update-test.img'
Remove-Item $updDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $updDisk,
    '--expect', 'B190',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'pkg stage /pkgs/hello-app-2.itpkg',
    '--send', 'shutdown',
    '--require', 'install name=hello-app v=1 result=ok',
    '--require', 'stage name=hello-app v=2',
    '--timeout-secs', '180', '--label', 'update-interrupt')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $updDisk,
    '--expect', 'B190',
    '--send', 'pkg recover',
    '--send', 'pkg launch hello-app',
    '--send', 'shutdown',
    '--require', 'orphan_staged=v2 action=remove',
    '--require', 'launch name=hello-app v=1',
    '--require', 'HELLO-APP-OK',
    '--timeout-secs', '180', '--label', 'update-recovery')
Remove-Item $updDisk -ErrorAction SilentlyContinue
Remove-Item $platDisk -ErrorAction SilentlyContinue

Write-Output '=== QEMU intentional panic (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-panictest-bios.img',
    '--expect', 'B010', '--expect', 'B020', '--expect-panic',
    '--require', 'intentional panic-test',
    '--timeout-secs', '60', '--label', 'panic-test-bios')

Write-Output '=== QEMU filesystem reboot persistence (UEFI, two boots) ==='
# Two separate QEMU guests share one disposable persistent disk: boot 1
# formats ITFS + writes /hello, boot 2 (fresh guest) mounts + verifies it —
# proving the write survived a full reboot through the real block layer.
# (UEFI: the fs-persist binary's BIOS image does not boot — see
# docs/DEVELOPMENT_STORY; UEFI is a fully verified firmware path.)
$persistDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-fs-persist-test.img'
Remove-Item $persistDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-fs-persist-uefi.img', '--uefi',
    '--nvme-persist', $persistDisk,
    '--expect', 'B010', '--expect', 'B160',
    '--require', 'FS-PERSIST-WROTE',
    '--timeout-secs', '90', '--label', 'fs-persist-write')
& $runner @('--image', 'target/images/itisyou-fs-persist-uefi.img', '--uefi',
    '--nvme-persist', $persistDisk,
    '--expect', 'B010', '--expect', 'B160',
    '--require', 'FS-PERSIST-VERIFIED',
    '--timeout-secs', '90', '--label', 'fs-persist-verify')
Remove-Item $persistDisk -ErrorAction SilentlyContinue

if ($anyFailed) { Write-Output 'TEST: FAILED'; exit 1 }
Write-Output 'TEST: OK'
exit 0
