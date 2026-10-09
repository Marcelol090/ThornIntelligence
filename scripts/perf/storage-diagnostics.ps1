# Read-only storage diagnostics. NO defrag, TRIM, format or deletion.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[A-Za-z]$')]
    [string]$DriveLetter,
    [string]$FilePath,
    [switch]$AnalyzeVolume,
    [string]$OutputDirectory = 'reports/performance'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$drive = $DriveLetter.ToUpperInvariant()
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $root $OutputDirectory
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss')
$report = Join-Path $OutputDirectory "storage-$drive-$stamp.json"
$volume = Get-Volume -DriveLetter $drive -ErrorAction Stop
$disk = @(Get-Partition -DriveLetter $drive -ErrorAction Stop | Get-Disk -ErrorAction Stop | Select-Object Number, FriendlyName, BusType, PartitionStyle, OperationalStatus)
$extents = $null
if ($FilePath) {
    $resolved = (Resolve-Path -LiteralPath $FilePath -ErrorAction Stop).Path
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw 'Extents target must be an existing file.'
    }
    # Documented read-only query. Permissions or filesystem support may vary.
    $extents = (& fsutil.exe file queryextents $resolved 2>&1 | Out-String)
}
$analysis = $null
if ($AnalyzeVolume) {
    # /A is analyze only. Never pass /D, /O, /L, or a mutating Optimize-Volume mode.
    $analysis = (& defrag.exe "$($drive):" /A /V 2>&1 | Out-String)
}
$payload = [ordered]@{
    schema = 1
    created_utc = (Get-Date).ToUniversalTime().ToString('o')
    drive = "$($drive):"
    volume = [ordered]@{
        file_system = $volume.FileSystem
        health_status = [string]$volume.HealthStatus
        operational_status = [string]$volume.OperationalStatus
        size_bytes = [long]$volume.Size
        remaining_bytes = [long]$volume.SizeRemaining
    }
    disk = $disk
    extent_file = $FilePath
    extent_result = $extents
    volume_analyze_only = [bool]$AnalyzeVolume
    analysis_output = $analysis
    warning = 'Physical extents are not SQLite free pages. SSD Windows optimization is generally TRIM; NEVER force HDD defrag on an SSD.'
}
$payload | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $report -Encoding utf8
Write-Host "Read-only storage diagnostics: $report" -ForegroundColor Green
