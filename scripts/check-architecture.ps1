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
        'vpn-backend' = @('vpn-application', 'vpn-infrastructure')
        'vpn-presentation' = @('vpn-application')
        'vpn-image-import' = @('vpn-presentation')
        'vpn-client' = @('vpn-backend', 'vpn-presentation', 'vpn-image-import')
        'vpn-android' = @('vpn-backend', 'vpn-infrastructure', 'vpn-presentation', 'vpn-image-import')
        'vpn-headless' = @('vpn-backend')
    }
    $manifestLocations = @{
        'vpn-domain' = 'src/domain/Cargo.toml'
        'vpn-application' = 'src/application/Cargo.toml'
        'vpn-infrastructure' = 'src/infrastructure/Cargo.toml'
        'vpn-backend' = 'src/backend/Cargo.toml'
        'vpn-presentation' = 'src/presentation/shared/Cargo.toml'
        'vpn-image-import' = 'src/presentation/image_import/Cargo.toml'
        'vpn-client' = 'src/presentation/desktop/Cargo.toml'
        'vpn-android' = 'src/presentation/android/Cargo.toml'
        'vpn-headless' = 'src/presentation/headless/Cargo.toml'
    }
    $workspacePackages = @($metadata.packages | Where-Object { $_.id -in $metadata.workspace_members })
    if ($workspacePackages.Count -ne $allowed.Count) { throw 'Update the architectural contract for workspace changes' }
    foreach ($package in $workspacePackages) {
        if (-not $allowed.ContainsKey($package.name)) { throw "Unrecognized package: $($package.name)" }
        $expectedPath = [IO.Path]::GetFullPath((Join-Path $repoRoot $manifestLocations[$package.name]))
        if ([IO.Path]::GetFullPath($package.manifest_path) -ne $expectedPath) {
            throw "Package outside its architectural layer: $($package.name)"
        }
        if ($package.license -ne 'MIT') { throw "Own packages must inherit MIT: $($package.name)" }
        foreach ($dependency in $package.dependencies) {
            $internal = $dependency.name -in $allowed.Keys
            if ($internal -and $dependency.name -notin $allowed[$package.name]) {
                throw "Forbidden layer dependency: $($package.name) -> $($dependency.name)"
            }
            if (-not $internal -and $package.name -in @('vpn-domain', 'vpn-application', 'vpn-presentation')) {
                throw "Platform-independent layer has an external dependency: $($package.name) -> $($dependency.name)"
            }
        }
        $manifest = Get-Content -Raw -LiteralPath $package.manifest_path
        if ($manifest -notmatch '(?ms)^\[lints\]\s*\nworkspace\s*=\s*true\s*(?:\n|$)') {
            throw "Workspace lints must be inherited: $($package.name)"
        }
    }
    $headlessTree = & cargo tree --locked -p vpn-headless --target x86_64-pc-windows-msvc --edges normal,build,dev --prefix none
    if ($LASTEXITCODE -ne 0) { throw 'Headless dependency inspection failed' }
    $guiPattern = '^(eframe|egui(?:[-_].*)?|epaint.*|winit|wgpu.*|wry|tauri.*|iced.*|slint.*|glutin.*|webview.*) v'
    if ($headlessTree | Where-Object { $_ -match $guiPattern }) {
        throw 'Headless dependency graph must not include GUI/window/webview packages'
    }
    $androidTree = & cargo tree --locked -p vpn-android --target x86_64-linux-android --edges normal,build --prefix none
    if ($LASTEXITCODE -ne 0) { throw 'Android dependency inspection failed' }
    if ($androidTree | Where-Object { $_ -match $guiPattern -or $_ -match '^(windows-dpapi|native-tls|openssl-sys) v' }) {
        throw 'Android dependency graph must not contain desktop GUI, DPAPI or desktop TLS adapters'
    }
    Write-Output 'Architecture, MIT, lint inheritance, Android and GUI-free headless checks passed.'
}
finally {
    Pop-Location
}
