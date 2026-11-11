param(
    [ValidateSet('All', 'Architecture', 'Format', 'Lint', 'Check', 'Test', 'Build')]
    [string]$Stage = 'All'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = Split-Path $PSScriptRoot -Parent
$buildTarget = 'x86_64-pc-windows-msvc'

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
        Invoke-CargoGate @('clippy', '--workspace', '--all-targets', '--locked', '--target', $buildTarget, '--', '-D', 'warnings', '-F', 'clippy::cognitive_complexity')
    }
    if ($Stage -in @('All', 'Check')) {
        Invoke-CargoGate @('check', '--workspace', '--all-targets', '--locked', '--target', $buildTarget)
    }
    if ($Stage -in @('All', 'Test')) {
        Invoke-CargoGate @('test', '--workspace', '--locked', '--target', $buildTarget)
    }
    if ($Stage -in @('All', 'Build')) {
        Invoke-CargoGate @('build', '--release', '--locked', '--target', $buildTarget, '-p', 'vpn-client')
    }
}
finally {
    Pop-Location
}
