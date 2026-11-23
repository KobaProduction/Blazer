#requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Install','Uninstall','Recover')][string]$Operation,
    [string]$PayloadDirectory,
    [string]$OperatorSid,
    [int]$BootstrapPid,
    [string]$SetupPath,
    [string]$Transaction
)
$env:PSModulePath = [IO.Path]::Combine($PSHOME, 'Modules')
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
foreach ($name in @('transaction.ps1','paths.ps1','service.ps1','provider.ps1')) { . (Join-Path $PSScriptRoot $name) }

function Assert-BlazerSetupCaller {
    if (-not [Environment]::Is64BitProcess) { throw 'Installer maintenance requires 64-bit Windows PowerShell.' }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not [Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Installer maintenance requires administrator privileges.'
    }
    if ($Operation -eq 'Recover') { return }
    if ($OperatorSid -cnotmatch '^S-1-5-21-(?:[0-9]+-){3}[0-9]+$' -or $OperatorSid.Length -gt 184 -or
        $Transaction -cnotmatch '^[a-f0-9]{32}$' -or $BootstrapPid -le 0) { throw 'Invalid installer identity.' }
    [void][Security.Principal.SecurityIdentifier]::new($OperatorSid)
    $parent = Get-CimInstance Win32_Process -Filter "ProcessId=$BootstrapPid"
    if (-not $parent -or $parent.ExecutablePath -ine $SetupPath -or
        $parent.SessionId -ne [Diagnostics.Process]::GetCurrentProcess().SessionId) { throw 'The original installer session is no longer valid.' }
    $owner = Invoke-CimMethod -InputObject $parent -MethodName GetOwnerSid
    if ($owner.ReturnValue -ne 0 -or $owner.Sid -ine $OperatorSid) { throw 'The pre-elevation Windows identity did not match.' }
}

function Read-BlazerIncomingPayload {
    $payload = Read-BlazerInventory (Join-Path $PayloadDirectory 'payload.json')
    if ($payload.target -cne 'windows-x86_64' -or $payload.version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or
        $payload.channel -cnotin @('preview','stable') -or $payload.commit -cnotmatch '^[a-f0-9]{40}$' -or
        $payload.sequence -lt 1 -or $payload.sequence -gt 2147483647) { throw 'Invalid setup release metadata.' }
    $uninstaller = Join-Path $PayloadDirectory 'uninstall.exe'
    if (-not [IO.File]::Exists($uninstaller)) { throw 'The embedded uninstaller is missing.' }
    $payload.files += [pscustomobject]@{ path='uninstall.exe'; sha256=(Get-FileHash -LiteralPath $uninstaller -Algorithm SHA256).Hash.ToLowerInvariant() }
    $payload
}

$mutex = $null
$locked = $false
try {
    Assert-BlazerSetupCaller
    $mutex = [Threading.Mutex]::new($false, 'Global\KobaProduction.Blazer.Setup')
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (-not $locked) { throw 'Another Blazer installation or removal is already running.' }
    $context = @{ Directory=(Get-BlazerDirectory); OperatorSid=$OperatorSid; Transaction=$Transaction;
        PayloadDirectory=$PayloadDirectory; Payload=$null; Metadata=$null }
    Repair-BlazerPendingTransaction $context
    if ($Operation -eq 'Recover') { Write-Output 'SETUP_RECOVERY_COMPLETE'; exit 0 }
    if ($Operation -eq 'Install') { $context.Payload=Read-BlazerIncomingPayload; $context.Metadata=$context.Payload }
    Invoke-BlazerInstallation (New-BlazerWindowsProvider $context) $Operation
    Write-Output 'SETUP_COMPLETE'
    exit 0
} catch {
    Write-Error $_ -ErrorAction Continue
    exit 1
} finally {
    if ($locked) { $mutex.ReleaseMutex() }
    if ($mutex) { $mutex.Dispose() }
}
