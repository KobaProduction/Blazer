$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = Split-Path $PSScriptRoot -Parent
Push-Location $repoRoot
try {
    $metadataJson = & cargo metadata --no-deps --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
    $metadata = ($metadataJson -join "`n") | ConvertFrom-Json
    $allowed = @{
        'vpn-domain' = @()
        'vpn-application' = @('vpn-domain')
        'vpn-infrastructure' = @('vpn-domain', 'vpn-application')
        'vpn-client' = @('vpn-domain', 'vpn-application', 'vpn-infrastructure')
    }
    $workspacePackages = @($metadata.packages | Where-Object { $_.id -in $metadata.workspace_members })
    if ($workspacePackages.Count -ne $allowed.Count) { throw 'Update the architectural contract for workspace changes' }
    foreach ($package in $workspacePackages) {
        if (-not $allowed.ContainsKey($package.name)) { throw "Unrecognized package: $($package.name)" }
        foreach ($dependency in $package.dependencies) {
            $internal = $dependency.name -in $allowed.Keys
            if ($internal -and $dependency.name -notin $allowed[$package.name]) {
                throw "Forbidden layer dependency: $($package.name) -> $($dependency.name)"
            }
            if (-not $internal -and $package.name -in @('vpn-domain', 'vpn-application')) {
                throw "Inner layer has an external dependency: $($package.name) -> $($dependency.name)"
            }
        }
        $manifest = Get-Content -Raw -LiteralPath $package.manifest_path
        if ($manifest -notmatch '(?ms)^\[lints\]\s*\nworkspace\s*=\s*true\s*(?:\n|$)') {
            throw "Workspace lints must be inherited: $($package.name)"
        }
    }
    Write-Output 'Architecture dependency and lint-inheritance checks passed.'
}
finally {
    Pop-Location
}
