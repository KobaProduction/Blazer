Set-StrictMode -Version Latest
$script:BlazerServiceName = 'BlazeCoreTun'

function Get-BlazerServiceState {
    $service = Get-CimInstance Win32_Service -Filter "Name='BlazeCoreTun'"
    if (-not $service) { return $null }
    @{ PathName=$service.PathName; StartName=$service.StartName; StartMode=$service.StartMode;
        State=$service.State; ServiceType=$service.ServiceType }
}

function Assert-BlazerOwnedService {
    param($Service, [string]$Directory, [string]$OperatorSid)
    if (-not $Service) { return }
    $expected = '"{0}" windows-tun-service {1}' -f (Join-Path $Directory 'blazecore.exe'), $OperatorSid
    if ($Service.PathName -ine $expected -or $Service.StartName -ine 'LocalSystem' -or
        $Service.ServiceType -ine 'Own Process') {
        throw 'BlazeCoreTun belongs to another installation. It will not be stopped, replaced or removed.'
    }
}

function Wait-BlazerService {
    param([string]$State, [int]$Seconds = 20)
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $current = Get-BlazerServiceState
        if (($State -eq 'Absent' -and -not $current) -or ($current -and $current.State -eq $State)) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "BlazeCoreTun did not reach $State within $Seconds seconds."
}

function Stop-BlazerService {
    if (-not (Get-BlazerServiceState)) { return }
    if ((Get-BlazerServiceState).State -ne 'Stopped') {
        $service = Get-CimInstance Win32_Service -Filter "Name='BlazeCoreTun'"
        $result = Invoke-CimMethod -InputObject $service -MethodName StopService -OperationTimeoutSec 10
        if ($result.ReturnValue -ne 0) { throw "Stopping BlazeCoreTun failed: SCM result $($result.ReturnValue)." }
        Wait-BlazerService 'Stopped'
    }
}

function Remove-BlazerService {
    $service = Get-CimInstance Win32_Service -Filter "Name='BlazeCoreTun'"
    if (-not $service) { return }
    $result = Invoke-CimMethod -InputObject $service -MethodName Delete
    if ($result.ReturnValue -notin @(0,16)) { throw "Deleting BlazeCoreTun failed: SCM result $($result.ReturnValue)." }
    Wait-BlazerService 'Absent'
}

function Set-BlazerService {
    param([string]$Directory, [string]$OperatorSid, [string]$StartMode)
    $command = '"{0}" windows-tun-service {1}' -f (Join-Path $Directory 'blazecore.exe'), $OperatorSid
    $mode = if ($StartMode -eq 'Auto') { 'Automatic' } else { $StartMode }
    $service = Get-CimInstance Win32_Service -Filter "Name='BlazeCoreTun'"
    if ($service) {
        $result = Invoke-CimMethod -InputObject $service -MethodName Change -Arguments @{ PathName=$command; StartMode=$mode }
    } else {
        $result = Invoke-CimMethod -ClassName Win32_Service -MethodName Create -Arguments @{
            Name=$script:BlazerServiceName; DisplayName='Blazer VPN tunnel'; PathName=$command;
            ServiceType=[byte]16; ErrorControl=[byte]1; StartMode=$mode; StartName='LocalSystem'; DesktopInteract=$false }
    }
    if ($result.ReturnValue -ne 0) { throw "Configuring BlazeCoreTun failed: SCM result $($result.ReturnValue)." }
}

function Start-BlazerServiceIfRequired {
    param($PreviousService, [string]$Directory, [string]$OperatorSid)
    if (-not $PreviousService -or $PreviousService.State -eq 'Running') {
        $runningDisabled = $PreviousService -and $PreviousService.StartMode -eq 'Disabled'
        if ($runningDisabled) { Set-BlazerService $Directory $OperatorSid 'Manual' }
        $service = Get-CimInstance Win32_Service -Filter "Name='BlazeCoreTun'"
        $result = Invoke-CimMethod -InputObject $service -MethodName StartService -OperationTimeoutSec 10
        if ($result.ReturnValue -ne 0) { throw "Starting BlazeCoreTun failed: SCM result $($result.ReturnValue)." }
        Wait-BlazerService 'Running'
        if ($runningDisabled) { Set-BlazerService $Directory $OperatorSid 'Disabled' }
    }
}

function Close-BlazerProcessSet {
    param([object[]]$Processes, [int]$TimeoutMilliseconds = 15000, [hashtable]$Outcome = @{})
    $requested = @()
    try {
        foreach ($application in $Processes) {
            if ($application.HasExited -or $application.MainWindowHandle -eq [IntPtr]::Zero) { continue }
            if (-not $application.CloseMainWindow()) { throw 'Blazer refused the graceful shutdown request.' }
            $requested += $application
        }
        $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
        foreach ($application in $Processes) {
            $remaining = [Math]::Max(0, [int]($deadline - [DateTime]::UtcNow).TotalMilliseconds)
            if (-not $application.WaitForExit($remaining)) {
                throw 'Close the remaining Blazer CLI session before continuing. No process was forcefully terminated.'
            }
        }
    } finally { $Outcome['GuiClosed'] = @($requested | Where-Object { $_.HasExited }).Count -gt 0 }
}

function Close-BlazerApplications {
    param([string]$Directory, [string]$OperatorSid, [string]$Transaction)
    $executable = Join-Path $Directory 'blazer.exe'
    $owned = @()
    foreach ($process in (Get-CimInstance Win32_Process -Filter "Name='blazer.exe'")) {
        if (-not $process.ExecutablePath -or $process.ExecutablePath -ine $executable) { continue }
        $owner = Invoke-CimMethod -InputObject $process -MethodName GetOwnerSid
        if ($owner.ReturnValue -ne 0 -or $owner.Sid -ine $OperatorSid -or
            $process.SessionId -ne [Diagnostics.Process]::GetCurrentProcess().SessionId) {
            throw 'Close Blazer in its owning Windows session before continuing.'
        }
        $handle = [Diagnostics.Process]::GetProcessById($process.ProcessId)
        [void]$handle.Handle
        # CIM timestamps retain microseconds; Process.StartTime retains 100 ns.
        if ([Math]::Abs($handle.StartTime.ToUniversalTime().Ticks - $process.CreationDate.ToUniversalTime().Ticks) -ge 10 -or
            $handle.MainModule.FileName -ine $executable) {
            $handle.Dispose()
            throw 'The Blazer process identity changed while preparing its shutdown.'
        }
        $owned += $handle
    }
    $outcome = @{ GuiClosed=$false }
    try { Close-BlazerProcessSet $owned -Outcome $outcome }
    finally {
        if ($outcome.GuiClosed) {
            $registration = Get-BlazerRegistration
            if ($registration) { $registration['LastClosedTransaction']=$Transaction; Set-BlazerRegistration $registration }
        }
        foreach ($application in $owned) { $application.Dispose() }
    }
}
