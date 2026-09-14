# V1-REL-005 evidence: every kernel_core decoder of untrusted bytes survives
# ITISYOU_FUZZ_ITERS seeded mutated inputs with no panic, no overflow and no
# hang. Runs the ignored `full_*` targets of
# crates/kernel-core/tests/fuzz_decoders.rs twice:
#
#   1. RELEASE, -Iterations per target (default 1 000 000): the volume pass;
#   2. DEBUG, -DebugIterations per target (default 100 000): the same targets
#      with debug assertions and integer-overflow checks compiled in.
#
# Each target prints one `FUZZ-TARGET name=... iters=... accepted=...
# refused=...` line; the harness's own watchdog ends the run (exit 3) when a
# target stops making progress, and this script adds a wall-clock backstop.
# Ends with exactly one verdict line:
#
#   FUZZ-DECODERS: OK <n> decoders x <iters> (release) + <n> x <debug-iters> (debug)
#   FUZZ-DECODERS: FAILED <reason>
#
# The run is deterministic for a given -Seed; a failing iteration prints the
# ITISYOU_FUZZ_REPLAY=<target>:<iteration> command that reproduces it alone.
# The process runs at below-normal priority so a concurrent QEMU matrix keeps
# its timing.
param(
    [long]$Iterations = 1000000,
    [long]$DebugIterations = 100000,
    [int]$TestThreads = 0,
    [int]$FuzzThreads = 0,
    [string]$Seed = '',
    [int]$TimeoutMinutes = 240,
    [switch]$SkipDebug
)

. $PSScriptRoot\env.ps1
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Get-Location
try {
    [System.Diagnostics.Process]::GetCurrentProcess().PriorityClass = 'BelowNormal'
} catch {
    Write-Host 'fuzz-decoders: could not lower the process priority; continuing'
}

$cores = [Environment]::ProcessorCount
if ($TestThreads -le 0) { $TestThreads = [Math]::Max(1, [int]($cores / 2)) }
$logDir = Join-Path $root 'target\fuzz-decoders'
New-Item -ItemType Directory -Force $logDir | Out-Null

# Everything a function prints goes through Write-Host: Write-Output inside
# a function would become part of its return value (and a failure verdict
# printed inside Invoke-Pass would never reach the log).
function Fail($why) {
    Write-Host "FUZZ-DECODERS: FAILED $why"
    exit 1
}

# Build the test binary for a profile and return its path.
function Build-Harness($profileArgs) {
    $json = & cargo test -p kernel-core --test fuzz_decoders @profileArgs --no-run --message-format=json 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    foreach ($line in $json) {
        if ($line -notmatch '"executable"') { continue }
        $msg = $line | ConvertFrom-Json
        if ($msg.target.name -eq 'fuzz_decoders' -and $msg.executable) { return $msg.executable }
    }
    return $null
}

# Run every ignored target once with $iters iterations; returns the parsed
# per-target results, or calls Fail.
function Invoke-Pass($label, $profileArgs, [long]$iters) {
    Write-Host ''
    Write-Host "===== fuzz-decoders: $label pass, $iters iterations per target ====="
    $exe = Build-Harness $profileArgs
    if (-not $exe) { Fail "$label build of the fuzz harness" }
    $listed = & $exe --list --ignored 2>$null | Where-Object { $_ -match '^full_\S+: test$' }
    $expected = @($listed).Count
    if ($expected -eq 0) { Fail "$label harness lists no full_* targets" }

    $env:ITISYOU_FUZZ_ITERS = "$iters"
    if ($FuzzThreads -gt 0) { $env:ITISYOU_FUZZ_THREADS = "$FuzzThreads" } else { Remove-Item Env:ITISYOU_FUZZ_THREADS -ErrorAction SilentlyContinue }
    if ($Seed) { $env:ITISYOU_FUZZ_SEED = $Seed } else { Remove-Item Env:ITISYOU_FUZZ_SEED -ErrorAction SilentlyContinue }
    Remove-Item Env:ITISYOU_FUZZ_REPLAY -ErrorAction SilentlyContinue

    $log = Join-Path $logDir "$label.log"
    $err = Join-Path $logDir "$label.stderr.log"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $exe -ArgumentList @('--ignored', '--nocapture', "--test-threads=$TestThreads") `
        -NoNewWindow -PassThru -RedirectStandardOutput $log -RedirectStandardError $err
    $null = $p.Handle # PowerShell 5.1 reports no ExitCode unless the handle is cached
    if (-not $p.WaitForExit($TimeoutMinutes * 60 * 1000)) {
        # Only the harness process this script started is stopped.
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        Fail "$label pass exceeded $TimeoutMinutes minutes (see $log)"
    }
    $p.WaitForExit()
    $secs = [Math]::Round($sw.Elapsed.TotalSeconds, 1)
    $code = $p.ExitCode

    $text = Get-Content $log, $err -ErrorAction SilentlyContinue
    # Unanchored: libtest may print `test x ... ` just before another
    # test's line.
    $text | Where-Object { $_ -match 'FUZZ-(FAIL|HANG)' -or $_ -match 'panicked at' } | ForEach-Object { Write-Host $_ }
    $results = @()
    foreach ($line in $text) {
        if ($line -match 'FUZZ-TARGET name=(\S+) iters=(\d+) accepted=(\d+) refused=(\d+)(.*?) secs=([\d.]+)') {
            $results += [pscustomobject]@{
                Target   = $Matches[1]
                Iters    = [long]$Matches[2]
                Accepted = [long]$Matches[3]
                Refused  = [long]$Matches[4]
                Detail   = $Matches[5].Trim()
                Secs     = [double]$Matches[6]
            }
        }
    }
    $results | Sort-Object Target | Format-Table -AutoSize Target, Iters, Accepted, Refused, Secs, Detail | Out-String -Width 400 | Write-Host
    Write-Host "fuzz-decoders: $label pass took $secs s (exit $code, log $log)"

    if ($code -eq 3) { Fail "$label pass: a target hung (see $log)" }
    if ($code -ne 0) { Fail "$label pass: a target failed (exit $code; see $log)" }
    if ($results.Count -ne $expected) { Fail "$label pass reported $($results.Count) of $expected targets" }
    $short = @($results | Where-Object { $_.Iters -lt $iters })
    if ($short.Count -gt 0) { Fail "$label pass: $($short.Count) target(s) ran fewer than $iters iterations" }
    return , $results
}

$release = Invoke-Pass 'release' @('--release') $Iterations
$n = @($release).Count
if ($SkipDebug) {
    Write-Output "FUZZ-DECODERS: OK $n decoders x $Iterations (release; debug pass skipped)"
    exit 0
}
$debug = Invoke-Pass 'debug' @() $DebugIterations
Write-Output "FUZZ-DECODERS: OK $n decoders x $Iterations (release) + $(@($debug).Count) x $DebugIterations (debug, overflow checks)"
exit 0
