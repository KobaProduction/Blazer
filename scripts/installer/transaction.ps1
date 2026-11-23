Set-StrictMode -Version Latest

function Invoke-BlazerInstallation {
    param([hashtable]$Provider, [ValidateSet('Install', 'Uninstall')][string]$Operation)

    $state = & $Provider.Validate $Operation
    $transaction = & $Provider.Prepare $state $Operation
    try {
        & $Provider.CloseApplication $transaction
        & $Provider.StopService $transaction
        if ($Operation -eq 'Uninstall') {
            & $Provider.DeleteService $transaction
            & $Provider.RemoveFiles $transaction
            & $Provider.RemoveRegistration $transaction
        } else {
            & $Provider.ReplaceFiles $transaction
            & $Provider.ConfigureService $transaction
            & $Provider.RestoreServiceState $transaction
            & $Provider.RegisterInstallation $transaction
        }
        & $Provider.Commit $transaction
    } catch {
        $failure = $_
        try { & $Provider.Rollback $transaction }
        catch { throw "SETUP_RECOVERY_REQUIRED: $($transaction.JournalDirectory). $($_.Exception.Message)" }
        throw "SETUP_ROLLED_BACK: $($failure.Exception.Message)"
    }
}
