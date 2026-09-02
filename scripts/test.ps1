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
& $runner @('--image', 'target/images/itisyou-kernel-selftest-bios.img',
    '--expect', 'B010', '--expect', 'B020', '--expect', 'B030',
    '--expect-selftest', '--timeout-secs', '90', '--label', 'selftest-bios')

Write-Output '=== QEMU selftest (UEFI) ==='
if (Test-Path 'target/images/itisyou-kernel-selftest-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-selftest-uefi.img', '--uefi',
        '--expect', 'B010', '--expect', 'B020', '--expect', 'B030',
        '--expect-selftest', '--timeout-secs', '90', '--label', 'selftest-uefi')
} else {
    Write-Output 'SKIPPED: UEFI image absent - bootloader UEFI stage blocked upstream (rust-osdev/bootloader#579)'
}

if ($anyFailed) { Write-Output 'TEST: FAILED'; exit 1 }
Write-Output 'TEST: OK'
exit 0
