# Boot-test the EXACT release images before they are published.
#
#   scripts\release-boot-test.ps1 -Dir <folder with the release .img files> -Version 0.8.1
#
# The images are the CI-built bytes (downloaded from the run's `boot-images`
# artifact and renamed to itisyou-os-<ver>-x86_64-{uefi,bios}.img). Each leg
# boots those files - not a local rebuild - and asserts that the running
# kernel names the release version. Evidence goes to <Dir>\evidence.
param(
    [Parameter(Mandatory = $true)][string]$Dir,
    [Parameter(Mandatory = $true)][string]$Version
)
. $PSScriptRoot\env.ps1
Set-Location (Split-Path $PSScriptRoot -Parent)

$uefi = Join-Path $Dir "itisyou-os-$Version-x86_64-uefi.img"
$bios = Join-Path $Dir "itisyou-os-$Version-x86_64-bios.img"
$evidence = Join-Path $Dir 'evidence'
foreach ($img in @($uefi, $bios)) {
    if (-not (Test-Path $img)) { Write-Output "missing $img"; exit 1 }
}
$failed = $false
function Leg([string[]]$legArgs) {
    cargo run -q -p qemu-runner -- @legArgs --artifacts $evidence
    if ($LASTEXITCODE -ne 0) { $script:failed = $true }
}

$console = @('--expect', 'B010', '--expect', 'B020', '--expect', 'B030', '--expect', 'B150',
    '--send', 'version', '--send', 'cat /etc/version', '--send', 'help', '--send', 'ps', '--send', 'shutdown',
    '--require', "itisyou-os $Version", '--require', 'shutting down (QEMU exit)',
    # Since v0.10.0 the userspace init runs as pid 1 on every boot.
    '--require', 'pid=1 ppid=0 path=/sbin/init', '--require', 'path=/bin/tickd state=',
    '--timeout-secs', '120')

Write-Output "=== release ${Version}: UEFI boot to the console ==="
Leg (@('--image', $uefi, '--uefi') + $console + @('--label', "release-$Version-uefi"))
Write-Output "=== release ${Version}: BIOS boot to the console ==="
Leg (@('--image', $bios) + $console + @('--label', "release-$Version-bios"))

# The documented persistent-store setup: a blank disk, written in one boot and
# read back by a fresh guest.
$disk = Join-Path $Dir 'release-data-disk.img'
if (Test-Path $disk) { Remove-Item -LiteralPath $disk }
Write-Output "=== release ${Version}: persistent /data store, boot 1 ==="
Leg @('--image', $uefi, '--uefi', '--nvme-persist', $disk, '--expect', 'B190',
    '--send', 'store put release-check.txt persisted-across-reboot', '--send', 'store ls', '--send', 'shutdown',
    '--require', 'release-check.txt', '--timeout-secs', '120', '--label', "release-$Version-persist-write")
Write-Output "=== release ${Version}: persistent /data store, boot 2 (fresh guest) ==="
Leg @('--image', $uefi, '--uefi', '--nvme-persist', $disk, '--expect', 'B190',
    '--send', 'store ls', '--send', 'store cat release-check.txt', '--send', 'shutdown',
    '--require', 'persisted-across-reboot', '--timeout-secs', '120', '--label', "release-$Version-persist-verify")
if (Test-Path $disk) { Remove-Item -LiteralPath $disk }

# The documented network setup: QEMU's user-mode network is an independent
# IPv4 implementation; the guest must resolve the gateway and get echo replies.
Write-Output "=== release ${Version}: QEMU user-mode network ==="
Leg @('--image', $bios, '--net-user', '--expect', 'B200',
    '--send', 'ping 10.0.2.2 2', '--send', 'shutdown',
    '--require', 'PING-SUMMARY target=10.0.2.2 sent=2 received=2',
    '--timeout-secs', '120', '--label', "release-$Version-net-user")

foreach ($img in @($uefi, $bios)) {
    $h = (Get-FileHash -LiteralPath $img -Algorithm SHA256).Hash.ToLower()
    Write-Output "$h  $(Split-Path $img -Leaf)"
}
if ($failed) { Write-Output 'RELEASE-BOOT-TEST: FAILED'; exit 1 }
Write-Output 'RELEASE-BOOT-TEST: OK'
exit 0
