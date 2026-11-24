Set-StrictMode -Version Latest

function Invoke-BlazerInstallation {
    param([hashtable]$Provider, [ValidateSet('Install', 'Uninstall')][string]$Operation)

    $transaction = $null
    $stage = 'Validate'
    try {
        $state = & $Provider.Validate $Operation
        $stage = 'Prepare'
        $transaction = & $Provider.Prepare $state $Operation
        $stage = 'CloseApplication'
        & $Provider.CloseApplication $transaction
        $stage = 'ServiceCheckpoint'
        & $Provider.Checkpoint $transaction 'service_stopping'
        $stage = 'StopService'
        & $Provider.StopService $transaction
        $stage = 'FilesCheckpoint'
        & $Provider.Checkpoint $transaction 'files_changing'
        if ($Operation -eq 'Uninstall') {
            $stage = 'DeleteService'
            & $Provider.DeleteService $transaction
            $stage = 'RemoveFiles'
            & $Provider.RemoveFiles $transaction
            $stage = 'RemoveRegistration'
            & $Provider.RemoveRegistration $transaction
        } else {
            $stage = 'ReplaceFiles'
            & $Provider.ReplaceFiles $transaction
            $stage = 'ConfigureService'
            & $Provider.ConfigureService $transaction
            $stage = 'RestoreServiceState'
            & $Provider.RestoreServiceState $transaction
            $stage = 'RegisterInstallation'
            & $Provider.RegisterInstallation $transaction
        }
        $stage = 'Commit'
        & $Provider.Commit $transaction
    } catch {
        $failure = $_
        $outcome = 'NoChanges'
        $detail = $failure.Exception.Message
        if ($transaction) {
            try {
                & $Provider.Rollback $transaction
                if ($transaction.Progress -ne 'prepared') { $outcome = 'Restored' }
            } catch {
                $outcome = 'Failed'
                $detail += '; recovery: ' + $_.Exception.Message
            }
        }
        $setupFailure = [InvalidOperationException]::new("SETUP_FAILED [$stage; $outcome]: $detail", $failure.Exception)
        $setupFailure.Data['SetupStage'] = $stage
        $setupFailure.Data['RollbackOutcome'] = $outcome
        if ($transaction) { $setupFailure.Data['RecoveryDirectory'] = $transaction.JournalDirectory }
        throw $setupFailure
    }
}
