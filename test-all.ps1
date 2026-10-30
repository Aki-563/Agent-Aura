# Runs everything: unit tests, then the desktop conformance suite in an isolated Chrome.
# Usage (from the repo root):  .\test-all.ps1            # unit tests + all conformance groups
#                              .\test-all.ps1 -Repeat 3   # run the conformance suite 3 times
# The desktop suite DRIVES YOUR REAL SCREEN (a separate Chrome, Notepad, Calculator). Do not use the
# mouse or keyboard while it runs; Esc stops it. Measurements go to
# %LOCALAPPDATA%\AgentAura\testlogs\<date>\*.jsonl (no report is printed on purpose).
param([int]$Repeat = 1)
$ErrorActionPreference = "Continue"
$env:AURA_MUTE = "1"
Push-Location "$PSScriptRoot\src-tauri"
try {
    cargo test --lib 2>&1 | Out-Null
    $unit = $LASTEXITCODE
    cargo build --release --bin aura-cli 2>&1 | Out-Null
    $failed = 0
    for ($i = 1; $i -le $Repeat; $i++) {
        $output = & .\target\release\aura-cli.exe conformance --browser chrome 2>&1
        if ($LASTEXITCODE -ne 0) { $failed++; $output | Where-Object { $_ -match "^FAIL" } | ForEach-Object { Write-Host $_ } }
    }
    if ($unit -eq 0 -and $failed -eq 0) { Write-Host "ALL PASSED (unit tests, and conformance x$Repeat). Logs: $env:LOCALAPPDATA\AgentAura\testlogs" ; exit 0 }
    Write-Host "NOT PASSING: unit exit code $unit, conformance rounds failed $failed of $Repeat. Fix the harness and re-run."
    exit 1
} finally { Pop-Location }
