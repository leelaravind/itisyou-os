# Environment doctor: inspect before installing (implementation plan §8).
# Exits non-zero if a required tool is missing. Never installs anything itself.
. $PSScriptRoot\env.ps1
$failed = $false

function Check($name, $ok, $detail) {
    $mark = if ($ok) { 'OK  ' } else { 'FAIL' }
    Write-Output "[$mark] $name $detail"
    if (-not $ok) { $script:failed = $true }
}

$git = Get-Command git -ErrorAction SilentlyContinue
Check 'git' ([bool]$git) $(if ($git) { (git --version) } else { 'not found' })

$gh = Get-Command gh -ErrorAction SilentlyContinue
Check 'gh' ([bool]$gh) $(if ($gh) { (gh --version | Select-Object -First 1) } else { 'not found' })

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
Check 'cargo' ([bool]$cargo) $(if ($cargo) { (cargo --version) } else { 'not found' })

$rustup = Get-Command rustup -ErrorAction SilentlyContinue
Check 'rustup' ([bool]$rustup) "RUSTUP_HOME=$env:RUSTUP_HOME"

$qemu = Test-Path $env:QEMU_SYSTEM_X86_64
Check 'qemu-system-x86_64' $qemu $(if ($qemu) { (& $env:QEMU_SYSTEM_X86_64 --version | Select-Object -First 1) } else { $env:QEMU_SYSTEM_X86_64 })

$ovmf = Test-Path $env:OVMF_CODE
Check 'OVMF firmware' $ovmf $env:OVMF_CODE

$node = Get-Command node -ErrorAction SilentlyContinue
Check 'node' ([bool]$node) $(if ($node) { (node --version) } else { 'not found' })

# Storage rules: nothing project-related may intentionally target C:.
Check 'RUSTUP_HOME off C:' ($env:RUSTUP_HOME -notlike 'C:*') $env:RUSTUP_HOME
Check 'CARGO_HOME off C:' ($env:CARGO_HOME -notlike 'C:*') $env:CARGO_HOME

Check 'TEMP off C:' ($env:TEMP -notlike 'C:*') $env:TEMP

$freeE = [math]::Round((Get-PSDrive E).Free / 1GB, 1)
Check 'free space E: >= 5GB' ($freeE -ge 5) "$freeE GB free"
# G: is an optional external scratch drive; env.ps1 falls back to E: without it.
$gDrive = Get-PSDrive G -ErrorAction SilentlyContinue
if ($gDrive) {
    $freeG = [math]::Round($gDrive.Free / 1GB, 1)
    Check 'free space G: >= 5GB' ($freeG -ge 5) "$freeG GB free"
} else {
    Write-Output "[OK  ] scratch G: not attached; using $env:ITISYOU_SCRATCH"
}

if ($failed) { Write-Output 'DOCTOR: FAILED'; exit 1 }
Write-Output 'DOCTOR: OK'
exit 0
