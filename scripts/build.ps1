# Build the kernel and produce bootable disk images under target/images.
param([switch]$Release)
. $PSScriptRoot\env.ps1
Set-Location (Split-Path $PSScriptRoot -Parent)

$flags = @()
if ($Release) { $flags += '--release' }

cargo run -p image-builder @flags -- target/images
exit $LASTEXITCODE
