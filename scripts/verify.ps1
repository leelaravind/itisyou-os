# Canonical one-command verification gate (implementation plan §12.1).
# Runs every applicable local check; returns non-zero on the first failure
# of a required stage. Website stages activate once website/ has a package.json.
. $PSScriptRoot\env.ps1
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Get-Location

function Stage($name, $script) {
    Write-Output ''
    Write-Output "===== verify: $name ====="
    & $script
    if ($LASTEXITCODE -ne 0) {
        Write-Output "VERIFY: FAILED at stage '$name'"
        exit 1
    }
}

Stage 'doctor' { & "$root\scripts\doctor.ps1" }
Stage 'format' { cargo fmt --all -- --check }
Stage 'clippy' { cargo clippy --workspace --all-targets -- -D warnings }
Stage 'host-tests + qemu-matrix' { & "$root\scripts\test.ps1" }

if (Test-Path "$root\website\package.json") {
    Stage 'website' {
        Push-Location "$root\website"
        npm run verify
        $code = $LASTEXITCODE
        Pop-Location
        if ($code -ne 0) { $global:LASTEXITCODE = $code }
    }
}

Stage 'secret-scan' { & "$root\scripts\secret-scan.ps1" }

Write-Output ''
Write-Output 'VERIFY: OK - all applicable gates passed'
exit 0
