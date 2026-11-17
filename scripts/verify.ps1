param(
    [ValidateSet('All', 'Architecture', 'Format', 'Lint', 'Check', 'Test', 'Build')]
    [string]$Stage = 'All'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = Split-Path $PSScriptRoot -Parent
$buildTarget = 'x86_64-pc-windows-msvc'
$workspaceChecks = @('--workspace', '--features', 'blazer/demo', '--locked', '--target', $buildTarget)

function Invoke-CargoGate {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) { throw "Cargo gate failed: $($CargoArguments -join ' ')" }
}

Push-Location $repoRoot
try {
    if ($Stage -in @('All', 'Architecture')) {
        & "$PSScriptRoot/check-architecture.ps1"
    }
    if ($Stage -in @('All', 'Format')) {
        Invoke-CargoGate @('fmt', '--all', '--check')
    }
    if ($Stage -in @('All', 'Lint')) {
        Invoke-CargoGate (@('clippy', '--all-targets') + $workspaceChecks + @('--', '-D', 'warnings', '-F', 'clippy::cognitive_complexity'))
    }
    if ($Stage -in @('All', 'Check')) {
        Invoke-CargoGate (@('check', '--all-targets') + $workspaceChecks)
    }
    if ($Stage -in @('All', 'Test')) {
        Invoke-CargoGate (@('test') + $workspaceChecks)
    }
    if ($Stage -in @('All', 'Build')) {
        Invoke-CargoGate @('build', '--release', '--locked', '--target', $buildTarget, '-p', 'blazer')
    }
}
finally {
    Pop-Location
}
