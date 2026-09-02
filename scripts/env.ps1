# Session environment for ITISYOU OS development.
# Dot-source from every script: `. $PSScriptRoot\env.ps1`
# Keeps all toolchain caches, build output, and temp data off C: (operating rules §5).

$env:RUSTUP_HOME = 'E:\toolchains\rustup'
$env:CARGO_HOME = 'E:\toolchains\cargo'
$env:QEMU_SYSTEM_X86_64 = 'E:\tools\qemu\qemu-system-x86_64.exe'
$env:OVMF_CODE = 'E:\tools\qemu\share\edk2-x86_64-code.fd'
if ($env:PATH -notlike '*E:\tools\qemu*') {
    $env:PATH = "E:\tools\qemu;$env:PATH"
}
# Bulk scratch for anything transient and large.
$env:ITISYOU_SCRATCH = 'G:\claude-tmp'
# Route temp files off C: for all build tooling (operating rules §5).
New-Item -ItemType Directory -Force 'G:\claude-tmp\tmp' | Out-Null
$env:TEMP = 'G:\claude-tmp\tmp'
$env:TMP = 'G:\claude-tmp\tmp'
