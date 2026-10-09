# Local-only Rust performance laboratory: produces measured JSON and raw logs.
# Run in PowerShell 7 on Windows from any working directory.
[CmdletBinding()]
param(
    [ValidateRange(100, 120000)][int]$Files = 1500,
    [ValidateRange(2, 30)][int]$Repeats = 5,
    [string]$OutputDirectory = 'reports/performance'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$cargo = Get-Command 'cargo.exe' -ErrorAction SilentlyContinue
if (-not $cargo) { throw 'Rust/Cargo not installed or missing from PATH.' }

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss')
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $root $OutputDirectory
}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$stdout = Join-Path $OutputDirectory "thorn-$stamp.stdout.log"
$stderr = Join-Path $OutputDirectory "thorn-$stamp.stderr.log"
$report = Join-Path $OutputDirectory "thorn-$stamp.json"

$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
$os = (Get-CimInstance Win32_OperatingSystem | Select-Object -ExpandProperty Caption)
$env:THORN_PERF_FILES = [string]$Files
$env:THORN_PERF_REPEAT = [string]$Repeats
$arguments = @('test', '--release', '--manifest-path', 'src-tauri/Cargo.toml',
               'perf_kit_', '--', '--ignored', '--nocapture', '--test-threads=1')
Write-Host "Running $Files synthetic files x $Repeats benchmark repetitions" -ForegroundColor Cyan
Write-Host 'Compilation and fixture generation are excluded from Rust scenario timings.'
$runner = $null
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$peakWorkingSet = [long]0
$sampleCount = 0
try {
    $runner = Start-Process -FilePath $cargo.Source -ArgumentList $arguments -WorkingDirectory $root -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    do {
        # Only direct child native benchmark processes. Build times are not Rust latency.
        $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($runner.Id)" -ErrorAction SilentlyContinue)
        foreach ($child in $children) {
            if ($child.Name -notmatch '^thorn_intelligence') { continue }
            $native = Get-Process -Id $child.ProcessId -ErrorAction SilentlyContinue
            if ($null -ne $native) {
                $sampleCount++
                $peakWorkingSet = [Math]::Max($peakWorkingSet, [long]$native.WorkingSet64)
            }
        }
        Start-Sleep -Milliseconds 250
        $runner.Refresh()
    } while (-not $runner.HasExited)
    $runner.WaitForExit()
}
finally {
    $stopwatch.Stop()
    Remove-Item Env:\THORN_PERF_FILES -ErrorAction SilentlyContinue
    Remove-Item Env:\THORN_PERF_REPEAT -ErrorAction SilentlyContinue
}
$exitCode = $runner.ExitCode
$raw = Get-Content -LiteralPath $stdout -Raw -ErrorAction Stop
$metrics = [Collections.Generic.List[object]]::new()
foreach ($hit in [regex]::Matches($raw, 'THORN_PERF_METRIC[ \t]+(\{[^\r\n]+\})')) {
    $metrics.Add(($hit.Groups[1].Value | ConvertFrom-Json -AsHashtable))
}
$storage = [Collections.Generic.List[object]]::new()
foreach ($hit in [regex]::Matches($raw, 'THORN_PERF_STORAGE[ \t]+(\{[^\r\n]+\})')) {
    $storage.Add(($hit.Groups[1].Value | ConvertFrom-Json -AsHashtable))
}
$data = [ordered]@{
    schema = 1
    created_utc = (Get-Date).ToUniversalTime().ToString('o')
    environment = [ordered]@{
        machine = $env:COMPUTERNAME
        os = $os
        cpu = $cpu.Trim()
        rustc = ((& rustc --version) -join ' ')
        profile = 'release'
        files = $Files
        repetitions = $Repeats
    }
    metrics = @($metrics.ToArray())
    storage = @($storage.ToArray())
    monitor = [ordered]@{
        wall_seconds_including_build_and_fixture = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 3)
        sampled_native_peak_working_set_bytes = $peakWorkingSet
        native_process_samples = $sampleCount
        sample_period_ms = 250
        note = 'Sampled working set is approximate; file I/O bytes come from scanner reports, not physical disk counters.'
    }
    runner_exit_code = $exitCode
}
$data | ConvertTo-Json -Depth 14 | Set-Content -LiteralPath $report -Encoding utf8
Write-Host "Report: $report" -ForegroundColor Green
Write-Host "Logs: $stdout ; $stderr"
if ($exitCode -ne 0) {
    throw "cargo test failed with exit code $exitCode; inspect stderr and stdout logs."
}
if ($metrics.Count -lt 7) {
    throw "Expected >= 7 performance scenarios, captured $($metrics.Count). Benchmark report retained."
}
Write-Host 'Baseline or candidate ready for Pareto comparison.' -ForegroundColor Green
