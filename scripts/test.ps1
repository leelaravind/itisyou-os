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
    '--timeout-secs', '60', '--label', 'boot-smoke-bios')

Write-Output '=== QEMU boot smoke (UEFI) ==='
if (Test-Path 'target/images/itisyou-kernel-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-uefi.img', '--uefi',
        '--expect', 'B010', '--expect', 'B020', '--expect', 'B030',
        '--exit-after-markers',
        '--timeout-secs', '60', '--label', 'boot-smoke-uefi')
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
    '--timeout-secs', '180', '--label', 'selftest-bios')

Write-Output '=== QEMU selftest (UEFI) ==='
if (Test-Path 'target/images/itisyou-kernel-selftest-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-selftest-uefi.img', '--uefi', '--nvme',
        '--expect', 'B010', '--expect', 'B020', '--expect', 'B030', '--expect', 'B140',
        '--expect', 'B160', '--expect-selftest',
        '--require', 'RING3-HELLO', '--require', 'RING3-DONE',
        '--require', 'RING3-PARENT-DONE', '--require', 'RING3-PARENT-IPC-OK',
        '--require', 'nvme_ready blocks=',
        '--timeout-secs', '180', '--label', 'selftest-uefi')
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
    '--require', 'itisyou-os 0.2.0-dev',
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

Write-Output '=== QEMU intentional panic (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-panictest-bios.img',
    '--expect', 'B010', '--expect', 'B020', '--expect-panic',
    '--require', 'intentional panic-test',
    '--timeout-secs', '60', '--label', 'panic-test-bios')

if ($anyFailed) { Write-Output 'TEST: FAILED'; exit 1 }
Write-Output 'TEST: OK'
exit 0
