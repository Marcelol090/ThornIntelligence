# Read-only process-tree telemetry for the running Tauri app (incl. WebView2 children).
# Samples CPU usage, resident/private bytes, handle/thread counts and process
# logical I/O transfer counters; never changes system performance settings.
[CmdletBinding(DefaultParameterSetName = 'ByName')]
param(
    [Parameter(ParameterSetName = 'ById', Mandatory = $true)]
    [ValidateRange(1, 2147483647)][int]$ProcessId,
    [Parameter(ParameterSetName = 'ByName')]
    [string]$ProcessName = 'thorn-intelligence',
    [ValidateRange(3, 3600)][int]$DurationSeconds = 30,
    [ValidateRange(250, 5000)][int]$IntervalMs = 1000,
    [string]$OutputDirectory = 'reports/performance'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSCmdlet.ParameterSetName -eq 'ById') {
    $rootProcess = Get-Process -Id $ProcessId -ErrorAction Stop
} else {
    $rootProcess = Get-Process -Name $ProcessName -ErrorAction Stop |
        Sort-Object StartTime -Descending | Select-Object -First 1
}
if (-not $rootProcess) { throw 'Target process unavailable.' }
$rootId = $rootProcess.Id
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $repoRoot $OutputDirectory
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss')
$output = Join-Path $OutputDirectory "app-monitor-$rootId-$stamp.csv"
$rows = [Collections.Generic.List[object]]::new()
$pastCpu = @{}
$pastTimestamp = $null
$watch = [Diagnostics.Stopwatch]::StartNew()

while ($watch.Elapsed.TotalSeconds -lt $DurationSeconds) {
    $now = Get-Date
    $tree = [Collections.Generic.HashSet[int]]::new()
    [void]$tree.Add($rootId)
    # Discover WebView2 and other descendants at each sample.
    $procs = @(Get-CimInstance Win32_Process -ErrorAction Stop |
        Select-Object ProcessId, ParentProcessId, ReadTransferCount, WriteTransferCount)
    do {
        $priorCount = $tree.Count
        foreach ($item in $procs) {
            if ($tree.Contains([int]$item.ParentProcessId)) {
                [void]$tree.Add([int]$item.ProcessId)
            }
        }
    } while ($tree.Count -gt $priorCount)
    $totalCpuSeconds = 0.0
    $totalWorkingSet = [long]0
    $totalPrivate = [long]0
    $handles = 0
    $threads = 0
    $readTransfers = [double]0
    $writeTransfers = [double]0
    $alive = 0
    foreach ($id in $tree) {
        $proc = Get-Process -Id $id -ErrorAction SilentlyContinue
        if (-not $proc) { continue }
        $alive++
        $cpu = if ($null -ne $proc.CPU) { [double]$proc.CPU } else { 0.0 }
        $totalCpuSeconds += $cpu
        $totalWorkingSet += [long]$proc.WorkingSet64
        $totalPrivate += [long]$proc.PrivateMemorySize64
        $handles += [int]$proc.HandleCount
        $threads += [int]$proc.Threads.Count
        $counter = $procs | Where-Object ProcessId -eq $id | Select-Object -First 1
        if ($counter) {
            $readTransfers += [double]$counter.ReadTransferCount
            $writeTransfers += [double]$counter.WriteTransferCount
        }
    }
    $corePercent = 0.0
    if ($null -ne $pastTimestamp) {
        $elapsed = ($now - $pastTimestamp).TotalSeconds
        if ($elapsed -gt 0) {
            $corePercent = [Math]::Max(0, 100.0 * ($totalCpuSeconds - $pastCpu.Total) / $elapsed)
        }
    }
    $pastTimestamp = $now
    $pastCpu = @{ Total = $totalCpuSeconds }
    $rows.Add([pscustomobject]@{
        timestamp_utc = $now.ToUniversalTime().ToString('o')
        root_pid = $rootId
        process_count = $alive
        cpu_percent_of_one_core_approx = [Math]::Round($corePercent, 2)
        working_set_bytes = $totalWorkingSet
        private_bytes = $totalPrivate
        handles = $handles
        threads = $threads
        logical_read_transfer_bytes = [long]$readTransfers
        logical_write_transfer_bytes = [long]$writeTransfers
    })
    if (-not (Get-Process -Id $rootId -ErrorAction SilentlyContinue)) { break }
    Start-Sleep -Milliseconds $IntervalMs
}
$rows | Export-Csv -LiteralPath $output -NoTypeInformation -Encoding utf8
Write-Host "Observed process tree: $rootId ($($rows.Count) samples)" -ForegroundColor Green
Write-Host "CSV: $output"
Write-Warning 'CPU percent is unnormalized core-equivalent; I/O counters are logical, not physical disk reads. Child process churn can skew CPU deltas.'
