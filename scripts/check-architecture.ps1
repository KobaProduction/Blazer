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
        'client-core' = @('vpn-application', 'vpn-infrastructure')
        'vpn-presentation' = @('vpn-application')
        'vpn-image-import' = @('vpn-presentation')
        'client-ui' = @('vpn-presentation', 'vpn-image-import')
        'blazer' = @('client-ui', 'client-cli', 'client-core', 'vpn-application', 'vpn-presentation', 'vpn-image-import')
        'vpn-android' = @('client-ui', 'client-core', 'vpn-infrastructure', 'vpn-presentation', 'vpn-image-import')
        'blazer-linux' = @('client-ui', 'client-core', 'vpn-infrastructure', 'vpn-presentation')
        'blazer-macos-preview' = @('client-ui', 'vpn-presentation')
        'blazer-ios-preview' = @('client-ui', 'vpn-presentation')
        'client-cli' = @('client-core')
    }
    $manifestLocations = @{
        'vpn-domain' = 'src/domain/Cargo.toml'
        'vpn-application' = 'src/application/Cargo.toml'
        'vpn-infrastructure' = 'src/infrastructure/Cargo.toml'
        'client-core' = 'src/client-core/Cargo.toml'
        'vpn-presentation' = 'src/presentation/shared/Cargo.toml'
        'vpn-image-import' = 'src/presentation/image_import/Cargo.toml'
        'client-ui' = 'src/presentation/client-ui/Cargo.toml'
        'blazer' = 'src/platform/windows/Cargo.toml'
        'vpn-android' = 'src/platform/android/Cargo.toml'
        'blazer-linux' = 'src/platform/linux/Cargo.toml'
        'blazer-macos-preview' = 'src/platform/macos/Cargo.toml'
        'blazer-ios-preview' = 'src/platform/ios/Cargo.toml'
        'client-cli' = 'src/presentation/client-cli/Cargo.toml'
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
            if ($package.name -eq 'client-ui' -and -not $internal -and
                $dependency.name -notin @('chrono', 'egui', 'epaint_default_fonts', 'image', 'serde_json', 'skrifa')) {
                throw "Unexpected dependency in the shared egui view: $($dependency.name)"
            }
            if ($package.name -in @('blazer-macos-preview', 'blazer-ios-preview') -and
                $dependency.name -eq 'eframe' -and 'glow' -in $dependency.features) {
                throw 'Apple previews must use the Metal renderer'
            }
        }
        $manifest = Get-Content -Raw -LiteralPath $package.manifest_path
        if ($manifest -notmatch '(?ms)^\[lints\]\s*\nworkspace\s*=\s*true\s*(?:\n|$)') {
            throw "Workspace lints must be inherited: $($package.name)"
        }
    }
    $headlessTree = & cargo tree --locked -p client-cli --target x86_64-pc-windows-msvc --edges normal,build,dev --prefix none
    if ($LASTEXITCODE -ne 0) { throw 'Headless dependency inspection failed' }
    $guiPattern = '^(client-ui|eframe|egui(?:[-_].*)?|epaint.*|winit|wgpu.*|wry|tauri.*|iced.*|slint.*|glutin.*|webview.*) v'
    if ($headlessTree | Where-Object { $_ -match $guiPattern }) {
        throw 'Headless dependency graph must not include GUI/window/webview packages'
    }
    $androidTree = & cargo tree --locked -p vpn-android --target x86_64-linux-android --edges normal,build --prefix none
    if ($LASTEXITCODE -ne 0) { throw 'Android dependency inspection failed' }
    $androidExcluded = '^(eframe|winit|wgpu.*|glutin.*|rfd|win-screenshot|windows-registry|windows-dpapi|native-tls|openssl-sys|wry|tauri.*|webview.*) v'
    if ($androidTree | Where-Object { $_ -match $androidExcluded }) {
        throw 'Android must render egui through its native surface without desktop window or system adapters'
    }
    Write-Output 'Architecture, MIT, lint inheritance, Android and GUI-free headless checks passed.'
}
finally {
    Pop-Location
}
