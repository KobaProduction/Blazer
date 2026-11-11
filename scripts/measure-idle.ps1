param(
    [Parameter(Mandatory)][string]$ExecutablePath,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedSha256,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$SourceRelation = 'Exact source revision is unverified',
    [ValidateRange(1, 60)][int]$DurationSeconds = 10,
    [ValidateRange(0, 10)][int]$WarmupSeconds = 2,
    [ValidateRange(1, 10)][int]$SampleIntervalSeconds = 1
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-OwnedProcessSample {
    param([System.Diagnostics.Process]$OwnedProcess, [string]$ExpectedPath)
    $OwnedProcess.Refresh()
    if ($OwnedProcess.HasExited) { throw 'The launched process exited before measurement completed' }
    $actualPath = [System.IO.Path]::GetFullPath($OwnedProcess.Path)
    if (-not [string]::Equals($actualPath, $ExpectedPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The launched process path does not match the verified measurement copy'
    }
    [pscustomobject]@{
        TimestampUtc = [DateTime]::UtcNow.ToString('o')
        CpuMilliseconds = $OwnedProcess.TotalProcessorTime.TotalMilliseconds
        PrivateBytes = $OwnedProcess.PrivateMemorySize64
        WorkingSetBytes = $OwnedProcess.WorkingSet64
    }
}

$sourcePath = (Resolve-Path -LiteralPath $ExecutablePath).Path
if ([System.IO.Path]::GetExtension($sourcePath) -ne '.exe') { throw 'An explicitly selected Windows executable is required' }
$sourceHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash
if ($sourceHash -ne $ExpectedSha256) { throw 'Source executable SHA256 does not match the expected build identity' }

$runName = 'hidden-idle-' + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + [guid]::NewGuid().ToString('N')
$runDirectory = Join-Path ([System.IO.Path]::GetFullPath($OutputDirectory)) $runName
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
$copyPath = Join-Path $runDirectory ([System.IO.Path]::GetFileName($sourcePath))
Copy-Item -LiteralPath $sourcePath -Destination $copyPath
$copyHash = (Get-FileHash -LiteralPath $copyPath -Algorithm SHA256).Hash
if ($copyHash -ne $sourceHash) { throw 'Measurement copy SHA256 does not match the source' }

$result = [ordered]@{
    SchemaVersion = 1
    Workload = 'hidden-idle-requested'
    VisibilityVerified = $false
    SourceExecutable = $sourcePath
    SourceSha256 = $sourceHash
    MeasurementExecutable = $copyPath
    MeasurementSha256 = $copyHash
    SourceRelation = $SourceRelation
    RequestedWindowStyle = 'Hidden'
    RequestedWarmupSeconds = $WarmupSeconds
    RequestedDurationSeconds = $DurationSeconds
    SampleIntervalSeconds = $SampleIntervalSeconds
    Outcome = 'UNVERIFIED'
    OwnedProcessId = $null
    CleanupVerified = $false
    EnergyMeasured = $false
    Failure = $null
}
$ownedProcess = $null
$ownedStartTicks = $null
$failure = $null
$samples = [System.Collections.Generic.List[object]]::new()
try {
    $ownedProcess = Start-Process -FilePath $copyPath -WorkingDirectory $runDirectory -WindowStyle Hidden -PassThru
    $ownedStartTicks = $ownedProcess.StartTime.ToUniversalTime().Ticks
    $result.OwnedProcessId = $ownedProcess.Id
    if ($WarmupSeconds -gt 0) { Start-Sleep -Seconds $WarmupSeconds }

    $initial = Get-OwnedProcessSample -OwnedProcess $ownedProcess -ExpectedPath $copyPath
    $samples.Add($initial)
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    while ($clock.Elapsed.TotalSeconds -lt $DurationSeconds) {
        $remainingMilliseconds = [Math]::Ceiling(($DurationSeconds - $clock.Elapsed.TotalSeconds) * 1000)
        $waitMilliseconds = [Math]::Min($SampleIntervalSeconds * 1000, [Math]::Max(1, $remainingMilliseconds))
        Start-Sleep -Milliseconds $waitMilliseconds
        $samples.Add((Get-OwnedProcessSample -OwnedProcess $ownedProcess -ExpectedPath $copyPath))
    }
    $clock.Stop()
    $final = $samples[$samples.Count - 1]
    $cpuDeltaMilliseconds = $final.CpuMilliseconds - $initial.CpuMilliseconds
    $result.MeasurementStartUtc = $initial.TimestampUtc
    $result.MeasurementEndUtc = $final.TimestampUtc
    $result.WallMilliseconds = [Math]::Round($clock.Elapsed.TotalMilliseconds, 3)
    $result.CpuDeltaMilliseconds = [Math]::Round($cpuDeltaMilliseconds, 3)
    $result.CpuPercentOfOneLogicalCore = [Math]::Round(100 * $cpuDeltaMilliseconds / $clock.Elapsed.TotalMilliseconds, 3)
    $result.PrivateLastBytes = $final.PrivateBytes
    $result.PrivateSampledPeakBytes = ($samples | Measure-Object -Property PrivateBytes -Maximum).Maximum
    $result.WorkingSetLastBytes = $final.WorkingSetBytes
    $result.WorkingSetSampledPeakBytes = ($samples | Measure-Object -Property WorkingSetBytes -Maximum).Maximum
    $result.Outcome = 'PROCESS_METRICS_OBSERVED'
}
catch {
    $failure = $_.Exception.Message
    $result.Failure = $failure
}
finally {
    if ($null -ne $ownedProcess) {
        try {
            $ownedProcess.Refresh()
            if (-not $ownedProcess.HasExited) {
                $currentPath = [System.IO.Path]::GetFullPath($ownedProcess.Path)
                $samePath = [string]::Equals($currentPath, $copyPath, [StringComparison]::OrdinalIgnoreCase)
                $sameStart = $ownedProcess.StartTime.ToUniversalTime().Ticks -eq $ownedStartTicks
                if (-not ($samePath -and $sameStart)) { throw 'Cleanup identity check failed; no process was stopped' }
                Stop-Process -InputObject $ownedProcess
                if (-not $ownedProcess.WaitForExit(5000)) {
                    throw 'The owned process did not exit within the cleanup timeout'
                }
            }
            $result.CleanupVerified = $ownedProcess.HasExited
        }
        catch {
            $result.CleanupFailure = $_.Exception.Message
            $failure = $_.Exception.Message
        }
        $ownedProcess.Dispose()
    }
    $result.Samples = @($samples.ToArray())
    $reportPath = Join-Path $runDirectory 'measurement.json'
    $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $reportPath -Encoding utf8
    Write-Output ('Measurement report: ' + $reportPath)
}
if ($null -ne $failure) { throw $failure }
