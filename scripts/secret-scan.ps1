# Secret scan over all git-tracked files plus staged content.
# Run before every push (operating rules §10). Exits non-zero on any hit.
Set-Location (Split-Path $PSScriptRoot -Parent)

$patterns = @(
    'AKIA[0-9A-Z]{16}',                       # AWS access key
    'gh[pousr]_[A-Za-z0-9]{20,}',             # GitHub tokens
    'sk-[A-Za-z0-9_-]{20,}',                  # generic sk- API keys
    'AIza[0-9A-Za-z_-]{35}',                  # Google API key
    '-----BEGIN[ A-Z]*PRIVATE KEY-----',      # private key material
    'xox[baprs]-[A-Za-z0-9-]{10,}',           # Slack tokens
    'eyJhbGciOi[A-Za-z0-9_-]{20,}',           # JWTs
    '(?i)(api[_-]?key|secret|password|token)["'']?\s*[:=]\s*["''][A-Za-z0-9+/_-]{16,}["'']'
)

$files = git ls-files
$hits = @()
foreach ($file in $files) {
    if (-not (Test-Path $file)) { continue }
    # Skip binary design evidence; text formats only.
    if ($file -match '\.(png|jpg|jpeg|gif|ico|zip|img|woff2?|ttf)$') { continue }
    foreach ($pattern in $patterns) {
        $matched = Select-String -Path $file -Pattern $pattern -AllMatches -ErrorAction SilentlyContinue
        if ($matched) {
            foreach ($m in $matched) {
                # Report location only - never echo the matched value.
                $hits += "$($m.Path):$($m.LineNumber) matches pattern '$pattern'"
            }
        }
    }
}

if ($hits.Count -gt 0) {
    Write-Output 'SECRET-SCAN: POTENTIAL SECRETS FOUND (values redacted):'
    $hits | ForEach-Object { Write-Output "  $_" }
    exit 1
}
Write-Output "SECRET-SCAN: clean ($($files.Count) files)"
exit 0
