Set-StrictMode -Version Latest

function Read-BlazerInventory {
    param([string]$Path)
    $inventory = Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
    if ($inventory.schema -ne 1 -or $inventory.product -cne $script:BlazerProduct -or
        @($inventory.files).Count -lt 1 -or @($inventory.files).Count -gt 128) { throw 'Invalid owned file inventory.' }
    $seen = @{}
    foreach ($file in $inventory.files) {
        Assert-BlazerFileName $file.path
        if ($seen.ContainsKey($file.path.ToLowerInvariant()) -or $file.sha256 -cnotmatch '^[a-f0-9]{64}$') {
            throw 'Invalid or duplicated inventory entry.'
        }
        $seen[$file.path.ToLowerInvariant()] = $true
    }
    $inventory
}

function Assert-BlazerInventoryFiles {
    param([string]$Directory, $Inventory, [switch]$Protected)
    foreach ($file in $Inventory.files) {
        $path = Assert-BlazerRegularPath (Join-Path $Directory $file.path) $Directory
        if (-not [IO.File]::Exists($path) -or
            (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ine $file.sha256) {
            throw "Owned payload digest mismatch: $($file.path)"
        }
        if ($Protected) { Assert-BlazerProtectedPath $path }
    }
}

function Get-BlazerValidatedState {
    param([hashtable]$Context, [string]$Operation)
    $directory = $Context.Directory
    [void](Assert-BlazerRegularPath $directory (Split-Path -Parent $directory))
    Assert-BlazerProtectedPath (Split-Path -Parent $directory)
    Assert-BlazerShortcut
    $registration = Get-BlazerRegistration
    $service = Get-BlazerServiceState
    $inventory = $null
    if ($registration) {
        if ($registration.ProductId -cne $script:BlazerProduct -or $registration.InstallDirectory -ine $directory) {
            throw 'The registered Blazer installation is not owned by this product.'
        }
        Assert-BlazerProtectedPath $directory
        $identityPath = Join-Path $directory 'blazer-installation.json'
        Assert-BlazerProtectedPath $identityPath
        $identity = Get-Content -Raw -LiteralPath $identityPath | ConvertFrom-Json
        if ($identity.product -cne $script:BlazerProduct -or $identity.schema -ne 1 -or
            $identity.commit -cne $registration.Commit -or $identity.sequence -ne $registration.Sequence) {
            throw 'The installation identity does not match its protected registration.'
        }
        $inventoryPath = Join-Path $directory 'blazer-owned-files.json'
        Assert-BlazerProtectedPath $inventoryPath
        $inventory = Read-BlazerInventory $inventoryPath
        Assert-BlazerInventoryFiles $directory $inventory -Protected
        if ($Operation -eq 'Install' -and $registration.OperatorSid -ine $Context.OperatorSid) {
            throw 'This Blazer installation belongs to another Windows user.'
        }
        if ($Operation -eq 'Install') { Assert-BlazerUpgradePolicy $identity $Context.Metadata }
    } elseif (Test-Path -LiteralPath $directory) {
        throw 'The target folder already contains files not owned by this installer.'
    }
    if ($Operation -eq 'Uninstall' -and -not $registration) { throw 'No owned Blazer installation is registered.' }
    if ($service -and -not $registration) { throw 'An independently installed BlazeCoreTun service already exists.' }
    $operator = if ($registration) { $registration.OperatorSid } else { $Context.OperatorSid }
    Assert-BlazerOwnedService $service $directory $operator
    if ($Operation -eq 'Install') {
        Assert-BlazerInventoryFiles $Context.PayloadDirectory $Context.Payload
        $oldNames = if ($inventory) { @($inventory.files | ForEach-Object { $_.path }) } else { @() }
        foreach ($file in $Context.Payload.files) {
            if ((Test-Path -LiteralPath (Join-Path $directory $file.path)) -and $file.path -notin $oldNames) {
                throw "The incoming file would replace an unowned file: $($file.path)"
            }
        }
    }
    @{ Registration=$registration; Service=$service; Inventory=$inventory; OperatorSid=$operator; Operation=$Operation }
}

function Copy-BlazerOwnedFiles {
    param([string]$Source, [string]$Destination, [string[]]$Names)
    foreach ($name in $Names) {
        Assert-BlazerFileName $name
        $from = Assert-BlazerRegularPath (Join-Path $Source $name) $Source
        $to = Assert-BlazerRegularPath (Join-Path $Destination $name) $Destination
        [IO.Directory]::CreateDirectory((Split-Path -Parent $to)) | Out-Null
        [IO.File]::Copy($from, $to, $true)
    }
}

function Remove-BlazerOwnedFiles {
    param([string]$Directory, [string[]]$Names)
    foreach ($name in $Names) {
        Assert-BlazerFileName $name
        $path = Assert-BlazerRegularPath (Join-Path $Directory $name) $Directory
        if ([IO.File]::Exists($path)) { [IO.File]::Delete($path) }
    }
    $licenses = Join-Path $Directory 'licenses'
    if ([IO.Directory]::Exists($licenses) -and -not @(Get-ChildItem -LiteralPath $licenses -Force).Count) {
        [IO.Directory]::Delete($licenses)
    }
}

function New-BlazerTransaction {
    param([hashtable]$Context, [hashtable]$State)
    $journalRoot = Join-Path (Split-Path -Parent $Context.Directory) 'Blazer.Setup'
    [void](Assert-BlazerRegularPath $journalRoot (Split-Path -Parent $journalRoot))
    if ([IO.Directory]::Exists($journalRoot)) { Assert-BlazerProtectedPath $journalRoot }
    Set-BlazerProtectedDirectory $journalRoot
    $journal = Join-Path $journalRoot $Context.Transaction
    if (Test-Path -LiteralPath $journal) { throw 'This setup transaction already exists.' }
    Set-BlazerProtectedDirectory $journal
    $previous = Join-Path $journal 'previous'
    Set-BlazerProtectedDirectory $previous
    $oldNames = @()
    if ($State.Inventory) {
        $oldNames = @($State.Inventory.files | ForEach-Object { $_.path }) + @('blazer-installation.json','blazer-owned-files.json')
        Copy-BlazerOwnedFiles $Context.Directory $previous $oldNames
    }
    $incoming = if ($State.Operation -eq 'Install') { @($Context.Payload.files | ForEach-Object { $_.path }) } else { @() }
    $transaction = @{ JournalDirectory=$journal; Directory=$Context.Directory; State=$State;
        OldNames=$oldNames; Incoming=$incoming; Transaction=$Context.Transaction; Context=$Context }
    $saved = @{ schema=1; phase='pending'; directory=$Context.Directory; transaction=$Context.Transaction;
        state=$State; old_names=$oldNames; incoming=$incoming }
    Write-BlazerJson (Join-Path $journal 'journal.json') $saved
    foreach ($name in @('maintenance.ps1','transaction.ps1','paths.ps1','service.ps1','provider.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $journal $name)
    }
    Write-BlazerJson (Join-Path $journalRoot 'active.json') @{ transaction=$Context.Transaction }
    $transaction
}

function Set-BlazerIncomingFiles {
    param([hashtable]$Transaction)
    $context = $Transaction.Context
    Set-BlazerProtectedDirectory $Transaction.Directory
    $stale = @($Transaction.OldNames | Where-Object { $_ -notin $Transaction.Incoming -and $_ -notin @('blazer-installation.json','blazer-owned-files.json') })
    Remove-BlazerOwnedFiles $Transaction.Directory $stale
    Copy-BlazerOwnedFiles $context.PayloadDirectory $Transaction.Directory $Transaction.Incoming
    foreach ($name in $Transaction.Incoming) { Set-BlazerProtectedFile (Join-Path $Transaction.Directory $name) }
    Assert-BlazerInventoryFiles $Transaction.Directory $context.Payload -Protected
}

function Register-BlazerTransaction {
    param([hashtable]$Transaction)
    $metadata = $Transaction.Context.Metadata
    $identity = @{ schema=1; product=$script:BlazerProduct; target='windows-x86_64'; version=$metadata.version;
        channel=$metadata.channel; sequence=$metadata.sequence; commit=$metadata.commit }
    Write-BlazerJson (Join-Path $Transaction.Directory 'blazer-installation.json') $identity
    Write-BlazerJson (Join-Path $Transaction.Directory 'blazer-owned-files.json') $Transaction.Context.Payload
    foreach ($name in @('blazer-installation.json','blazer-owned-files.json')) {
        Set-BlazerProtectedFile (Join-Path $Transaction.Directory $name)
    }
    Set-BlazerRegistration @{ ProductId=$script:BlazerProduct; InstallDirectory=$Transaction.Directory;
        OperatorSid=$Transaction.State.OperatorSid; Version=$metadata.version; Channel=$metadata.channel;
        Sequence=[int]$metadata.sequence; Commit=$metadata.commit }
    Set-BlazerUninstallRegistration $Transaction.Directory $metadata.version
    Set-BlazerShortcut
}

function Complete-BlazerTransaction {
    param([hashtable]$Transaction)
    $journalRoot = Split-Path -Parent $Transaction.JournalDirectory
    [void](Assert-BlazerRegularPath $Transaction.JournalDirectory $journalRoot)
    # This durable tombstone makes a crash between directory and pointer removal
    # distinguishable from a missing, still-needed rollback journal.
    Write-BlazerJson (Join-Path $journalRoot 'active.json') @{ transaction=$Transaction.Transaction; cleanup=$true }
    if ([IO.Directory]::Exists($Transaction.JournalDirectory)) {
        Remove-Item -LiteralPath $Transaction.JournalDirectory -Recurse -Force
    }
    [IO.File]::Delete((Join-Path $journalRoot 'active.json'))
    if (-not @(Get-ChildItem -LiteralPath $journalRoot -Force).Count) { [IO.Directory]::Delete($journalRoot) }
}

function Commit-BlazerTransaction {
    param([hashtable]$Transaction)
    $path = Join-Path $Transaction.JournalDirectory 'journal.json'
    $journal = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
    $journal.phase = 'committed'
    Write-BlazerJson $path $journal
    $registration = Get-BlazerRegistration
    if ($registration) {
        $registration['LastSetupTransaction']=$Transaction.Transaction
        Set-BlazerRegistration $registration
    }
    try { Complete-BlazerTransaction $Transaction }
    catch { Write-Warning "SETUP_CLEANUP_PENDING: $($Transaction.JournalDirectory)" }
}

function Repair-BlazerPendingTransaction {
    param([hashtable]$Context)
    $root = Join-Path (Split-Path -Parent $Context.Directory) 'Blazer.Setup'
    $active = Join-Path $root 'active.json'
    if (-not [IO.File]::Exists($active)) { return }
    [void](Assert-BlazerRegularPath $active (Split-Path -Parent $root))
    Assert-BlazerProtectedPath $root
    Assert-BlazerProtectedPath $active
    $pointer = Get-Content -Raw -LiteralPath $active | ConvertFrom-Json
    if ($pointer.transaction -cnotmatch '^[a-f0-9]{32}$') { throw 'Invalid pending setup transaction.' }
    $directory = Join-Path $root $pointer.transaction
    if ($pointer.PSObject.Properties['cleanup'] -and $pointer.cleanup -eq $true) {
        Complete-BlazerTransaction @{ JournalDirectory=$directory; Transaction=$pointer.transaction }
        return
    }
    [void](Assert-BlazerRegularPath $directory $root)
    Assert-BlazerProtectedPath $directory
    Assert-BlazerProtectedPath (Join-Path $directory 'journal.json')
    $saved = Get-Content -Raw -LiteralPath (Join-Path $directory 'journal.json') | ConvertFrom-Json
    if ($saved.schema -ne 1 -or $saved.directory -ine $Context.Directory -or
        $saved.transaction -cne $pointer.transaction) { throw 'Invalid setup recovery journal.' }
    $state = @{}
    foreach ($property in $saved.state.PSObject.Properties) { $state[$property.Name]=$property.Value }
    if ($state.Registration) {
        $registration = @{}
        foreach ($property in $state.Registration.PSObject.Properties) { $registration[$property.Name]=$property.Value }
        $state.Registration=$registration
    }
    $transaction = @{ JournalDirectory=$directory; Directory=$Context.Directory; State=$state;
        OldNames=@($saved.old_names); Incoming=@($saved.incoming); Transaction=$saved.transaction; Context=$Context }
    if ($saved.phase -eq 'committed') { Complete-BlazerTransaction $transaction }
    elseif ($saved.phase -eq 'pending') { Restore-BlazerTransaction $transaction }
    else { throw 'Unknown setup recovery phase.' }
}

function Restore-BlazerTransaction {
    param([hashtable]$Transaction)
    $state = $Transaction.State
    $currentRegistration = Get-BlazerRegistration
    $guiClosed = $currentRegistration -and $currentRegistration.ContainsKey('LastClosedTransaction') -and
        $currentRegistration.LastClosedTransaction -ceq $Transaction.Transaction
    Assert-BlazerOwnedService (Get-BlazerServiceState) $Transaction.Directory $state.OperatorSid
    Stop-BlazerService
    $newOnly = @($Transaction.Incoming | Where-Object { $_ -notin $Transaction.OldNames })
    Remove-BlazerOwnedFiles $Transaction.Directory $newOnly
    if ($state.Registration) {
        Set-BlazerProtectedDirectory $Transaction.Directory
        Copy-BlazerOwnedFiles (Join-Path $Transaction.JournalDirectory 'previous') $Transaction.Directory $Transaction.OldNames
        foreach ($name in $Transaction.OldNames) { Set-BlazerProtectedFile (Join-Path $Transaction.Directory $name) }
        $registration = @{}; foreach ($property in $state.Registration.Keys) { $registration[$property]=$state.Registration[$property] }
        if ($guiClosed) { $registration['LastClosedTransaction']=$Transaction.Transaction }
        Set-BlazerRegistration $registration
        Set-BlazerUninstallRegistration $Transaction.Directory $registration.Version
        Set-BlazerShortcut
    } else {
        Remove-BlazerOwnedFiles $Transaction.Directory @('blazer-installation.json','blazer-owned-files.json')
        Remove-BlazerRegistration
        if ([IO.Directory]::Exists($Transaction.Directory) -and -not @(Get-ChildItem -LiteralPath $Transaction.Directory -Force).Count) {
            [IO.Directory]::Delete($Transaction.Directory)
        }
    }
    if ($state.Service) {
        Set-BlazerService $Transaction.Directory $state.OperatorSid $state.Service.StartMode
        Start-BlazerServiceIfRequired $state.Service $Transaction.Directory $state.OperatorSid
    } else { Remove-BlazerService }
    if ($state.Registration) {
        $registration = Get-BlazerRegistration
        $registration['LastRollbackTransaction']=$Transaction.Transaction
        Set-BlazerRegistration $registration
    }
    Complete-BlazerTransaction $Transaction
}

function Assert-BlazerUpgradePolicy {
    param($Installed, $Incoming)
    $old = [version]($Installed.version + '.0.0')
    $new = [version]($Incoming.version + '.0.0')
    if ($new -lt $old) { throw 'A newer version of Blazer is already installed.' }
    if ($new -gt $old) { return }
    if ($Installed.channel -eq 'stable' -and $Incoming.channel -ne 'stable') {
        throw 'A preview cannot replace the same stable version.'
    }
    if ($Installed.channel -eq 'preview' -and $Incoming.channel -eq 'preview' -and
        ($Incoming.sequence -lt $Installed.sequence -or
        ($Incoming.sequence -eq $Installed.sequence -and $Incoming.commit -cne $Installed.commit))) {
        throw 'The installed preview is newer or belongs to another source history.'
    }
}

function New-BlazerWindowsProvider {
    param([hashtable]$Context)
    @{
        Validate = { param($operation) Get-BlazerValidatedState $Context $operation }.GetNewClosure()
        Prepare = { param($state, $operation) New-BlazerTransaction $Context $state }.GetNewClosure()
        CloseApplication = { param($tx) Close-BlazerApplications $tx.Directory $tx.State.OperatorSid $tx.Transaction }
        StopService = { param($tx) Stop-BlazerService }
        DeleteService = { param($tx) Remove-BlazerService }
        RemoveFiles = { param($tx) Remove-BlazerOwnedFiles $tx.Directory $tx.OldNames }
        RemoveRegistration = { param($tx) Remove-BlazerRegistration }
        ReplaceFiles = { param($tx) Set-BlazerIncomingFiles $tx }
        ConfigureService = { param($tx)
            $mode = if ($tx.State.Service) { $tx.State.Service.StartMode } else { 'Auto' }
            Set-BlazerService $tx.Directory $tx.State.OperatorSid $mode }
        RestoreServiceState = { param($tx) Start-BlazerServiceIfRequired $tx.State.Service $tx.Directory $tx.State.OperatorSid }
        RegisterInstallation = { param($tx) Register-BlazerTransaction $tx }
        Commit = { param($tx) Commit-BlazerTransaction $tx }
        Rollback = { param($tx) Restore-BlazerTransaction $tx }
    }
}
