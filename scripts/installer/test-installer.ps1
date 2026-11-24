#Requires -Version 5.1
[CmdletBinding()]
param([string[]]$Case = @())

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$env:PSModulePath = [IO.Path]::Combine($PSHOME,'Modules')
foreach ($name in @('transaction.ps1','paths.ps1','service.ps1','provider.ps1')) {
    . (Join-Path $PSScriptRoot $name)
}
$script:ProductionServiceStartPolicy = ${function:Start-BlazerServiceIfRequired}
$script:ProductionRegistryPath = ${function:Assert-BlazerRegistryPath}
$script:NativeGetAcl = Get-Command -Name Get-Acl -CommandType Cmdlet

$script:FixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('BlazerInstallerTests-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($script:FixtureRoot) | Out-Null
$script:Events = [Collections.Generic.List[string]]::new()
$script:Registration = $null
$script:GuiClosed = $false

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "TEST FAILED: $Message" }
}

function Assert-Throws {
    param([scriptblock]$Action, [string]$Pattern)
    try { & $Action } catch {
        if ($_.Exception.Message -notmatch $Pattern) { throw }
        return
    }
    throw "TEST FAILED: expected failure matching $Pattern"
}

function Assert-FixturePath {
    param([string]$Path)
    $absolute = [IO.Path]::GetFullPath($Path)
    if ($absolute -ine $script:FixtureRoot -and
        -not $absolute.StartsWith($script:FixtureRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'A test adapter attempted to touch a path outside its isolated fixture.'
    }
}

# Every OS-mutating boundary is replaced. Production transaction/file operations
# still execute; no real SCM, registry, administrator ACL or Program Files access.
function Set-BlazerProtectedDirectory { param([string]$Directory) Assert-FixturePath $Directory; [IO.Directory]::CreateDirectory($Directory) | Out-Null }
function Assert-BlazerProtectedPath { param([string]$Path) Assert-FixturePath $Path }
function Set-BlazerProtectedFile { param([string]$Path) Assert-FixturePath $Path }
function Get-BlazerDirectory { Join-Path $script:FixtureRoot 'managed-program-files' }
function Get-BlazerSetupStateDirectory { Join-Path $script:FixtureRoot 'machine-state' }
function Get-BlazerShortcutPath { Join-Path $script:FixtureRoot 'start-menu/Blazer.lnk' }
function Assert-BlazerShortcut { Assert-FixturePath (Get-BlazerShortcutPath) }
function Set-BlazerShortcut { Assert-BlazerShortcut; $script:Events.Add('SetShortcut') }
function Remove-BlazerShortcut { Assert-BlazerShortcut; $script:Events.Add('RemoveShortcut') }
function Assert-BlazerRegistryPath { throw 'Unexpected native registry access in isolated harness.' }
function Set-BlazerProtectedRegistry { throw 'Unexpected native registry mutation in isolated harness.' }
function Get-CimInstance { throw 'Unexpected native CIM access in isolated harness.' }
function Invoke-CimMethod { throw 'Unexpected native CIM mutation in isolated harness.' }
function Start-Service { throw 'Unexpected native service mutation in isolated harness.' }
function Stop-Service { throw 'Unexpected native service mutation in isolated harness.' }
function Set-Service { throw 'Unexpected native service mutation in isolated harness.' }
function New-Service { throw 'Unexpected native service mutation in isolated harness.' }
function Remove-Service { throw 'Unexpected native service mutation in isolated harness.' }
function Start-Process { throw 'Unexpected native process launch in isolated harness.' }
function Stop-Process { throw 'Unexpected native process termination in isolated harness.' }
function Get-Acl { throw 'Unexpected native ACL access in isolated harness.' }
function Set-Acl { throw 'Unexpected native ACL mutation in isolated harness.' }
function New-Object { throw 'Unexpected native or COM object creation in isolated harness.' }
function Get-BlazerRegistration { $script:Registration }
function Set-BlazerRegistration { param([hashtable]$Values) $script:Registration=$Values }
function Remove-BlazerRegistration { $script:Registration=$null; $script:Events.Add('RemoveRegistration') }
function Set-BlazerUninstallRegistration { param([string]$Directory,[string]$Version) Assert-FixturePath $Directory; $script:Events.Add('UninstallRegistration') }
function Get-BlazerServiceState { $null }
function Assert-BlazerOwnedService { param($Service,[string]$Directory,[string]$OperatorSid) Assert-FixturePath $Directory }
function Stop-BlazerService { $script:Events.Add('StopService') }
function Remove-BlazerService { $script:Events.Add('DeleteService') }
function Set-BlazerService { param([string]$Directory,[string]$OperatorSid,[string]$StartMode) Assert-FixturePath $Directory; $script:Events.Add('ConfigureService') }
function Start-BlazerServiceIfRequired { param($State,[string]$Directory,[string]$OperatorSid) Assert-FixturePath $Directory; $script:Events.Add('RestoreServiceState') }

function New-CaseDirectory {
    param([string]$Name)
    $directory = Join-Path $script:FixtureRoot $Name
    Assert-FixturePath $directory
    [IO.Directory]::CreateDirectory($directory) | Out-Null
    $directory
}

function New-FakeProcess {
    param([string]$Name,[bool]$Window,[bool]$FailWait=$false)
    $process = [pscustomobject]@{ Name=$Name; HasExited=$false; MainWindowHandle=[IntPtr]([int]$Window); FailWait=$FailWait; KillCalled=$false; ObservedGui=$null }
    $process | Add-Member ScriptMethod CloseMainWindow {
        $script:Events.Add('Close:' + $this.Name)
        $script:GuiClosed=$true
        return $true
    }
    $process | Add-Member ScriptMethod WaitForExit {
        param($Timeout)
        $script:Events.Add('Wait:' + $this.Name)
        if ($this.ObservedGui -and $script:GuiClosed) { $this.ObservedGui.HasExited=$true }
        $completed=$script:GuiClosed -and -not $this.FailWait -and $Timeout -ge 0
        if ($completed) { $this.HasExited=$true }
        return $completed
    }
    $process | Add-Member ScriptMethod Kill { $this.KillCalled=$true; throw 'Forceful process termination is forbidden by this fixture.' }
    $process
}

function Test-GracefulProcessOrdering {
    $script:Events.Clear(); $script:GuiClosed=$false
    $guard=New-FakeProcess 'hidden-guard' $false
    $gui=New-FakeProcess 'gui' $true
    $outcome=@{}
    Close-BlazerProcessSet @($guard,$gui) 100 $outcome
    Assert-True (($script:Events -join ',') -ceq 'Close:gui,Wait:hidden-guard,Wait:gui') 'GUI must receive close before waiting on its hidden guard.'
    Assert-True $outcome.GuiClosed 'GUI exit is observed after bounded waits, not before them.'
    $script:GuiClosed=$false
    $guard=New-FakeProcess 'blocked-cli' $false $true
    $gui=New-FakeProcess 'gui' $true
    $guard.ObservedGui=$gui
    $outcome=@{}
    Assert-Throws { Close-BlazerProcessSet @($guard,$gui) 1 $outcome } 'No process was forcefully terminated'
    Assert-True (-not $guard.KillCalled -and -not $gui.KillCalled) 'Timeout must not call Kill.'
    Assert-True $outcome.GuiClosed 'A GUI that already exited during a guard timeout must remain eligible for relaunch.'
}

function New-Context {
    param([string]$Case)
    $root=New-CaseDirectory $Case
    $payload=Join-Path $root 'payload'
    [IO.Directory]::CreateDirectory($payload) | Out-Null
    @{ Directory=(Join-Path $root 'Blazer'); PayloadDirectory=$payload;
       Transaction=[Guid]::NewGuid().ToString('N'); OperatorSid='S-1-5-21-111-222-333-1001';
       Metadata=@{version='0.1';channel='preview';sequence=11;commit=('b'*40)};
       Payload=@{schema=1;product=$script:BlazerProduct;files=@([pscustomobject]@{path='blazer.exe';sha256=('a'*64)})} }
}

function Test-BackupAndRestore {
    $context=New-Context 'backup'
    [IO.Directory]::CreateDirectory($context.Directory) | Out-Null
    $names=@('blazer.exe','blazecore.exe','uninstall.exe','blazer-installation.json','blazer-owned-files.json')
    $hashes=@{}
    foreach ($name in $names) {
        $path=Join-Path $context.Directory $name
        [IO.File]::WriteAllText($path, ('old-' + $name))
        $hashes[$name]=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
    }
    $inventory=@{files=@($names[0..2] | ForEach-Object { [pscustomobject]@{path=$_;sha256=$hashes[$_].ToLowerInvariant()} })}
    $state=@{Registration=@{Version='0.1';ProductId=$script:BlazerProduct};Service=$null;
        Inventory=$inventory;OperatorSid=$context.OperatorSid;Operation='Install'}
    $tx=New-BlazerTransaction $context $state
    Set-BlazerTransactionProgress $tx 'files_changing'
    Assert-FixturePath $tx.JournalDirectory
    foreach ($name in $names) {
        $backup=Join-Path (Join-Path $tx.JournalDirectory 'previous') $name
        Assert-True ((Get-FileHash -LiteralPath $backup -Algorithm SHA256).Hash -ceq $hashes[$name]) "Backup missing or changed: $name"
        [IO.File]::WriteAllText((Join-Path $context.Directory $name),'changed-by-failed-install')
    }
    Restore-BlazerTransaction $tx
    foreach ($name in $names) {
        Assert-True ((Get-FileHash -LiteralPath (Join-Path $context.Directory $name) -Algorithm SHA256).Hash -ceq $hashes[$name]) "Rollback failed to restore: $name"
    }
    Assert-True (-not (Test-Path -LiteralPath $tx.JournalDirectory)) 'Successful rollback must close its journal.'
}

function Test-FreshRegistrationRollbackPreservesForeignFiles {
    $context=New-Context 'fresh-rollback'
    $state=@{Registration=$null;Service=$null;Inventory=$null;OperatorSid=$context.OperatorSid;Operation='Install'}
    $tx=New-BlazerTransaction $context $state
    Set-BlazerTransactionProgress $tx 'files_changing'
    [IO.Directory]::CreateDirectory($context.Directory) | Out-Null
    foreach ($name in @('blazer.exe','blazer-installation.json','blazer-owned-files.json','foreign.txt')) {
        [IO.File]::WriteAllText((Join-Path $context.Directory $name),$name)
    }
    Restore-BlazerTransaction $tx
    Assert-True ([IO.File]::Exists((Join-Path $context.Directory 'foreign.txt'))) 'Fresh rollback must preserve foreign files.'
    foreach ($name in @('blazer.exe','blazer-installation.json','blazer-owned-files.json')) {
        Assert-True (-not [IO.File]::Exists((Join-Path $context.Directory $name))) "Fresh rollback retained generated file: $name"
    }
}

function Test-EarlyCloseFailurePreservesRunningInstallation {
    $context=New-Context 'early-close-failure'
    [IO.Directory]::CreateDirectory($context.Directory) | Out-Null
    foreach($name in @('blazer.exe','blazer-installation.json','blazer-owned-files.json')) {
        [IO.File]::WriteAllText((Join-Path $context.Directory $name),('unchanged-' + $name))
    }
    $executable=Join-Path $context.Directory 'blazer.exe'
    $hash=(Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash
    $state=@{Registration=@{Version='0.1';ProductId=$script:BlazerProduct};
        Service=@{State='Running';StartMode='Auto'};OperatorSid=$context.OperatorSid;Operation='Install';
        Inventory=@{files=@([pscustomobject]@{path='blazer.exe';sha256=$hash.ToLowerInvariant()})}}
    $script:Events.Clear()
    $provider=New-OrderingProvider $script:Events 'CloseApplication'
    $script:EarlyCloseState=$state
    $script:EarlyCloseContext=$context
    $provider.Validate={param($operation) $script:EarlyCloseState}
    $provider.Prepare={
        param($saved,$operation)
        $tx=New-BlazerTransaction $script:EarlyCloseContext $saved
        $expected=Join-Path (Join-Path (Get-BlazerSetupStateDirectory) 'transactions') $script:EarlyCloseContext.Transaction
        Assert-True ($tx.JournalDirectory -ieq $expected) 'Recovery state was not created under the machine-state root.'
        Assert-True (@(Get-ChildItem -LiteralPath $script:EarlyCloseContext.Directory -Force).Count -eq 3) 'Installer state was written into the static application directory.'
        $tx
    }
    $provider.Checkpoint={param($tx,$progress) $script:Events.Add('Checkpoint:' + $progress); Set-BlazerTransactionProgress $tx $progress}
    $provider.Rollback={param($tx) Restore-BlazerTransaction $tx}
    # Simulate the still-running GUI image: copying or deleting it must fail.
    $locked=[IO.File]::Open($executable,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
    try {
        $failure=$null
        try { Invoke-BlazerInstallation $provider Install } catch { $failure=$_.Exception }
        Assert-True ($null -ne $failure) 'The injected close failure did not occur.'
        Assert-True ($failure.Message -match 'SETUP_FAILED \[CloseApplication; NoChanges\]: injected-CloseApplication') ('Unexpected early-close outcome: ' + $failure.Message)
        Assert-True ($failure.Data['SetupStage'] -ceq 'CloseApplication' -and $failure.Data['RollbackOutcome'] -ceq 'NoChanges') 'Structured failure metadata lost the stage or recovery outcome.'
        Assert-True (($script:Events -join ',') -ceq 'CloseApplication') 'An early close failure touched the service, registration, files or a mutation checkpoint.'
        Assert-True ((Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash -ceq $hash) 'Early close failure changed the running client image.'
        Assert-True ($state.Service.State -ceq 'Running') 'Early close failure changed the previous running state.'
        Assert-True (-not [IO.Directory]::Exists((Join-Path (Get-BlazerSetupStateDirectory) 'transactions'))) 'Prepared-only recovery did not finish its journal.'
    } finally { $locked.Dispose() }
}

function Test-AtomicJson {
    $directory=New-CaseDirectory 'atomic-json'
    $path=Join-Path $directory 'active.json'
    Write-BlazerJson $path @{phase='old'}
    Write-BlazerJson $path @{phase='new'}
    Assert-True ((Get-Content -Raw -LiteralPath $path | ConvertFrom-Json).phase -ceq 'new') 'Atomic JSON replacement did not commit.'
    [IO.File]::SetAttributes($path,[IO.FileAttributes]::ReadOnly)
    try {
        Assert-Throws { Write-BlazerJson $path @{phase='must-not-commit'} } 'denied|access|Unauthorized|Exception calling'
        Assert-True ((Get-Content -Raw -LiteralPath $path | ConvertFrom-Json).phase -ceq 'new') 'Failed JSON replacement destroyed previous valid JSON.'
    } finally { [IO.File]::SetAttributes($path,[IO.FileAttributes]::Normal) }
    Assert-True (@(Get-ChildItem -LiteralPath $directory -Filter '*.tmp').Count -eq 0) 'Failed atomic replacement retained a temporary file.'
}

function Test-CleanupTombstone {
    $context=New-Context 'cleanup-tombstone'
    $root=Join-Path (Get-BlazerSetupStateDirectory) 'transactions'
    [IO.Directory]::CreateDirectory($root) | Out-Null
    Write-BlazerJson (Join-Path $root 'active.json') @{transaction=$context.Transaction;cleanup=$true}
    Repair-BlazerPendingTransaction $context
    Repair-BlazerPendingTransaction $context
    Assert-True (-not [IO.Directory]::Exists($root)) 'Cleanup tombstone with absent journal must be idempotent.'
}

function New-OrderingProvider {
    param([Collections.Generic.List[string]]$Events,[string]$Failure)
    $provider=@{
        Validate={param($operation) $Events.Add('Validate'); @{Operation=$operation}}.GetNewClosure()
        Prepare={param($state,$operation) $Events.Add('Prepare'); @{JournalDirectory='isolated-fixture';Progress='prepared'}}.GetNewClosure()
        Checkpoint={param($transaction,$progress) $Events.Add('Checkpoint:' + $progress); $transaction.Progress=$progress}.GetNewClosure()
    }
    foreach ($name in @('CloseApplication','StopService','DeleteService','RemoveFiles','RemoveRegistration','ReplaceFiles','ConfigureService','RestoreServiceState','RegisterInstallation','Commit','Rollback')) {
        $stage=$name
        $provider[$name]={param($transaction) $Events.Add($stage); if ($Failure -ceq $stage) { throw ('injected-' + $stage) }}.GetNewClosure()
    }
    $provider
}

function Test-OperationOrdering {
    $events=[Collections.Generic.List[string]]::new()
    Invoke-BlazerInstallation (New-OrderingProvider $events '') Install
    $expected='Validate,Prepare,CloseApplication,Checkpoint:service_stopping,StopService,Checkpoint:files_changing,ReplaceFiles,ConfigureService,RestoreServiceState,RegisterInstallation,Commit'
    Assert-True (($events -join ',') -ceq $expected) 'Install operation ordering differs.'
    foreach ($stage in @('RestoreServiceState','RegisterInstallation','DeleteService')) {
        $events.Clear()
        $operation=if ($stage -eq 'DeleteService') {'Uninstall'} else {'Install'}
        $provider=New-OrderingProvider $events $stage
        Assert-Throws { Invoke-BlazerInstallation $provider $operation } 'SETUP_FAILED'
        Assert-True ($events[$events.Count-1] -ceq 'Rollback' -and -not $events.Contains('Commit')) "Failure at $stage did not roll back before commit."
    }
}

function Test-UpgradePolicy {
    $installed=@{version='0.1';channel='preview';sequence=10;commit=('a'*40)}
    $incoming=@{version='0.1';channel='preview';sequence=11;commit=('b'*40)}
    Assert-BlazerUpgradePolicy $installed $incoming
    $incoming.sequence=9
    Assert-Throws { Assert-BlazerUpgradePolicy $installed $incoming } 'newer|source history'
    $incoming.sequence=10
    Assert-Throws { Assert-BlazerUpgradePolicy $installed $incoming } 'newer|source history'
    $incoming.channel='stable'
    Assert-BlazerUpgradePolicy $installed $incoming
    $installed.channel='stable'; $incoming.channel='preview'; $incoming.sequence=11
    Assert-Throws { Assert-BlazerUpgradePolicy $installed $incoming } 'preview cannot replace'
    $incoming.version='0.0'
    Assert-Throws { Assert-BlazerUpgradePolicy $installed $incoming } 'newer version'
}

function Test-PriorServiceStates {
    # These local fake adapters shadow the fail-closed guards only for this
    # production-function test; no live CIM object is obtained or modified.
    function Get-CimInstance { param($ClassName,$Filter) @{Name='fixture-service'} }
    function Invoke-CimMethod { param($InputObject,$MethodName,$OperationTimeoutSec) $script:Events.Add($MethodName); @{ReturnValue=0} }
    function Wait-BlazerService { param($State) $script:Events.Add('Wait:' + $State) }
    function Set-BlazerService { param($Directory,$OperatorSid,$StartMode) Assert-FixturePath $Directory; $script:Events.Add('Mode:' + $StartMode) }
    $directory=New-CaseDirectory 'service-state-policy'
    $cases=@(
        @{Previous=@{State='Running';StartMode='Auto'};Expected='StartService,Wait:Running'},
        @{Previous=@{State='Stopped';StartMode='Manual'};Expected=''},
        @{Previous=@{State='Running';StartMode='Disabled'};Expected='Mode:Manual,StartService,Wait:Running,Mode:Disabled'},
        @{Previous=@{State='Stopped';StartMode='Disabled'};Expected=''},
        @{Previous=$null;Expected='StartService,Wait:Running'}
    )
    foreach ($case in $cases) {
        $script:Events.Clear()
        & $script:ProductionServiceStartPolicy $case.Previous $directory 'S-1-5-21-111-222-333-1001'
        Assert-True (($script:Events -join ',') -ceq $case.Expected) 'Production service-state restoration changed the previous Running/Stopped/Disabled policy.'
    }
}

function New-RegistrySecurityFixture {
    param([string]$Owner='S-1-5-32-544',[bool]$Protected=$true,
        [string]$Principal='S-1-5-32-545',[Security.AccessControl.RegistryRights]$Rights='ReadKey')
    $acl=[Security.AccessControl.RegistrySecurity]::new()
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new($Owner))
    $acl.SetAccessRuleProtection($Protected,$false)
    $acl.AddAccessRule([Security.AccessControl.RegistryAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'),'FullControl','Allow'))
    $acl.AddAccessRule([Security.AccessControl.RegistryAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new($Principal),$Rights,'Allow'))
    $acl
}

function Test-RegistrySecurityPolicy {
    foreach($owner in @('S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')){
        $acl=New-RegistrySecurityFixture -Owner $owner
        Assert-BlazerRegistrySecurity $acl $true
        Assert-BlazerRegistrySecurity $acl $false
    }
    $terminal=New-RegistrySecurityFixture -Principal 'S-1-5-13' -Rights 'SetValue,CreateSubKey,Delete'
    Assert-BlazerRegistrySecurity $terminal $false
    Assert-Throws { Assert-BlazerRegistrySecurity $terminal $true } 'unsafe access-control'
    $unprotected=New-RegistrySecurityFixture -Protected $false
    Assert-BlazerRegistrySecurity $unprotected $false
    Assert-Throws { Assert-BlazerRegistrySecurity $unprotected $true } 'protected DACL'
    $untrusted=New-RegistrySecurityFixture -Owner 'S-1-5-21-111-222-333-1001'
    Assert-Throws { Assert-BlazerRegistrySecurity $untrusted $false } 'unprivileged owner'
    Assert-Throws { Assert-BlazerRegistrySecurity $untrusted $true } 'unprivileged owner'
    foreach($rights in @([Security.AccessControl.RegistryRights]::ChangePermissions,
        [Security.AccessControl.RegistryRights]::TakeOwnership,([Enum]::ToObject([Security.AccessControl.RegistryRights],[int]0x10000000)))){
        $unsafe=New-RegistrySecurityFixture -Principal 'S-1-5-13' -Rights $rights
        Assert-Throws { Assert-BlazerRegistrySecurity $unsafe $false } 'unsafe access-control'
    }
    $genericWrite=New-RegistrySecurityFixture -Principal 'S-1-5-13' -Rights ([Enum]::ToObject([Security.AccessControl.RegistryRights],[int]0x40000000))
    Assert-BlazerRegistrySecurity $genericWrite $false
    Assert-Throws { Assert-BlazerRegistrySecurity $genericWrite $true } 'unsafe access-control'
}

function Test-NativeRegistryReadOnlyPreflight {
    Assert-True ([Environment]::Is64BitProcess) 'Native registry preflight must run in the 64-bit PS5.1 view.'
    function Get-Acl {
        param([string]$Path)
        $allowed=@(
            'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\KobaProduction\Blazer',
            'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\KobaProduction',
            'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\KobaProduction.Blazer.Windows',
            'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
            'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion',
            'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows',
            'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft'
        )
        if($Path -cnotin $allowed){throw 'Readonly preflight attempted an unexpected ACL namespace.'}
        & $script:NativeGetAcl -Path $Path
    }
    & $script:ProductionRegistryPath $script:BlazerRegistry
    & $script:ProductionRegistryPath $script:BlazerUninstallRegistry
}

try {
    $tests=@('Test-GracefulProcessOrdering','Test-BackupAndRestore','Test-FreshRegistrationRollbackPreservesForeignFiles','Test-EarlyCloseFailurePreservesRunningInstallation','Test-AtomicJson','Test-CleanupTombstone','Test-OperationOrdering','Test-UpgradePolicy','Test-PriorServiceStates','Test-RegistrySecurityPolicy','Test-NativeRegistryReadOnlyPreflight')
    if(@($Case | Where-Object { $_ -notin $tests }).Count){throw 'Unknown installer acceptance case.'}
    foreach ($test in $tests) {
        if($Case.Count -gt 0 -and $test -notin $Case){continue}
        & $test
        Write-Output ($test + ': PASS')
    }
    Write-Output 'Selected installer helper acceptance PASS; no native SCM/registry mutations.'
} finally {
    $resolved=[IO.Path]::GetFullPath($script:FixtureRoot)
    $parent=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
    if ((Split-Path -Parent $resolved) -ine $parent -or
        (Split-Path -Leaf $resolved) -cnotmatch '^BlazerInstallerTests-[a-f0-9]{32}$' -or
        ((Get-Item -LiteralPath $resolved -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Refusing to remove an unexpected fixture directory.'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
