# Launch the interactive kernel in QEMU.
# -Firmware uefi (default) boots via OVMF; bios uses the legacy image.
# -Headless suppresses the display window (serial only).
param(
    [ValidateSet('bios', 'uefi')] [string]$Firmware = 'uefi',
    [switch]$Headless,
    [switch]$Selftest
)
. $PSScriptRoot\env.ps1
Set-Location (Split-Path $PSScriptRoot -Parent)

$variant = if ($Selftest) { 'itisyou-kernel-selftest' } else { 'itisyou-kernel' }
$image = "target/images/$variant-$Firmware.img"
if (-not (Test-Path $image)) {
    Write-Output "Image $image missing - run scripts/build.ps1 first."
    exit 1
}

$qemuArgs = @(
    '-drive', "format=raw,file=$image",
    '-serial', 'stdio',
    '-no-reboot',
    '-m', '256M',
    '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04'
)
if ($Firmware -eq 'uefi') {
    $qemuArgs += @('-drive', "if=pflash,format=raw,readonly=on,file=$env:OVMF_CODE")
}
if ($Headless) { $qemuArgs += @('-display', 'none') }

& $env:QEMU_SYSTEM_X86_64 @qemuArgs
exit $LASTEXITCODE
